//! Detects, backs up, previews, and merges the Claude Code hook config into
//! `~/.claude/settings.json` (global) or `<project>/.claude/settings.json`
//! (project-level) — never touching any other existing hooks or settings in
//! that file.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const NOTIFICATION_ROUTE: &str = "notification";
const STOP_ROUTE: &str = "stop";
const TASK_COMPLETED_ROUTE: &str = "task-completed";

pub fn global_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("settings.json"))
}

pub fn project_settings_path(project_dir: &str) -> PathBuf {
    Path::new(project_dir).join(".claude").join("settings.json")
}

fn hook_url(port: u16, token: &str, route: &str) -> String {
    format!("http://127.0.0.1:{port}/hooks/claude-code/{route}/{token}")
}

/// Reads settings.json, or an empty object if it doesn't exist yet. Returns
/// an error (rather than silently proceeding) if the file exists but isn't
/// valid JSON — we should never guess how to merge into a file we can't
/// parse.
pub fn read_settings(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content)
        .map_err(|e| format!("existing settings.json is not valid JSON: {e}"))
}

/// Returns true only when all Samlu hooks point at this installation's exact
/// token. Prefix-only checks can report a stale, unauthorized hook as healthy.
pub fn is_installed(value: &Value, port: u16, token: &str) -> bool {
    for (event, route) in [
        ("Notification", NOTIFICATION_ROUTE),
        ("Stop", STOP_ROUTE),
        ("TaskCompleted", TASK_COMPLETED_ROUTE),
    ] {
        let expected = hook_url(port, token, route);
        let entries = value
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|e| e.as_array());
        let Some(entries) = entries else {
            return false;
        };
        let found = entries.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("url").and_then(|u| u.as_str()) == Some(expected.as_str())
                    })
                })
        });
        if !found {
            return false;
        }
    }
    true
}

/// Returns a new value with exactly one current Samlu handler under each
/// supported event. Stale Samlu URLs are replaced while unrelated handlers,
/// including handlers sharing the same matcher entry, are preserved.
pub fn merge_hooks(existing: &Value, port: u16, token: &str) -> Result<Value, String> {
    let mut root = existing.clone();
    if !root.is_object() {
        return Err("settings.json must contain an object at the top level".to_string());
    }
    let root_obj = root.as_object_mut().expect("just ensured object above");

    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Err("the existing \"hooks\" value must be an object".to_string());
    };

    let prefix = format!("http://127.0.0.1:{port}/hooks/claude-code/");
    for (event, route) in [
        ("Notification", NOTIFICATION_ROUTE),
        ("Stop", STOP_ROUTE),
        ("TaskCompleted", TASK_COMPLETED_ROUTE),
    ] {
        let event_value = hooks_obj
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(arr) = event_value.as_array_mut() else {
            return Err(format!("the existing hooks.{event} value must be an array"));
        };

        for entry in arr.iter_mut() {
            if let Some(handlers) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                handlers.retain(|handler| {
                    !handler
                        .get("url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| url.starts_with(&prefix))
                });
            }
        }
        arr.retain(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .map_or(true, |handlers| !handlers.is_empty())
        });

        let url = hook_url(port, token, route);
        arr.push(json!({
            "hooks": [
                { "type": "http", "url": url, "timeout": 5 }
            ]
        }));
    }

    Ok(root)
}

/// Just the hooks snippet (not merged with any existing file) — for the
/// "copy this JSON yourself" fallback path.
pub fn snippet(port: u16, token: &str) -> Result<Value, String> {
    merge_hooks(&json!({}), port, token)
}

fn backup_path(app_data_dir: &Path, original: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let name = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    app_data_dir
        .join("hook-backups")
        .join(format!("{name}.{stamp}.bak"))
}

/// Backs up the existing file (if any) into the app's own data dir, then
/// writes the merged config. Refuses to touch a file that exists but isn't a
/// valid JSON object at the top level.
pub fn apply(path: &Path, app_data_dir: &Path, port: u16, token: &str) -> Result<(), String> {
    let existing = read_settings(path)?;
    if !existing.is_object() {
        return Err("existing settings.json is not a JSON object at the top level — refusing to modify it automatically. Use the \"copy snippet\" option instead.".to_string());
    }

    if path.exists() {
        let backup = backup_path(app_data_dir, path);
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(path, &backup).map_err(|e| e.to_string())?;
    }

    let merged = merge_hooks(&existing, port, token)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pretty = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    fs::write(path, pretty).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_installed, merge_hooks};
    use serde_json::json;

    #[test]
    fn replaces_stale_urls_and_preserves_other_handlers() {
        let existing = json!({
            "hooks": {
                "Notification": [{
                    "hooks": [
                        {"type": "http", "url": "http://127.0.0.1:47823/hooks/claude-code/notification/old"},
                        {"type": "command", "command": "echo keep"}
                    ]
                }]
            }
        });
        let merged = merge_hooks(&existing, 47823, "new").unwrap();
        let text = merged.to_string();
        assert!(!text.contains("/old"));
        assert!(text.contains("echo keep"));
        assert!(is_installed(&merged, 47823, "new"));
    }
}

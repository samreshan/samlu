//! Detects, backs up, previews, and merges the Claude Code hook config into
//! `~/.claude/settings.json` (global) or `<project>/.claude/settings.json`
//! (project-level) — never touching any other existing hooks or settings in
//! that file.

use super::InstallState;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Every event Samlu listens to, with the route its handler posts to.
const EVENTS: [(&str, &str); 8] = [
    ("SessionStart", "session-start"),
    ("SessionEnd", "session-end"),
    ("UserPromptSubmit", "prompt"),
    ("PermissionRequest", "permission"),
    ("PostToolUse", "post-tool"),
    ("Notification", "notification"),
    ("Stop", "stop"),
    ("TaskCompleted", "task-completed"),
];

/// Claude holds a PermissionRequest hook open until Samlu (answering at 590 s)
/// or the user responds, so its timeout is far longer than the others.
const PERMISSION_TIMEOUT_SECS: u64 = 600;
const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// Env vars forwarded as `X-Samlu-*` headers so Samlu can find the window a
/// session runs in. Claude expands `$VAR` in header values only for vars
/// listed in `allowedEnvVars`.
const ORIGIN_HEADERS: [(&str, &str); 4] = [
    ("X-Samlu-Entrypoint", "CLAUDE_CODE_ENTRYPOINT"),
    ("X-Samlu-Host-Session", "CLAUDE_CODE_HOST_SESSION_ID"),
    ("X-Samlu-Warp-Focus", "WARP_FOCUS_URL"),
    ("X-Samlu-Term", "TERM_PROGRAM"),
];

const URL_MARKER: &str = "/hooks/claude-code/";

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

fn handler(port: u16, token: &str, event: &str, route: &str) -> Value {
    let timeout = if event == "PermissionRequest" {
        PERMISSION_TIMEOUT_SECS
    } else {
        DEFAULT_TIMEOUT_SECS
    };
    let headers: Map<String, Value> = ORIGIN_HEADERS
        .iter()
        .map(|(header, var)| (header.to_string(), json!(format!("${var}"))))
        .collect();
    let vars: Vec<&str> = ORIGIN_HEADERS.iter().map(|(_, var)| *var).collect();
    json!({
        "type": "http",
        "url": hook_url(port, token, route),
        "timeout": timeout,
        "headers": headers,
        "allowedEnvVars": vars,
    })
}

fn handlers_of(value: &Value, event: &str) -> Vec<Value> {
    value
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("hooks").and_then(Value::as_array))
                .flatten()
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn is_samlu_handler(handler: &Value) -> bool {
    handler
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| url.contains(URL_MARKER))
}

/// `Installed` only when every event has a handler identical to what
/// `merge_hooks` writes for this port and token (URL, timeout, headers).
/// Samlu present but not matching, such as an older three-event install or a
/// stale token, is `NeedsUpdate`.
pub fn status(value: &Value, port: u16, token: &str) -> InstallState {
    let samlu_present = EVENTS
        .iter()
        .any(|(event, _)| handlers_of(value, event).iter().any(is_samlu_handler));
    if !samlu_present {
        return InstallState::NotInstalled;
    }
    let complete = EVENTS.iter().all(|(event, route)| {
        let expected = handler(port, token, event, route);
        handlers_of(value, event).contains(&expected)
    });
    if complete {
        InstallState::Installed
    } else {
        InstallState::NeedsUpdate
    }
}

/// True only when all Samlu hooks are current for this installation.
pub fn is_installed(value: &Value, port: u16, token: &str) -> bool {
    status(value, port, token) == InstallState::Installed
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

    let prefix = format!("http://127.0.0.1:{port}{URL_MARKER}");
    for (event, route) in EVENTS {
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
                .is_none_or(|handlers| !handlers.is_empty())
        });

        arr.push(json!({ "hooks": [handler(port, token, event, route)] }));
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

/// Removes only Samlu's localhost hooks while retaining every unrelated hook
/// and matcher in the user's Claude configuration.
pub fn uninstall(path: &Path, app_data_dir: &Path) -> Result<(), String> {
    let existing = read_settings(path)?;
    if !existing.is_object() {
        return Err("existing settings.json is not a JSON object at the top level".to_string());
    }
    if path.exists() {
        let backup = backup_path(app_data_dir, path);
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(path, backup).map_err(|error| error.to_string())?;
    }
    let mut updated = existing;
    let root = updated
        .as_object_mut()
        .expect("just ensured configuration is an object");
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for (event, _) in EVENTS {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                for entry in entries.iter_mut() {
                    if let Some(handlers) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                        handlers.retain(|handler| {
                            !handler
                                .get("url")
                                .and_then(Value::as_str)
                                .is_some_and(|url| url.contains(URL_MARKER))
                        });
                    }
                }
                entries.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_none_or(|handlers| !handlers.is_empty())
                });
            }
        }
    }
    let pretty = serde_json::to_string_pretty(&updated).map_err(|error| error.to_string())?;
    fs::write(path, pretty).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{is_installed, merge_hooks, status, uninstall, EVENTS};
    use crate::setup::InstallState;
    use serde_json::{json, Value};

    fn old_three_event_install(token: &str) -> Value {
        let entry = |route: &str| {
            json!([{ "hooks": [{
                "type": "http",
                "url": format!("http://127.0.0.1:47823/hooks/claude-code/{route}/{token}"),
                "timeout": 5
            }] }])
        };
        json!({ "hooks": {
            "Notification": entry("notification"),
            "Stop": entry("stop"),
            "TaskCompleted": entry("task-completed"),
        } })
    }

    #[test]
    fn status_of_old_three_event_install_is_needs_update() {
        let old = old_three_event_install("tok");
        assert_eq!(status(&old, 47823, "tok"), InstallState::NeedsUpdate);
        assert!(!is_installed(&old, 47823, "tok"));
    }

    #[test]
    fn status_full_install_is_installed() {
        let merged = merge_hooks(&json!({}), 47823, "tok").unwrap();
        assert_eq!(status(&merged, 47823, "tok"), InstallState::Installed);
        assert!(is_installed(&merged, 47823, "tok"));
        assert_eq!(status(&json!({}), 47823, "tok"), InstallState::NotInstalled);
    }

    #[test]
    fn stale_token_is_needs_update() {
        let merged = merge_hooks(&json!({}), 47823, "old").unwrap();
        assert_eq!(status(&merged, 47823, "new"), InstallState::NeedsUpdate);
    }

    #[test]
    fn missing_headers_is_needs_update() {
        let mut merged = merge_hooks(&json!({}), 47823, "tok").unwrap();
        let handler = &mut merged["hooks"]["Stop"][0]["hooks"][0];
        handler.as_object_mut().unwrap().remove("headers");
        assert_eq!(status(&merged, 47823, "tok"), InstallState::NeedsUpdate);
    }

    #[test]
    fn permission_timeout_600() {
        let merged = merge_hooks(&json!({}), 47823, "tok").unwrap();
        assert_eq!(
            merged["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
            600
        );
        assert_eq!(merged["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
        let handler = &merged["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(handler["headers"]["X-Samlu-Term"], "$TERM_PROGRAM");
        assert_eq!(
            handler["allowedEnvVars"],
            json!([
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_HOST_SESSION_ID",
                "WARP_FOCUS_URL",
                "TERM_PROGRAM"
            ])
        );
    }

    #[test]
    fn merge_replaces_stale_and_keeps_foreign_handlers() {
        let mut hooks = serde_json::Map::new();
        for (event, route) in EVENTS {
            hooks.insert(
                event.to_string(),
                json!([{ "hooks": [
                    {"type": "http", "url": format!("http://127.0.0.1:47823/hooks/claude-code/{route}/old")},
                    {"type": "command", "command": format!("echo keep-{event}")}
                ] }]),
            );
        }
        let existing = json!({ "hooks": hooks, "model": "opus" });
        let merged = merge_hooks(&existing, 47823, "new").unwrap();
        let text = merged.to_string();
        assert!(!text.contains("/old"));
        assert_eq!(merged["model"], "opus");
        for (event, _) in EVENTS {
            assert!(text.contains(&format!("echo keep-{event}")));
            let samlu = merged["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|e| e["hooks"].as_array().unwrap())
                .filter(|h| {
                    h["url"]
                        .as_str()
                        .is_some_and(|u| u.contains("/hooks/claude-code/"))
                })
                .count();
            assert_eq!(samlu, 1, "{event}");
        }
        assert!(is_installed(&merged, 47823, "new"));
    }

    #[test]
    fn uninstall_removes_all_eight() {
        let dir = std::env::temp_dir().join(format!("samlu-claude-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut merged = merge_hooks(&json!({}), 47823, "tok").unwrap();
        merged["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [{"type": "command", "command": "echo keep"}] }));
        std::fs::write(&path, merged.to_string()).unwrap();
        uninstall(&path, &dir).unwrap();
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!after.to_string().contains("/hooks/claude-code/"));
        assert!(after.to_string().contains("echo keep"));
        assert_eq!(status(&after, 47823, "tok"), InstallState::NotInstalled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

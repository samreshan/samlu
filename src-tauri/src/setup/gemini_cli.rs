//! Gemini CLI hook installation. The hooks stream their JSON stdin directly
//! to Samlu's local listener, so no shell wrapper or persistent helper is
//! needed.

use super::json_config;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const AFTER_AGENT: &str = "samlu-gemini-after-agent";
const NOTIFICATION: &str = "samlu-gemini-notification";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub path: String,
    pub exists: bool,
    pub installed: bool,
}

pub fn config_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".gemini").join("settings.json"))
        .ok_or_else(|| "could not determine your home directory".to_string())
}

fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "/usr/bin/curl --silent --show-error --max-time 3 -X POST -H 'Content-Type: application/json' --data-binary @- 'http://127.0.0.1:{port}/hooks/gemini-cli/{route}/{token}' >/dev/null 2>&1 || true"
    )
}

fn hook(name: &str, command: String) -> Value {
    json!({
        "type": "command",
        "name": name,
        "description": "Send a Gemini CLI agent update to Samlu",
        "command": command,
        "timeout": 5000
    })
}

fn contains_named_hook(value: &Value, event: &str, name: &str) -> bool {
    value
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks
                            .iter()
                            .any(|hook| hook.get("name").and_then(Value::as_str) == Some(name))
                    })
            })
        })
}

pub fn installed(value: &Value) -> bool {
    contains_named_hook(value, "AfterAgent", AFTER_AGENT)
        && contains_named_hook(value, "Notification", NOTIFICATION)
}

pub fn merge(existing: &Value, port: u16, token: &str) -> Result<Value, String> {
    let mut root = existing.clone();
    let root_object = json_config::root_object(&mut root)?;
    let hooks = root_object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks_object = hooks
        .as_object_mut()
        .ok_or_else(|| "the existing \"hooks\" value must be an object".to_string())?;

    for (event, name, route, matcher) in [
        ("AfterAgent", AFTER_AGENT, "after-agent", "*"),
        (
            "Notification",
            NOTIFICATION,
            "notification",
            "ToolPermission",
        ),
    ] {
        let groups = hooks_object
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("the existing hooks.{event} value must be an array"))?;
        remove_named_hook(groups, name);
        groups.push(json!({
            "matcher": matcher,
            "hooks": [hook(name, curl_command(port, token, route))]
        }));
    }
    Ok(root)
}

pub fn remove(existing: &Value) -> Result<Value, String> {
    let mut root = existing.clone();
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(root);
    };
    for (event, name) in [("AfterAgent", AFTER_AGENT), ("Notification", NOTIFICATION)] {
        if let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) {
            remove_named_hook(groups, name);
        }
    }
    Ok(root)
}

fn remove_named_hook(groups: &mut Vec<Value>, name: &str) {
    for group in groups.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            hooks.retain(|hook| hook.get("name").and_then(Value::as_str) != Some(name));
        }
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .map_or(true, |hooks| !hooks.is_empty())
    });
}

pub fn status() -> Result<Status, String> {
    let path = config_path()?;
    let existing = json_config::read(&path)?;
    Ok(Status {
        path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        installed: installed(&existing),
    })
}

pub fn preview(port: u16, token: &str) -> Result<String, String> {
    let path = config_path()?;
    let merged = merge(&json_config::read(&path)?, port, token)?;
    serde_json::to_string_pretty(&merged).map_err(|error| error.to_string())
}

pub fn apply(app_data_dir: &Path, port: u16, token: &str) -> Result<(), String> {
    let path = config_path()?;
    let existing = json_config::read(&path)?;
    json_config::backup(&path, app_data_dir, "gemini-settings")?;
    json_config::write(&path, &merge(&existing, port, token)?)
}

pub fn uninstall(app_data_dir: &Path) -> Result<(), String> {
    let path = config_path()?;
    let existing = json_config::read(&path)?;
    json_config::backup(&path, app_data_dir, "gemini-settings")?;
    json_config::write(&path, &remove(&existing)?)
}

#[cfg(test)]
mod tests {
    use super::{installed, merge, remove};
    use serde_json::json;

    #[test]
    fn preserves_existing_hooks_and_removes_only_samlu_entries() {
        let existing = json!({"hooks":{"AfterAgent":[{"hooks":[{"name":"keep"}]}]}});
        let merged = merge(&existing, 47823, "token").unwrap();
        assert!(installed(&merged));
        assert!(merged.to_string().contains("keep"));
        let removed = remove(&merged).unwrap();
        assert!(!installed(&removed));
        assert!(removed.to_string().contains("keep"));
    }
}

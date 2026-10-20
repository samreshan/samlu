//! Gemini CLI hook installation. The hooks stream their JSON stdin directly
//! to Samlu's local listener, so no shell wrapper or persistent helper is
//! needed.

use super::{json_config, InstallState};
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
    #[serde(skip)]
    pub state: InstallState,
}

pub fn config_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".gemini").join("settings.json"))
        .ok_or_else(|| "could not determine your home directory".to_string())
}

#[cfg(not(windows))]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "/usr/bin/curl --silent --show-error --max-time 3 -X POST -H 'Content-Type: application/json' -H \"X-Samlu-Warp-Focus: $WARP_FOCUS_URL\" -H \"X-Samlu-Term: $TERM_PROGRAM\" -H \"X-Samlu-Bundle: $__CFBundleIdentifier\" -H \"X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE\" --data-binary @- 'http://127.0.0.1:{port}/hooks/gemini-cli/{route}/{token}' >/dev/null 2>&1 || true"
    )
}

/// Hook commands run through `cmd.exe` on Windows, which ships `curl.exe`.
#[cfg(windows)]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "curl.exe --silent --show-error --max-time 3 -X POST -H \"Content-Type: application/json\" -H \"X-Samlu-Warp-Focus: %WARP_FOCUS_URL%\" -H \"X-Samlu-Term: %TERM_PROGRAM%\" -H \"X-Samlu-Bundle: %__CFBundleIdentifier%\" -H \"X-Samlu-Originator: %CODEX_INTERNAL_ORIGINATOR_OVERRIDE%\" --data-binary @- \"http://127.0.0.1:{port}/hooks/gemini-cli/{route}/{token}\" >NUL 2>&1 || exit /b 0"
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

/// The command of the Samlu hook named `name` under `event`, if present.
fn named_hook_command<'a>(value: &'a Value, event: &str, name: &str) -> Option<&'a str> {
    value
        .get("hooks")?
        .get(event)?
        .as_array()?
        .iter()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|hook| hook.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|hook| hook.get("command").and_then(Value::as_str))
}

/// Present by name but with a different command (an older Samlu, a changed
/// token or port) means the hooks need rewriting.
pub fn install_state(value: &Value, port: u16, token: &str) -> InstallState {
    let entries = [
        ("AfterAgent", AFTER_AGENT, "after-agent"),
        ("Notification", NOTIFICATION, "notification"),
    ];
    let mut found = 0;
    let mut current = 0;
    for (event, name, route) in entries {
        if let Some(command) = named_hook_command(value, event, name) {
            found += 1;
            if command == curl_command(port, token, route) {
                current += 1;
            }
        }
    }
    match (found, current) {
        (0, _) => InstallState::NotInstalled,
        (_, current) if current == entries.len() => InstallState::Installed,
        _ => InstallState::NeedsUpdate,
    }
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
            .is_none_or(|hooks| !hooks.is_empty())
    });
}

pub fn status(port: u16, token: &str) -> Result<Status, String> {
    let path = config_path()?;
    let existing = json_config::read(&path)?;
    Ok(Status {
        path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        state: install_state(&existing, port, token),
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
    use super::{curl_command, install_state, merge, remove};
    use crate::setup::InstallState;
    use serde_json::json;

    #[test]
    fn commands_carry_origin_headers() {
        let command = curl_command(47823, "token", "after-agent");
        for header in [
            "X-Samlu-Warp-Focus",
            "X-Samlu-Term",
            "X-Samlu-Bundle",
            "X-Samlu-Originator",
        ] {
            assert!(command.contains(header), "{header}");
        }
    }

    #[test]
    fn preserves_existing_hooks_and_removes_only_samlu_entries() {
        let existing = json!({"hooks":{"AfterAgent":[{"hooks":[{"name":"keep"}]}]}});
        let merged = merge(&existing, 47823, "token").unwrap();
        assert_eq!(
            install_state(&merged, 47823, "token"),
            InstallState::Installed
        );
        assert!(merged.to_string().contains("keep"));
        let removed = remove(&merged).unwrap();
        assert_eq!(
            install_state(&removed, 47823, "token"),
            InstallState::NotInstalled
        );
        assert!(removed.to_string().contains("keep"));
    }

    #[test]
    fn hooks_without_the_current_command_need_an_update() {
        let merged = merge(&json!({}), 47823, "token").unwrap();
        assert_eq!(
            install_state(&merged, 47823, "other"),
            InstallState::NeedsUpdate
        );
        let old = merged.to_string().replace("X-Samlu-Bundle", "X-Old");
        let old: serde_json::Value = serde_json::from_str(&old).unwrap();
        assert_eq!(
            install_state(&old, 47823, "token"),
            InstallState::NeedsUpdate
        );
    }
}

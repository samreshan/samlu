//! Google Antigravity hook installation.
//!
//! Antigravity stores named hook configurations in `hooks.json`, separate from
//! Gemini CLI's `settings.json`. Samlu owns only the top-level `samlu` entry.

use super::{json_config, InstallState};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SAMLU_HOOK_NAME: &str = "samlu";

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
        .map(|home| home.join(".gemini").join("config").join("hooks.json"))
        .ok_or_else(|| "could not determine your home directory".to_string())
}

#[cfg(not(windows))]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "/usr/bin/curl --silent --show-error --max-time 3 -X POST -H 'Content-Type: application/json' -H \"X-Samlu-Warp-Focus: $WARP_FOCUS_URL\" -H \"X-Samlu-Term: $TERM_PROGRAM\" -H \"X-Samlu-Bundle: $__CFBundleIdentifier\" -H \"X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE\" --data-binary @- 'http://127.0.0.1:{port}/hooks/antigravity/{route}/{token}' >/dev/null 2>&1 || true"
    )
}

/// Hook commands run through `cmd.exe` on Windows, which ships `curl.exe`.
#[cfg(windows)]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "curl.exe --silent --show-error --max-time 3 -X POST -H \"Content-Type: application/json\" -H \"X-Samlu-Warp-Focus: %WARP_FOCUS_URL%\" -H \"X-Samlu-Term: %TERM_PROGRAM%\" -H \"X-Samlu-Bundle: %__CFBundleIdentifier%\" -H \"X-Samlu-Originator: %CODEX_INTERNAL_ORIGINATOR_OVERRIDE%\" --data-binary @- \"http://127.0.0.1:{port}/hooks/antigravity/{route}/{token}\" >NUL 2>&1 || exit /b 0"
    )
}

fn command(name: &str, command: String) -> Value {
    json!({
        "type": "command",
        "name": name,
        "command": command,
        "timeout": 5
    })
}

/// A command string under `Stop` or `PreToolUse` in Samlu's entry.
fn samlu_commands(value: &Value) -> (Option<&str>, Option<&str>) {
    let Some(configuration) = value.get(SAMLU_HOOK_NAME).and_then(Value::as_object) else {
        return (None, None);
    };
    let stop = configuration
        .get("Stop")
        .and_then(Value::as_array)
        .and_then(|hooks| hooks.first())
        .and_then(|hook| hook.get("command"))
        .and_then(Value::as_str);
    let needs_input = configuration
        .get("PreToolUse")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .and_then(|group| group.get("hooks"))
        .and_then(Value::as_array)
        .and_then(|hooks| hooks.first())
        .and_then(|hook| hook.get("command"))
        .and_then(Value::as_str);
    (stop, needs_input)
}

/// Present but with a different command (an older Samlu, a changed token or
/// port) means the entry needs rewriting.
pub fn install_state(value: &Value, port: u16, token: &str) -> InstallState {
    let (stop, needs_input) = samlu_commands(value);
    if value.get(SAMLU_HOOK_NAME).is_none() {
        return InstallState::NotInstalled;
    }
    if stop == Some(curl_command(port, token, "stop").as_str())
        && needs_input == Some(curl_command(port, token, "needs-input").as_str())
    {
        InstallState::Installed
    } else if stop.is_none() && needs_input.is_none() {
        InstallState::NotInstalled
    } else {
        InstallState::NeedsUpdate
    }
}

pub fn merge(existing: &Value, port: u16, token: &str) -> Result<Value, String> {
    let mut root = existing.clone();
    let object = json_config::root_object(&mut root)?;
    object.insert(
        SAMLU_HOOK_NAME.to_string(),
        json!({
            "enabled": true,
            "Stop": [command("samlu-antigravity-stop", curl_command(port, token, "stop"))],
            "PreToolUse": [{
                "matcher": "ask_permission|ask_question",
                "hooks": [command(
                    "samlu-antigravity-needs-input",
                    curl_command(port, token, "needs-input")
                )]
            }]
        }),
    );
    Ok(root)
}

pub fn remove(existing: &Value) -> Result<Value, String> {
    let mut root = existing.clone();
    json_config::root_object(&mut root)?.remove(SAMLU_HOOK_NAME);
    Ok(root)
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
    json_config::backup(&path, app_data_dir, "antigravity-hooks")?;
    json_config::write(&path, &merge(&existing, port, token)?)
}

pub fn uninstall(app_data_dir: &Path) -> Result<(), String> {
    let path = config_path()?;
    let existing = json_config::read(&path)?;
    json_config::backup(&path, app_data_dir, "antigravity-hooks")?;
    json_config::write(&path, &remove(&existing)?)
}

#[cfg(test)]
mod tests {
    use super::{curl_command, install_state, merge, remove};
    use crate::setup::InstallState;
    use serde_json::json;

    #[test]
    fn commands_carry_origin_headers() {
        let command = curl_command(47823, "token", "needs-input");
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
    fn owns_only_its_named_configuration() {
        let existing = json!({"team-default": {"Stop": []}});
        let merged = merge(&existing, 47823, "token").unwrap();
        assert_eq!(
            install_state(&merged, 47823, "token"),
            InstallState::Installed
        );
        assert!(merged.get("team-default").is_some());
        let removed = remove(&merged).unwrap();
        assert_eq!(
            install_state(&removed, 47823, "token"),
            InstallState::NotInstalled
        );
        assert!(removed.get("team-default").is_some());
    }

    #[test]
    fn an_entry_without_the_current_commands_needs_an_update() {
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

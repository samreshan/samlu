//! Google Antigravity hook installation.
//!
//! Antigravity stores named hook configurations in `hooks.json`, separate from
//! Gemini CLI's `settings.json`. Samlu owns only the top-level `samlu` entry.

use super::json_config;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SAMLU_HOOK_NAME: &str = "samlu";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub path: String,
    pub exists: bool,
    pub installed: bool,
}

pub fn config_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".gemini").join("config").join("hooks.json"))
        .ok_or_else(|| "could not determine your home directory".to_string())
}

fn curl_command(port: u16, token: &str, route: &str) -> String {
    format!(
        "/usr/bin/curl --silent --show-error --max-time 3 -X POST -H 'Content-Type: application/json' --data-binary @- 'http://127.0.0.1:{port}/hooks/antigravity/{route}/{token}' >/dev/null 2>&1 || true"
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

pub fn installed(value: &Value) -> bool {
    value
        .get(SAMLU_HOOK_NAME)
        .and_then(Value::as_object)
        .is_some_and(|configuration| {
            configuration
                .get("Stop")
                .and_then(Value::as_array)
                .is_some_and(|hooks| !hooks.is_empty())
                && configuration
                    .get("PreToolUse")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| !hooks.is_empty())
        })
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
    use super::{installed, merge, remove};
    use serde_json::json;

    #[test]
    fn owns_only_its_named_configuration() {
        let existing = json!({"team-default": {"Stop": []}});
        let merged = merge(&existing, 47823, "token").unwrap();
        assert!(installed(&merged));
        assert!(merged.get("team-default").is_some());
        let removed = remove(&merged).unwrap();
        assert!(!installed(&removed));
        assert!(removed.get("team-default").is_some());
    }
}

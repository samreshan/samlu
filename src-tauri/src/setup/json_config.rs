//! Safe JSON config file helpers shared by Gemini CLI and Antigravity setup.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn read(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| format!("existing JSON is invalid: {error}"))
}

pub fn write(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "configuration path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let contents = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".samlu-{}.tmp",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        error.to_string()
    })
}

pub fn backup(path: &Path, app_data_dir: &Path, name: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let backup_dir = app_data_dir.join("hook-backups");
    fs::create_dir_all(&backup_dir).map_err(|error| error.to_string())?;
    let backup = backup_dir.join(format!(
        "{name}.{}.json.bak",
        chrono::Utc::now().format("%Y%m%d-%H%M%S%3f")
    ));
    fs::copy(path, backup).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn root_object(value: &mut Value) -> Result<&mut serde_json::Map<String, Value>, String> {
    value
        .as_object_mut()
        .ok_or_else(|| "configuration must contain a JSON object at the top level".to_string())
}

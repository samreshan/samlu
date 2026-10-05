//! Installs Samlu as Codex CLI's external notification command while
//! preserving the rest of `~/.codex/config.toml`, including comments.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use toml_edit::{value, Array, DocumentMut};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatus {
    pub path: String,
    pub exists: bool,
    pub installed: bool,
    pub command: Vec<String>,
}

pub fn config_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".codex").join("config.toml"))
        .ok_or_else(|| "could not determine your home directory".to_string())
}

fn command(_app: &AppHandle) -> Result<Vec<String>, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(vec![
        executable.to_string_lossy().to_string(),
        "--codex-notify".to_string(),
    ])
}

fn read(path: &Path) -> Result<DocumentMut, String> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    std::fs::read_to_string(path)
        .map_err(|error| error.to_string())?
        .parse::<DocumentMut>()
        .map_err(|error| format!("existing Codex config.toml is invalid: {error}"))
}

fn installed_command(document: &DocumentMut) -> Vec<String> {
    document
        .get("notify")
        .and_then(|item| item.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn merged(mut document: DocumentMut, command: &[String]) -> DocumentMut {
    let mut array = Array::new();
    for part in command {
        array.push(part.as_str());
    }
    document["notify"] = value(array);
    document
}

fn backup(path: &Path, app_data_dir: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let backup = app_data_dir
        .join("hook-backups")
        .join(format!("codex-config.{stamp}.toml.bak"));
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::copy(path, backup).map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_codex_status(app: AppHandle) -> Result<CodexStatus, String> {
    let path = config_path()?;
    let expected = command(&app)?;
    let current = installed_command(&read(&path)?);
    Ok(CodexStatus {
        path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        installed: current == expected,
        command: expected,
    })
}

#[tauri::command]
pub fn preview_codex_config(app: AppHandle) -> Result<String, String> {
    let path = config_path()?;
    Ok(merged(read(&path)?, &command(&app)?).to_string())
}

#[tauri::command]
pub fn apply_codex_config(app: AppHandle) -> Result<(), String> {
    let path = config_path()?;
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    backup(&path, &app_data_dir)?;
    let merged = merged(read(&path)?, &command(&app)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, merged.to_string()).map_err(|error| error.to_string())
}

/// Removes Samlu's notify command when it is still the active value. Codex
/// supports one notify command, so a previous value cannot be safely inferred;
/// the timestamped backup remains available for manual restoration.
pub fn uninstall(app: AppHandle) -> Result<(), String> {
    let path = config_path()?;
    let expected = command(&app)?;
    let mut document = read(&path)?;
    if installed_command(&document) != expected {
        return Ok(());
    }
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    backup(&path, &app_data_dir)?;
    document.remove("notify");
    std::fs::write(path, document.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{installed_command, merged};

    #[test]
    fn preserves_other_codex_settings() {
        let document = "model = \"gpt-5\"\n".parse().unwrap();
        let command = vec![
            "/Applications/Samlu.app/Contents/MacOS/samlu".into(),
            "--codex-notify".into(),
        ];
        let merged = merged(document, &command);
        assert_eq!(merged["model"].as_str(), Some("gpt-5"));
        assert_eq!(installed_command(&merged), command);
    }
}

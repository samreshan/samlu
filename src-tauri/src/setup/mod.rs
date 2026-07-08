//! Tauri commands the Status/Setup UI (public/index.html + settings.js)
//! calls to detect, preview, and apply the Claude Code hook config.

pub mod claude_hooks;

use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize)]
pub struct HookStatus {
    pub path: String,
    pub exists: bool,
    pub installed: bool,
    pub server_running: bool,
    pub port: u16,
    pub last_event: Option<String>,
}

fn resolve_path(global: bool, project_dir: Option<String>) -> Result<PathBuf, String> {
    if global {
        claude_hooks::global_settings_path()
            .ok_or_else(|| "could not determine your home directory".to_string())
    } else {
        let dir = project_dir
            .ok_or_else(|| "project_dir is required for project-level setup".to_string())?;
        Ok(claude_hooks::project_settings_path(&dir))
    }
}

#[tauri::command]
pub fn get_hook_status(
    state: State<'_, std::sync::Arc<AppState>>,
    global: bool,
    project_dir: Option<String>,
) -> Result<HookStatus, String> {
    let path = resolve_path(global, project_dir)?;
    let exists = path.exists();
    let value = claude_hooks::read_settings(&path)?;
    let installed = claude_hooks::is_installed(&value, state.port);
    Ok(HookStatus {
        path: path.to_string_lossy().to_string(),
        exists,
        installed,
        server_running: state.is_server_running(),
        port: state.port,
        last_event: state.last_event_summary(),
    })
}

/// Returns the pretty-printed JSON that WOULD be written, for the setup
/// wizard's diff view — does not touch the file.
#[tauri::command]
pub fn preview_hook_merge(
    state: State<'_, std::sync::Arc<AppState>>,
    global: bool,
    project_dir: Option<String>,
) -> Result<String, String> {
    let path = resolve_path(global, project_dir)?;
    let existing = claude_hooks::read_settings(&path)?;
    if !existing.is_object() {
        return Err(
            "existing settings.json is not a JSON object at the top level".to_string(),
        );
    }
    let merged = claude_hooks::merge_hooks(&existing, state.port, &state.token);
    serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_hook_merge(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    global: bool,
    project_dir: Option<String>,
) -> Result<(), String> {
    let path = resolve_path(global, project_dir)?;
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    claude_hooks::apply(&path, &app_data_dir, state.port, &state.token)
}

/// The raw hooks JSON snippet, for the "copy it myself" fallback path.
#[tauri::command]
pub fn get_hook_snippet(state: State<'_, std::sync::Arc<AppState>>) -> String {
    serde_json::to_string_pretty(&claude_hooks::snippet(state.port, &state.token))
        .unwrap_or_default()
}

//! One product-facing API for the supported agent integrations.

use super::{antigravity, claude_hooks, codex_notify, gemini_cli};
use crate::state::AppState;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub path: String,
    pub exists: bool,
    pub installed: bool,
    pub error: Option<String>,
}

fn unavailable(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    error: String,
) -> IntegrationStatus {
    IntegrationStatus {
        id,
        name,
        description,
        path: "Unavailable".to_string(),
        exists: false,
        installed: false,
        error: Some(error),
    }
}

#[tauri::command]
pub fn get_agent_integrations(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Vec<IntegrationStatus> {
    let claude = match claude_hooks::global_settings_path() {
        Some(path) => match claude_hooks::read_settings(&path) {
            Ok(value) => IntegrationStatus {
                id: "claude-code",
                name: "Claude Code",
                description: "Input requests, completed tasks, and stopped turns.",
                path: path.to_string_lossy().to_string(),
                exists: path.exists(),
                installed: claude_hooks::is_installed(&value, state.port, &state.token),
                error: None,
            },
            Err(error) => unavailable(
                "claude-code",
                "Claude Code",
                "Input requests, completed tasks, and stopped turns.",
                error,
            ),
        },
        None => unavailable(
            "claude-code",
            "Claude Code",
            "Input requests, completed tasks, and stopped turns.",
            "could not determine your home directory".to_string(),
        ),
    };

    let codex = match codex_notify::get_codex_status(app.clone()) {
        Ok(status) => IntegrationStatus {
            id: "codex",
            name: "Codex",
            description: "Completed agent turns through Codex's notify command.",
            path: status.path,
            exists: status.exists,
            installed: status.installed,
            error: None,
        },
        Err(error) => unavailable(
            "codex",
            "Codex",
            "Completed agent turns through Codex's notify command.",
            error,
        ),
    };

    let antigravity = match antigravity::status() {
        Ok(status) => IntegrationStatus {
            id: "antigravity",
            name: "Antigravity",
            description: "Completion and permission/question requests from Antigravity hooks.",
            path: status.path,
            exists: status.exists,
            installed: status.installed,
            error: None,
        },
        Err(error) => unavailable(
            "antigravity",
            "Antigravity",
            "Completion and permission/question requests from Antigravity hooks.",
            error,
        ),
    };

    let gemini = match gemini_cli::status() {
        Ok(status) => IntegrationStatus {
            id: "gemini-cli",
            name: "Gemini CLI",
            description: "Turn completion and tool-permission requests from Gemini CLI hooks.",
            path: status.path,
            exists: status.exists,
            installed: status.installed,
            error: None,
        },
        Err(error) => unavailable(
            "gemini-cli",
            "Gemini CLI",
            "Turn completion and tool-permission requests from Gemini CLI hooks.",
            error,
        ),
    };

    vec![claude, codex, antigravity, gemini]
}

#[tauri::command]
pub fn preview_agent_integration(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    match id.as_str() {
        "claude-code" => {
            let path = claude_hooks::global_settings_path()
                .ok_or_else(|| "could not determine your home directory".to_string())?;
            let existing = claude_hooks::read_settings(&path)?;
            serde_json::to_string_pretty(&claude_hooks::merge_hooks(
                &existing,
                state.port,
                &state.token,
            )?)
            .map_err(|error| error.to_string())
        }
        "codex" => codex_notify::preview_codex_config(app),
        "antigravity" => antigravity::preview(state.port, &state.token),
        "gemini-cli" => gemini_cli::preview(state.port, &state.token),
        _ => Err("unsupported agent integration".to_string()),
    }
}

#[tauri::command]
pub fn apply_agent_integration(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    match id.as_str() {
        "claude-code" => {
            let path = claude_hooks::global_settings_path()
                .ok_or_else(|| "could not determine your home directory".to_string())?;
            claude_hooks::apply(&path, &app_data_dir, state.port, &state.token)
        }
        "codex" => codex_notify::apply_codex_config(app),
        "antigravity" => antigravity::apply(&app_data_dir, state.port, &state.token),
        "gemini-cli" => gemini_cli::apply(&app_data_dir, state.port, &state.token),
        _ => Err("unsupported agent integration".to_string()),
    }
}

#[tauri::command]
pub fn remove_agent_integration(id: String, app: AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    match id.as_str() {
        "claude-code" => {
            let path = claude_hooks::global_settings_path()
                .ok_or_else(|| "could not determine your home directory".to_string())?;
            claude_hooks::uninstall(&path, &app_data_dir)
        }
        "codex" => codex_notify::uninstall(app),
        "antigravity" => antigravity::uninstall(&app_data_dir),
        "gemini-cli" => gemini_cli::uninstall(&app_data_dir),
        _ => Err("unsupported agent integration".to_string()),
    }
}

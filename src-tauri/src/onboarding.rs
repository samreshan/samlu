//! First-run setup. Samlu asks for the two permissions it cannot work without,
//! proves dictation end to end with one test sentence, and lets the user pick
//! how agent events should interrupt them. Everything else stays in Settings.

use crate::settings::AppConfig;
use crate::voice::config::VoiceConfig;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "onboarding";

const WIDTH: f64 = 780.0;
const HEIGHT: f64 = 610.0;

/// Opens the setup guide, creating the window the first time it is needed.
pub fn show(app: &AppHandle) -> Result<(), String> {
    crate::macos::set_background_mode(false);
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("onboarding/onboarding.html".into()),
    )
    .title("Welcome to Samlu")
    .inner_size(WIDTH, HEIGHT)
    .resizable(false)
    .maximizable(false)
    .center()
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Everything the setup guide polls for: it re-reads this each time the window
/// regains focus, because both permissions are granted outside the app.
#[tauri::command]
pub fn onboarding_state(
    config: tauri::State<'_, Arc<AppConfig>>,
    voice: tauri::State<'_, Arc<VoiceConfig>>,
) -> serde_json::Value {
    serde_json::json!({
        "microphone": crate::macos::microphone_access().as_str(),
        "accessibility": crate::voice::get_voice_accessibility_status(),
        "hasVoiceKey": voice.has_api_key("transcription"),
        "voiceHotkey": voice.hotkey(),
        "delivery": config.delivery(),
        "completed": config.onboarding_completed(),
    })
}

#[tauri::command]
pub fn onboarding_request_microphone() -> &'static str {
    let status = crate::macos::microphone_access();
    if status == crate::macos::MicrophoneAccess::NotDetermined {
        crate::macos::request_microphone_access();
    }
    status.as_str()
}

#[tauri::command]
pub fn onboarding_open_microphone_settings() -> Result<(), String> {
    crate::open_path("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
}

#[tauri::command]
pub fn onboarding_set_delivery(
    delivery: String,
    config: tauri::State<'_, Arc<AppConfig>>,
) -> Result<(), String> {
    config.set_delivery(&delivery)
}

/// Ends setup. `open_agents` sends the user straight to the adapter list,
/// otherwise Samlu retreats to the menu bar the way it will normally live.
#[tauri::command]
pub fn onboarding_finish(app: AppHandle, open_agents: bool) {
    finish(&app, open_agents);
}

#[tauri::command]
pub fn onboarding_show(app: AppHandle) -> Result<(), String> {
    show(&app)
}

pub(crate) fn finish(app: &AppHandle, open_agents: bool) {
    if let Some(config) = app.try_state::<Arc<AppConfig>>() {
        config.set_onboarding_completed(true);
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    if open_agents {
        crate::tray::show_main(app);
        let _ = app.emit_to("main", "settings://tab", "agents");
    } else if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
        crate::macos::set_background_mode(true);
    }
}

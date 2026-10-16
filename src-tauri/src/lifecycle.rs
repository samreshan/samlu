//! Settings-window lifecycle for Samlu's menu-bar-first runtime.
//!
//! The main webview is normally created from tauri.conf.json. It can still be
//! destroyed by macOS, however, so every caller that wants Settings uses this
//! module rather than assuming a cached window is alive and focusable.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const MAIN_LABEL: &str = "main";

/// Mirrors the "main" window in tauri.conf.json: a full-height sidebar under
/// the window controls, as in System Settings. Windows keeps its standard
/// title bar.
fn create_main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    let builder = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("index.html".into()))
        .title("Samlu")
        .inner_size(820.0, 640.0)
        .min_inner_size(720.0, 520.0)
        .resizable(true);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(20.0, 24.0));
    let window = builder
        .center()
        .build()
        .map_err(|error| format!("could not recreate Settings: {error}"))?;
    if let Some(config) = app.try_state::<std::sync::Arc<crate::settings::AppConfig>>() {
        crate::settings::apply_window_theme(app, &config.appearance());
    }
    Ok(window)
}

/// Makes Settings visible and key in one AppKit main-thread operation. This
/// avoids racing a separately-dispatched activation-policy change against the
/// framework's `show`/`focus` calls.
pub fn present_settings(app: &AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window(MAIN_LABEL) {
        Some(window) => window,
        None => {
            log::warn!("Settings window was missing; recreating it");
            create_main_window(app)?
        }
    };

    window
        .unminimize()
        .map_err(|error| format!("could not restore minimized Settings: {error}"))?;
    if !crate::platform::present_application_window(&window) {
        return Err("could not access the native Settings window".to_string());
    }
    Ok(())
}

/// Moves Samlu back to its normal menu-bar-only mode once no primary UI is
/// needed. This remains intentionally separate from hiding the window so the
/// close preference can choose to quit instead.
pub fn hide_to_background(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        if let Err(error) = window.hide() {
            log::warn!("could not hide Settings: {error}");
        }
    }
    // The setup guide is a regular window too; it keeps the Dock icon, and
    // finishing it comes back here.
    let guide_open = app
        .get_webview_window(crate::onboarding::LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    if !guide_open {
        crate::platform::set_background_mode(true);
    }
}

//! Settings-window lifecycle for Samlu's menu-bar-first runtime.
//!
//! The main webview is normally created from tauri.conf.json. It can still be
//! destroyed by macOS, however, so every caller that wants Settings uses this
//! module rather than assuming a cached window is alive and focusable.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const MAIN_LABEL: &str = "main";

fn create_main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("index.html".into()))
        .title("Samlu")
        .inner_size(820.0, 680.0)
        .min_inner_size(720.0, 540.0)
        .resizable(true)
        .center()
        .build()
        .map_err(|error| format!("could not recreate Settings: {error}"))
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
    if !crate::macos::present_application_window(&window) {
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
    crate::macos::set_background_mode(true);
}

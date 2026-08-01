//! Menu-bar integration. Samlu stays available for global shortcuts and
//! agent notifications when its settings window is closed.

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id("show", "Show Samlu").build(app)?;
    let setup_item = MenuItemBuilder::with_id("setup", "Setup guide…").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&setup_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("Samlu")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "setup" => {
                if let Err(error) = crate::onboarding::show(app) {
                    log::error!("failed to open the setup guide: {error}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Shows and focuses the settings window. Also reused by the launcher's
/// "Open Samlu settings" quick action.
pub(crate) fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        crate::macos::set_background_mode(false);
        let _ = win.show();
        let _ = win.set_focus();
    }
}

//! Menu-bar integration. Samlu stays available for global shortcuts and
//! agent notifications when its settings window is closed.

use std::sync::Arc;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::updater::UpdaterState;

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let settings_item = MenuItemBuilder::with_id("show", "Settings…").build(app)?;
    let update_item = MenuItemBuilder::with_id("update", "Check for Updates…").build(app)?;
    let setup_item = MenuItemBuilder::with_id("setup", "Setup Guide…").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Samlu").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&settings_item)
        .item(&update_item)
        .item(&setup_item)
        .separator()
        .item(&quit_item)
        .build()?;

    app.state::<Arc<UpdaterState>>()
        .set_menu_item(update_item.clone());

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("Samlu")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "update" => crate::updater::check_from_menu(app),
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
/// "Samlu Settings" command.
pub(crate) fn show_main(app: &AppHandle) {
    if let Err(error) = crate::lifecycle::present_settings(app) {
        log::error!("failed to present Settings: {error}");
    }
}

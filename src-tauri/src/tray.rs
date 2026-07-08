//! System tray: show the settings window, toggle the mascot, quit. Samlu is
//! meant to live in the tray/background — closing the main window hides it
//! rather than quitting (see lib.rs's CloseRequested handler).

use crate::pet;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id("show", "Show Samlu").build(app)?;
    let toggle_pet_item = MenuItemBuilder::with_id("toggle_pet", "Show/Hide Mascot").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&toggle_pet_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("Samlu – Agent Watch")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "toggle_pet" => toggle_pet(app),
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

fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn toggle_pet(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(pet::PET_LABEL) {
        let visible = win.is_visible().unwrap_or(false);
        if visible {
            let _ = pet::pet_close(app.clone());
        } else {
            let _ = pet::pet_open(app.clone(), None, None);
        }
    }
}

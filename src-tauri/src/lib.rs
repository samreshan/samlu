mod adapters;
mod events;
mod notify;
mod pet;
mod server;
mod setup;
mod state;
mod tray;

use state::AppState;
use std::sync::Arc;
use tauri::{Listener, Manager};

#[tauri::command]
fn get_app_status(state: tauri::State<'_, Arc<AppState>>) -> serde_json::Value {
    serde_json::json!({
        "port": state.port,
        "serverRunning": state.is_server_running(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin registered — it needs to intercept a
        // second launch before anything else (notably our own hook
        // listener, which must never have two copies bound to the same
        // fixed port) initializes.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Pre-create the mascot overlay window (hidden) on the main
            // thread — building it later from a command/other thread
            // deadlocks on Windows (see pet.rs's doc comment).
            if let Err(err) = pet::pet_init(app.handle()) {
                log::error!("failed to pre-create mascot overlay window: {err}");
            }
            // Samlu's whole point is an always-visible companion, so show it
            // by default rather than waiting for the user to dig it out of
            // a menu.
            if let Err(err) = pet::pet_open(app.handle().clone(), None, None) {
                log::error!("failed to show mascot overlay window: {err}");
            }

            let token = server::auth::generate_token();
            let app_state = Arc::new(AppState::new(token, server::port::DEFAULT_PORT));
            app.manage(app_state.clone());

            let registry = Arc::new(adapters::AdapterRegistry::new());
            match server::start(app.handle().clone(), app_state.clone(), registry) {
                Ok(port) => {
                    app_state.set_server_running(true);
                    log::info!("hook listener bound to 127.0.0.1:{port}");
                }
                Err(err) => {
                    log::error!("failed to start hook listener: {err}");
                }
            }

            tray::build(app)?;

            // Mascot click -> bring the settings window forward and clear
            // whatever event it was highlighting (the user has now seen it).
            let poke_state = app_state.clone();
            let poke_handle = app.handle().clone();
            app.handle().listen("pet://poke", move |_event| {
                poke_state.clear_highlight();
                if let Some(win) = poke_handle.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            pet::pet_open,
            pet::pet_close,
            pet::pet_signal,
            pet::pet_set_position,
            pet::pet_set_clickthrough,
            setup::get_hook_status,
            setup::preview_hook_merge,
            setup::apply_hook_merge,
            setup::get_hook_snippet,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    // Hide to tray instead of quitting — the mascot and the
                    // hook listener should keep running in the background.
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Samlu");
}

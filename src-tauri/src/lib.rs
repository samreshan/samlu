mod adapters;
mod events;
mod launcher;
mod lifecycle;
mod macos;
mod notify;
mod onboarding;
mod pet;
mod server;
mod settings;
mod setup;
mod state;
mod tray;
mod voice;

use state::AppState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

/// Some macOS/Tauri runtimes request process exit as the last visible normal
/// window closes. A background close is deliberate, so suppress only that
/// immediate request; explicit Quit still carries an exit code and proceeds.
static BACKGROUND_CLOSE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Frees Samlu's own global shortcuts while the settings window is recording a
/// replacement. Without this, pressing a chord that is already bound fires its
/// action — starting dictation, opening the launcher — instead of reaching the
/// recorder that is waiting for it.
#[tauri::command]
fn suspend_global_shortcuts(app: tauri::AppHandle) {
    let _ = app.global_shortcut().unregister_all();
}

/// Rebinds everything from the saved configuration. Clears first so this is
/// idempotent: it may run after a hotkey command has already re-registered
/// part of the set, and binding the same chord twice would leave a stale
/// handler shadowing the new one.
#[tauri::command]
fn resume_global_shortcuts(app: tauri::AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    let launcher_hotkey = app
        .state::<Arc<launcher::config::LauncherConfig>>()
        .hotkey();
    if let Err(error) = launcher::register_hotkey(&app, &launcher_hotkey) {
        log::error!("failed to restore launcher hotkey ({launcher_hotkey}): {error}");
    }
    let voice_hotkey = app.state::<Arc<voice::config::VoiceConfig>>().hotkey();
    if let Err(error) = voice::register_hotkeys(&app, &voice_hotkey) {
        log::error!("failed to restore voice hotkeys ({voice_hotkey}): {error}");
    }
}

#[tauri::command]
fn get_app_status(state: tauri::State<'_, Arc<AppState>>) -> serde_json::Value {
    serde_json::json!({
        "port": state.port,
        "serverRunning": state.is_server_running(),
    })
}

#[tauri::command]
fn get_event_history(state: tauri::State<'_, Arc<AppState>>) -> Vec<events::EventHistoryItem> {
    state
        .event_history()
        .iter()
        .map(events::EventHistoryItem::from)
        .collect()
}

#[tauri::command]
fn clear_event_history(state: tauri::State<'_, Arc<AppState>>) {
    state.clear_event_history();
}

pub(crate) fn open_path(path: &str) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn run() {
    let initial_args: Vec<String> = std::env::args().collect();
    let initial_codex_payload =
        adapters::codex::payload_from_args(&initial_args).map(str::to_string);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(payload) = adapters::codex::payload_from_args(&argv) {
                match adapters::codex::parse_cli_payload(payload) {
                    Ok(event) => {
                        let state = app.state::<Arc<AppState>>().inner().clone();
                        notify::handle_event(app.clone(), state, event);
                    }
                    Err(error) => log::warn!("ignored Codex notification: {error}"),
                }
            } else {
                tray::show_main(app);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(if cfg!(debug_assertions) {
                        log::LevelFilter::Debug
                    } else {
                        log::LevelFilter::Info
                    })
                    .build(),
            )?;

            let app_data_dir = app.path().app_data_dir()?;
            let app_config = Arc::new(settings::AppConfig::load(&app_data_dir));
            let token = app_config.auth_token();
            let needs_onboarding = !app_config.onboarding_completed();
            app.manage(app_config);

            let app_state = Arc::new(AppState::new(
                token,
                server::port::DEFAULT_PORT,
                &app_data_dir,
            ));
            app.manage(app_state.clone());
            app.manage(Arc::new(pet::PetState::new()));
            app.manage(Arc::new(notify::IslandState::new()));

            app.manage(Arc::new(launcher::history::LauncherHistory::load(
                &app_data_dir,
            )));
            let launcher_config = Arc::new(launcher::config::LauncherConfig::load(&app_data_dir));
            let launcher_hotkey = launcher_config.hotkey();
            let project_index = Arc::new(launcher::projects::ProjectIndex::new(
                &launcher_config.project_roots(),
                &launcher_config.excluded_project_paths(),
            ));
            app.manage(launcher_config);
            app.manage(Arc::new(launcher::app_index::AppIndex::new()));
            app.manage(project_index);
            app.manage(Arc::new(launcher::snippets::SnippetStore::load(
                &app_data_dir,
            )));
            let clipboard_history =
                Arc::new(launcher::clipboard::ClipboardHistory::load(&app_data_dir));
            app.manage(clipboard_history.clone());
            launcher::clipboard::start_monitor(app.handle().clone(), clipboard_history);

            if let Err(error) = launcher::launcher_init(app.handle()) {
                log::error!("failed to create launcher window: {error}");
            }
            if let Err(error) = launcher::register_hotkey(app.handle(), &launcher_hotkey) {
                log::error!("failed to register launcher hotkey ({launcher_hotkey}): {error}");
            }

            let voice_config = Arc::new(voice::config::VoiceConfig::load(&app_data_dir));
            let voice_hotkey = voice_config.hotkey();
            app.manage(voice_config);
            app.manage(Arc::new(voice::VoiceState::new()));
            if let Err(error) = voice::voice_preview_init(app.handle()) {
                log::error!("failed to create voice preview window: {error}");
            }
            if let Err(error) = voice::register_hotkeys(app.handle(), &voice_hotkey) {
                log::error!("failed to register voice hotkeys ({voice_hotkey}): {error}");
            }
            if let Err(error) = notify::island_init(app.handle()) {
                log::error!("failed to create Samlu Island: {error}");
            }
            if let Err(error) = pet::pet_init(app.handle()) {
                log::error!("failed to create the pet overlay: {error}");
            }

            let registry = Arc::new(adapters::AdapterRegistry::new());
            match server::start(app.handle().clone(), app_state.clone(), registry) {
                Ok(port) => {
                    app_state.set_server_running(true);
                    log::info!("hook listener bound to 127.0.0.1:{port}");
                }
                Err(error) => log::error!("failed to start hook listener: {error}"),
            }

            tray::build(app)?;

            // First launch opens the setup guide instead of the settings window.
            if needs_onboarding && initial_codex_payload.is_none() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
                if let Err(error) = onboarding::show(app.handle()) {
                    log::error!("failed to open the setup guide: {error}");
                    tray::show_main(app.handle());
                }
            }

            if let Some(payload) = initial_codex_payload.as_deref() {
                match adapters::codex::parse_cli_payload(payload) {
                    Ok(event) => {
                        notify::handle_event(app.handle().clone(), app_state.clone(), event)
                    }
                    Err(error) => log::warn!("ignored Codex notification: {error}"),
                }
                lifecycle::hide_to_background(app.handle());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            get_event_history,
            clear_event_history,
            settings::get_notification_preferences,
            settings::set_notification_preferences,
            settings::get_appearance,
            settings::set_appearance,
            onboarding::onboarding_state,
            onboarding::onboarding_request_microphone,
            onboarding::onboarding_open_microphone_settings,
            onboarding::onboarding_set_delivery,
            onboarding::onboarding_finish,
            onboarding::onboarding_show,
            pet::pet_set_interactive,
            pet::pet_ready,
            pet::pet_dismiss,
            pet::pet_open_activity,
            notify::agent_island_dismiss,
            notify::agent_island_ready,
            notify::agent_island_open_activity,
            notify::test_agent_notification,
            suspend_global_shortcuts,
            resume_global_shortcuts,
            launcher::launcher_toggle,
            launcher::launcher_hide,
            launcher::launcher_resize,
            launcher::launcher_search,
            launcher::launcher_search_files,
            launcher::launcher_activate,
            launcher::get_launcher_hotkey,
            launcher::set_launcher_hotkey,
            launcher::get_project_settings,
            launcher::choose_project_folder,
            launcher::set_project_settings,
            launcher::reindex_projects,
            launcher::launcher_app_icon,
            launcher::snippets::get_snippets,
            launcher::snippets::save_snippet,
            launcher::snippets::delete_snippet,
            launcher::clipboard::clear_clipboard_history,
            voice::get_voice_settings,
            voice::set_voice_api_key,
            voice::set_voice_stt_model,
            voice::set_voice_transform_model,
            voice::set_voice_hotkey,
            voice::set_voice_endpoint,
            voice::set_voice_separate_providers,
            voice::set_voice_delivery,
            voice::set_voice_interface_sounds,
            voice::set_voice_pet_capsule,
            voice::get_voice_microphone_status,
            voice::request_voice_microphone,
            voice::open_voice_microphone_settings,
            voice::get_voice_accessibility_status,
            voice::request_voice_accessibility,
            voice::open_voice_accessibility_settings,
            voice::voice_preview_accept,
            voice::voice_preview_cancel,
            voice::voice_preview_ready,
            voice::voice_preview_drag,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_recovery_retry,
            voice::voice_recovery_copy,
            voice::voice_recovery_preview,
            setup::get_hook_status,
            setup::preview_hook_merge,
            setup::apply_hook_merge,
            setup::get_hook_snippet,
            setup::codex_notify::get_codex_status,
            setup::codex_notify::preview_codex_config,
            setup::codex_notify::apply_codex_config,
            setup::integrations::get_agent_integrations,
            setup::integrations::preview_agent_integration,
            setup::integrations::apply_agent_integration,
            setup::integrations::remove_agent_integration,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "main" => {
                        api.prevent_close();
                        let close_behavior = window
                            .app_handle()
                            .state::<Arc<settings::AppConfig>>()
                            .close_behavior();
                        if close_behavior == "quit" {
                            window.app_handle().exit(0);
                        } else {
                            let generation =
                                BACKGROUND_CLOSE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
                            lifecycle::hide_to_background(window.app_handle());
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_secs(2));
                                let _ = BACKGROUND_CLOSE_GENERATION.compare_exchange(
                                    generation,
                                    0,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                );
                            });
                        }
                    }
                    // Dismissing the setup guide counts as finishing it, so it
                    // does not reappear on every launch. The tray reopens it.
                    onboarding::LABEL => {
                        onboarding::finish(window.app_handle(), false);
                        api.prevent_close();
                    }
                    _ => {}
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running Samlu");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => tray::show_main(app),
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } if BACKGROUND_CLOSE_GENERATION.swap(0, Ordering::AcqRel) != 0 => {
            api.prevent_exit();
        }
        _ => {}
    });
}

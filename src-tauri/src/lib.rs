// Linux builds exist only to run the portable code's tests; native features
// are stand-ins there, which leaves code that only macOS and Windows reach.
#![cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]

mod adapters;
mod agents;
mod events;
mod launcher;
mod lifecycle;
mod modifier_shortcuts;
mod notify;
mod onboarding;
mod project_actions;
mod server;
mod settings;
mod setup;
mod state;
mod tray;
mod updater;
mod voice;

// Native behaviour each platform supplies behind the same functions: AppKit on
// macOS, Win32 on Windows, and inert stand-ins anywhere else.
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows/mod.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", windows)))]
#[path = "other_platform.rs"]
mod platform;

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
    modifier_shortcuts::unbind_launcher();
    modifier_shortcuts::unbind_voice();
}

/// Rebinds everything from the saved configuration. Clears first so this is
/// idempotent: it may run after a hotkey command has already re-registered
/// part of the set, and binding the same chord twice would leave a stale
/// handler shadowing the new one.
#[tauri::command]
fn resume_global_shortcuts(app: tauri::AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    modifier_shortcuts::unbind_launcher();
    modifier_shortcuts::unbind_voice();
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

/// Asks before something that cannot be undone, as a native alert sheet on
/// the window that asked. The dialog plugin replaces `window.confirm` with an
/// async function, so a plain `if (!confirm(...))` in a page sees a promise,
/// treats it as yes, and never waits for the answer; pages ask through here.
#[tauri::command]
async fn confirm_action(
    window: tauri::WebviewWindow,
    title: String,
    message: String,
    confirm: String,
) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let (answer, answered) = std::sync::mpsc::channel();
    window
        .dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            confirm,
            "Cancel".to_string(),
        ))
        .parent(&window)
        .show(move |confirmed| {
            let _ = answer.send(confirmed);
        });
    tauri::async_runtime::spawn_blocking(move || answered.recv().unwrap_or(false))
        .await
        .unwrap_or(false)
}

/// Opens one of Samlu's own pages in the default browser. The UI can only
/// name these fixed addresses, never pass a URL of its own.
#[tauri::command]
fn open_samlu_link(link: String) -> Result<(), String> {
    let url = match link.as_str() {
        "releases" => "https://github.com/samreshan/samlu/releases",
        "repository" => "https://github.com/samreshan/samlu",
        "issues" => "https://github.com/samreshan/samlu/issues",
        _ => return Err(format!("unknown link: {link}")),
    };
    open_path(url)
}

pub(crate) fn open_path(path: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        // Explorer hands the file to its default app with no shell in
        // between, so nothing in a file name is ever run as a command. The
        // path is always quoted (Windows paths can't contain quotes).
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        command.raw_arg(format!("\"{path}\""));
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(path);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Shows a file selected in Finder, or in Explorer on Windows.
pub(crate) fn reveal_path(path: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        command.raw_arg(format!("/select,\"{path}\""));
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(path);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn run() {
    let initial_args: Vec<String> = std::env::args().collect();
    let initial_codex_payload =
        adapters::codex::payload_from_args(&initial_args).map(str::to_string);

    #[allow(unused_mut)]
    let mut context = tauri::generate_context!();
    // On macOS Samlu starts in the background like other menu bar utilities:
    // Settings is created hidden and opens from the menu bar icon, the
    // launcher, or by opening Samlu again. Windows still shows it at launch.
    #[cfg(target_os = "macos")]
    for window in &mut context.config_mut().app.windows {
        if window.label == lifecycle::MAIN_LABEL {
            window.visible = false;
        }
    }

    #[allow(unused_mut)]
    let mut app = tauri::Builder::default()
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            log::info!("Samlu {} starting", app.package_info().version);
            #[cfg(windows)]
            platform::init();

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
            app.manage(Arc::new(agents::hub::AgentHub::new(&app_data_dir)));
            app.manage(Arc::new(notify::IslandState::new()));
            app.manage(Arc::new(updater::UpdaterState::new()));

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
            let deep_find = launcher::deep_find::DeepFind::new(&app_data_dir);
            deep_find.start(app.handle().clone());
            app.manage(deep_find);
            app.manage(project_index);
            app.manage(Arc::new(launcher::snippets::SnippetStore::load(
                &app_data_dir,
            )));
            let clipboard_history =
                Arc::new(launcher::clipboard::ClipboardHistory::load(&app_data_dir));
            app.manage(clipboard_history.clone());
            launcher::clipboard::start_monitor(app.handle().clone(), clipboard_history);

            // Resolves the login shell and loads project actions off the main
            // thread, so the first launcher action never waits for them.
            let project_actions = project_actions::ProjectActions::new(app.handle(), &app_data_dir);
            project_actions.warm();
            app.manage(project_actions);

            if let Err(error) = launcher::launcher_init(app.handle()) {
                log::error!("failed to create launcher window: {error}");
            }
            if let Err(error) = launcher::register_hotkey(app.handle(), &launcher_hotkey) {
                log::error!("failed to register launcher hotkey ({launcher_hotkey}): {error}");
            }

            let voice_config = Arc::new(voice::config::VoiceConfig::load(&app_data_dir));
            let voice_hotkey = voice_config.hotkey();
            app.manage(voice_config);
            app.manage(Arc::new(voice::history::VoiceHistory::load(&app_data_dir)));
            app.manage(Arc::new(voice::VoiceState::new()));
            app.manage(Arc::new(voice::Downloads::new()));
            voice::engines_init();
            if let Err(error) = voice::voice_preview_init(app.handle()) {
                log::error!("failed to create voice preview window: {error}");
            }
            if let Err(error) = voice::register_hotkeys(app.handle(), &voice_hotkey) {
                log::error!("failed to register voice hotkeys ({voice_hotkey}): {error}");
            }
            if let Err(error) = notify::island_init(app.handle()) {
                log::error!("failed to create Samlu Island: {error}");
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
            updater::start(app.handle());
            settings::apply_window_theme(
                app.handle(),
                &app.state::<Arc<settings::AppConfig>>().appearance(),
            );

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
            confirm_action,
            open_samlu_link,
            get_event_history,
            clear_event_history,
            settings::get_notification_preferences,
            settings::set_notification_preferences,
            settings::get_agent_alert_settings,
            settings::set_agent_alert_settings,
            agents::hub::agent_card_action,
            agents::hub::agent_queue_step,
            agents::hub::agent_sessions,
            agents::hub::agent_session_view,
            agents::hub::agent_sound_catalog,
            agents::hub::agent_sound_preview,
            agents::hub::agent_sound_choose_custom,
            settings::get_appearance,
            settings::set_appearance,
            onboarding::onboarding_state,
            onboarding::onboarding_request_microphone,
            onboarding::onboarding_open_microphone_settings,
            onboarding::onboarding_set_delivery,
            onboarding::onboarding_finish,
            onboarding::onboarding_show,
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
            launcher::deep_find::launcher_find_in_files,
            launcher::deep_find::get_deep_find_settings,
            launcher::deep_find::set_deep_find_enabled,
            launcher::deep_find::set_deep_find_folders,
            launcher::deep_find::set_deep_find_model,
            launcher::deep_find::deep_find_rebuild,
            launcher::deep_find::deep_find_choose_folder,
            launcher::deep_find::deep_find_download_model,
            launcher::deep_find::deep_find_cancel_download,
            launcher::deep_find::deep_find_remove_data,
            launcher::launcher_activate,
            launcher::launcher_reveal,
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
            project_actions::launcher_project_bindings,
            project_actions::launcher_run_project_chord,
            project_actions::launcher_project_actions,
            project_actions::launcher_run_project_action,
            project_actions::launcher_run_script,
            project_actions::tasks_list,
            project_actions::task_detail,
            project_actions::task_stop,
            project_actions::task_rerun,
            project_actions::task_open_log,
            project_actions::tasks_clear_finished,
            project_actions::task_url,
            project_actions::get_project_actions,
            project_actions::save_project_action,
            project_actions::delete_project_action,
            project_actions::reorder_project_actions,
            project_actions::set_project_action_binding,
            project_actions::set_project_action_preferences,
            project_actions::reset_project_actions,
            project_actions::choose_application,
            updater::get_update_state,
            updater::set_update_preferences,
            updater::check_for_updates,
            updater::install_update,
            updater::restart_to_update,
            voice::get_voice_settings,
            voice::set_voice_api_key,
            voice::set_voice_stt,
            voice::set_voice_language,
            voice::set_voice_transformation,
            voice::set_voice_cleanup,
            voice::set_voice_vocabulary,
            voice::set_voice_transform_model,
            voice::set_voice_hotkey,
            voice::set_voice_delivery,
            voice::set_voice_interface_sounds,
            voice::voice_apple_install,
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
            voice::get_voice_history,
            voice::clear_voice_history,
            voice::set_voice_keep_history,
            voice::model_commands::get_voice_models,
            voice::model_commands::voice_download_model,
            voice::model_commands::voice_cancel_model_download,
            voice::model_commands::voice_add_model,
            voice::model_commands::voice_remove_model,
            voice::model_commands::voice_delete_model,
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
        .build(context)
        .expect("error while running Samlu");

    // No Dock icon until Settings opens (see lifecycle.rs). Applied as the app
    // finishes launching, before any window exists; LSUIElement in Info.plist
    // keeps the Dock tile away until then.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => tray::show_main(app),
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } if BACKGROUND_CLOSE_GENERATION.swap(0, Ordering::AcqRel) != 0 => {
            api.prevent_exit();
        }
        tauri::RunEvent::Exit => {
            voice::engines_shutdown();
            // Stops EmbeddingGemma's llama.cpp helper with Samlu.
            app.state::<Arc<launcher::deep_find::DeepFind>>().shutdown();
            // Dev servers and tunnels started as tasks go with Samlu.
            app.state::<Arc<project_actions::ProjectActions>>()
                .shutdown();
        }
        _ => {}
    });
}

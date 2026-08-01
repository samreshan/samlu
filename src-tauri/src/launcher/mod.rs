//! Developer launcher overlay, pre-created hidden on the main thread and
//! backed by macOS application metadata and Spotlight file search.

pub mod app_index;
pub mod calculator;
pub mod clipboard;
pub mod color_picker;
pub mod config;
pub mod history;
pub mod icons;
pub mod projects;
pub mod search;
pub mod snippets;

use app_index::AppIndex;
use clipboard::ClipboardHistory;
use config::LauncherConfig;
use history::LauncherHistory;
use projects::ProjectIndex;
use snippets::SnippetStore;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{
    utils::config::Color, AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt;

use crate::settings::AppConfig;
use crate::state::AppState;

pub const LAUNCHER_LABEL: &str = "launcher";
pub const LAUNCHER_W: f64 = 560.0;
pub const LAUNCHER_H: f64 = 420.0;
/// Long enough for the panel's exit to finish. Keep in step with
/// --duration-exit in presence.css.
const LAUNCHER_EXIT_MS: u64 = 170;
static LAUNCHER_SHORTCUT_DOWN: AtomicBool = AtomicBool::new(false);
/// Guards a delayed hide against a launcher that has been reopened in the
/// meantime — double-tapping the hotkey must not hide the fresh window.
static LAUNCHER_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Height the panel settles at with an empty query. The launcher always opens
/// on an empty query, so presenting at this height means it never flashes at
/// full size before collapsing to fit.
static LAUNCHER_REST_H: AtomicU64 = AtomicU64::new(320);

/// Build the launcher overlay window, hidden, on the main thread.
pub fn launcher_init(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(LAUNCHER_LABEL).is_some() {
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(
        app,
        LAUNCHER_LABEL,
        WebviewUrl::App("launcher/launcher.html".into()),
    )
    .title("Samlu Launcher")
    .inner_size(LAUNCHER_W, LAUNCHER_H)
    .decorations(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .devtools(cfg!(debug_assertions))
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false);

    let window = builder.build().map_err(|e| e.to_string())?;
    crate::macos::configure_overlay(&window);
    Ok(())
}

/// Show (centered on whichever display the cursor is on) if hidden, hide if
/// visible — the global hotkey's toggle behavior.
#[tauri::command]
pub fn launcher_toggle(app: AppHandle) -> Result<(), String> {
    if app.get_webview_window(LAUNCHER_LABEL).is_none() {
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = launcher_init(&handle);
        })
        .map_err(|e| e.to_string())?;
    }

    let Some(win) = app.get_webview_window(LAUNCHER_LABEL) else {
        return Ok(());
    };

    if win.is_visible().unwrap_or(false) {
        hide_launcher(&app);
    } else {
        // Cancels any exit still in flight from a very recent dismissal.
        LAUNCHER_GENERATION.fetch_add(1, Ordering::SeqCst);
        let _ = app.emit_to(LAUNCHER_LABEL, "launcher://present", ());
        let rest = LAUNCHER_REST_H.load(Ordering::Relaxed) as f64;
        if !crate::macos::present_focusable_overlay_at_cursor(&win, LAUNCHER_W, rest, 0.22) {
            center_on_active_monitor(&win);
            win.show().map_err(|e| e.to_string())?;
            let _ = win.set_focus();
        }

        // Never put filesystem discovery on the global-hotkey path. Searches
        // can immediately use the startup index while a fresh snapshot is
        // prepared for the next keystroke/open.
        let app_index = app.state::<Arc<AppIndex>>().inner().clone();
        std::thread::spawn(move || app_index.rebuild());
    }
    Ok(())
}

/// Hide the launcher — called on Escape / blur / after activating a result.
#[tauri::command]
pub fn launcher_hide(app: AppHandle) -> Result<(), String> {
    hide_launcher(&app);
    Ok(())
}

/// Lets the panel retrace its entrance before the window goes away. Hiding
/// first would make every dismissal a hard cut.
fn hide_launcher(app: &AppHandle) {
    let Some(win) = app.get_webview_window(LAUNCHER_LABEL) else {
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        return;
    }
    let generation = LAUNCHER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = app.emit_to(LAUNCHER_LABEL, "launcher://dismiss", ());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LAUNCHER_EXIT_MS));
        if LAUNCHER_GENERATION.load(Ordering::SeqCst) != generation {
            return; // reopened mid-exit
        }
        if let Some(win) = app.get_webview_window(LAUNCHER_LABEL) {
            let _ = win.hide();
        }
    });
}

/// Fits the window to the result list. The panel keeps its top edge so the
/// query field never moves while you type. `resting` marks the height of the
/// empty-query state, which is what the next presentation opens at.
#[tauri::command]
pub fn launcher_resize(app: AppHandle, height: f64, resting: bool) {
    let clamped = height.clamp(120.0, LAUNCHER_H);
    if resting {
        LAUNCHER_REST_H.store(clamped.round() as u64, Ordering::Relaxed);
    }
    let Some(win) = app.get_webview_window(LAUNCHER_LABEL) else {
        return;
    };
    crate::macos::resize_overlay_keeping_top(&win, clamped);
}

/// Fast path (apps + Samlu's own actions/recent projects) — called on every
/// keystroke, no subprocess involved.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn launcher_search(
    query: String,
    app_state: tauri::State<'_, Arc<AppState>>,
    history: tauri::State<'_, Arc<LauncherHistory>>,
    app_index: tauri::State<'_, Arc<AppIndex>>,
    project_index: tauri::State<'_, Arc<ProjectIndex>>,
    snippets: tauri::State<'_, Arc<SnippetStore>>,
    clipboard: tauri::State<'_, Arc<ClipboardHistory>>,
    config: tauri::State<'_, Arc<AppConfig>>,
) -> Vec<search::LauncherResult> {
    search::search_fast(
        &query,
        &app_state,
        &history,
        &app_index,
        &project_index,
        &snippets,
        &clipboard,
        config.preferences().clipboard_history_enabled,
    )
}

/// Slow path (files, via `mdfind`) — called separately and less often so its
/// per-invocation subprocess cost never blocks the fast path above.
#[tauri::command]
pub async fn launcher_search_files(app: AppHandle, query: String) -> Vec<search::LauncherResult> {
    let history = app.state::<Arc<LauncherHistory>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || search::search_slow(&query, &history))
        .await
        .unwrap_or_default()
}

/// Icon for an "app"-kind result, as a `data:image/png;base64,...` URL —
/// `None` if it can't be found/converted. Fetched lazily per-row by the
/// frontend (not as part of `launcher_search`) so a cache miss on one icon
/// never slows down the search results themselves.
#[tauri::command]
pub async fn launcher_app_icon(app: AppHandle, path: String) -> Option<String> {
    let cache_dir = app.path().app_data_dir().ok()?.join("icon-cache");
    tauri::async_runtime::spawn_blocking(move || icons::icon_data_url(&path, &cache_dir))
        .await
        .ok()
        .flatten()
}

/// Runs whatever `id`/`kind`/`target` describes (open an app/file/project, or
/// dispatch a quick action) and records it for future frecency ranking.
#[tauri::command]
pub fn launcher_activate(
    app: AppHandle,
    id: String,
    kind: String,
    target: String,
    history: tauri::State<'_, Arc<LauncherHistory>>,
) -> Result<(), String> {
    let result = match kind.as_str() {
        "app" | "file" | "project" => crate::open_path(&target),
        "snippet" | "clipboard" | "calculation" => app
            .clipboard()
            .write_text(target)
            .map_err(|error| error.to_string()),
        "action" => run_action(&app, &target),
        other => Err(format!("unknown launcher result kind: {other}")),
    };
    if result.is_ok() {
        history.record_selection(&id);
    }
    result
}

/// Binds `hotkey` (e.g. "Alt+Space", "Cmd+Shift+K") to toggle the launcher.
/// Called once at startup with the persisted value, and again whenever the
/// user changes it in settings.
pub fn register_hotkey(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    // A rebind can happen with the old chord still physically held down; the
    // release for it will never arrive at the new registration.
    LAUNCHER_SHORTCUT_DOWN.store(false, Ordering::Release);
    app.global_shortcut()
        .on_shortcut(hotkey, |app_handle, _shortcut, event| match event.state() {
            ShortcutState::Pressed if !LAUNCHER_SHORTCUT_DOWN.swap(true, Ordering::AcqRel) => {
                let _ = launcher_toggle(app_handle.clone());
            }
            ShortcutState::Released => {
                LAUNCHER_SHORTCUT_DOWN.store(false, Ordering::Release);
            }
            _ => {}
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_launcher_hotkey(config: tauri::State<'_, Arc<LauncherConfig>>) -> String {
    config.hotkey()
}

/// Re-binds the launcher's global hotkey. If the new binding is invalid or
/// already taken by another app, the old one is re-registered so the user
/// isn't left without a working hotkey at all.
#[tauri::command]
pub fn set_launcher_hotkey(
    app: AppHandle,
    hotkey: String,
    config: tauri::State<'_, Arc<LauncherConfig>>,
) -> Result<(), String> {
    let hotkey = hotkey.trim().to_string();
    if hotkey.is_empty() {
        return Err("Hotkey can't be empty.".to_string());
    }

    let previous = config.hotkey();
    LAUNCHER_SHORTCUT_DOWN.store(false, Ordering::Release);
    let _ = app.global_shortcut().unregister(previous.as_str());

    if let Err(err) = register_hotkey(&app, &hotkey) {
        let _ = register_hotkey(&app, &previous);
        return Err(err);
    }

    config.set_hotkey(hotkey);
    Ok(())
}

#[tauri::command]
pub fn get_project_settings(
    config: tauri::State<'_, Arc<LauncherConfig>>,
    index: tauri::State<'_, Arc<ProjectIndex>>,
) -> serde_json::Value {
    serde_json::json!({
        "roots": config.project_roots(),
        "excludedPaths": config.excluded_project_paths(),
        "projectCount": index.len(),
    })
}

#[tauri::command]
pub async fn choose_project_folder(prompt: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let prompt = if prompt.trim().is_empty() {
            "Choose a project folder".to_string()
        } else {
            prompt
        };
        let output = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "POSIX path of (choose folder with prompt (item 1 of argv))",
                "-e",
                "end run",
                "--",
                &prompt,
            ])
            .output()
            .map_err(|error| format!("could not open the macOS folder picker: {error}"))?;
        if output.status.success() {
            let raw_path = String::from_utf8_lossy(&output.stdout);
            let raw_path = raw_path.trim();
            let path = if raw_path == "/" {
                raw_path.to_string()
            } else {
                raw_path.trim_end_matches('/').to_string()
            };
            return Ok((!path.is_empty()).then_some(path));
        }
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("-128") || error.to_lowercase().contains("cancel") {
            Ok(None)
        } else {
            Err(format!("macOS folder picker failed: {}", error.trim()))
        }
    })
    .await
    .map_err(|error| format!("folder picker task failed: {error}"))?
}

#[tauri::command]
pub fn set_project_settings(
    roots: Vec<String>,
    excluded_paths: Vec<String>,
    config: tauri::State<'_, Arc<LauncherConfig>>,
    index: tauri::State<'_, Arc<ProjectIndex>>,
) -> Result<usize, String> {
    let roots = normalize_directories(roots)?;
    let excluded_paths = normalize_directories(excluded_paths)?;
    config.set_project_settings(roots.clone(), excluded_paths.clone());
    Ok(index.rebuild(&roots, &excluded_paths))
}

#[tauri::command]
pub fn reindex_projects(
    config: tauri::State<'_, Arc<LauncherConfig>>,
    index: tauri::State<'_, Arc<ProjectIndex>>,
) -> usize {
    index.rebuild(&config.project_roots(), &config.excluded_project_paths())
}

fn normalize_directories(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for value in paths {
        let value = value.trim();
        let value = if value == "/" {
            value
        } else {
            value.trim_end_matches('/')
        };
        if value.is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(value);
        if !path.is_absolute() || !path.is_dir() {
            return Err(format!("Project folder is unavailable: {value}"));
        }
        let path = path.canonicalize().unwrap_or(path);
        let path = path.to_string_lossy().to_string();
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn run_action(app: &AppHandle, action: &str) -> Result<(), String> {
    match action {
        "open_settings" => {
            crate::tray::show_main(app);
            Ok(())
        }
        "manage_snippets" => {
            crate::tray::show_main(app);
            let _ = app.emit_to("main", "settings://tab", "launcher");
            Ok(())
        }
        "open_terminal" => std::process::Command::new("/usr/bin/open")
            .args(["-a", "Terminal"])
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string()),
        "color_picker" => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || match color_picker::pick() {
                Ok(color) => {
                    let _ = app.clipboard().write_text(color.clone());
                    let _ = app
                        .notification()
                        .builder()
                        .title("Color copied")
                        .body(color)
                        .show();
                }
                Err(error) if error != "Color picking was cancelled." => {
                    log::warn!("color picker failed: {error}");
                }
                Err(_) => {}
            });
            Ok(())
        }
        other => Err(format!("unknown launcher action: {other}")),
    }
}

/// Centers the window on the monitor the cursor is currently on (falling
/// back to the primary monitor) — Spotlight-style launchers open wherever
/// you're actually looking, not wherever the window happened to last be.
fn center_on_active_monitor(win: &WebviewWindow) {
    let monitor = win
        .cursor_position()
        .ok()
        .and_then(|pos| win.monitor_from_point(pos.x, pos.y).ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else { return };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let pos = monitor.position();

    let window_w_physical = LAUNCHER_W * scale;
    let x = pos.x as f64 + (size.width as f64 - window_w_physical) / 2.0;
    let y = pos.y as f64 + size.height as f64 * 0.22;
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

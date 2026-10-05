//! Notification policy for normalized agent events.

use crate::events::{AgentEvent, AgentEventKind};
use crate::settings::AppConfig;
use crate::state::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{utils::config::Color, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;

const ISLAND_LABEL: &str = "agent-island";
const ISLAND_WIDTH: f64 = 420.0;
const ISLAND_HEIGHT: f64 = 138.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IslandPayload {
    kind: &'static str,
    agent: String,
    title: String,
    project: String,
    summary: String,
    persistent: bool,
    has_notch: bool,
}

pub struct IslandState {
    ready: AtomicBool,
    pending: Mutex<Option<IslandPayload>>,
}

impl IslandState {
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            pending: Mutex::new(None),
        }
    }

    fn present_or_queue(&self, app: &AppHandle, payload: IslandPayload) {
        if self.ready.load(Ordering::Acquire) {
            emit_island(app, payload);
        } else {
            *self.pending.lock().unwrap() = Some(payload);
        }
    }
}

pub fn island_init(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(ISLAND_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        ISLAND_LABEL,
        WebviewUrl::App("agent-island/island.html".into()),
    )
    .title("Samlu Agent Update")
    .inner_size(ISLAND_WIDTH, ISLAND_HEIGHT)
    .decorations(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .focused(false)
    .visible(false)
    .build()
    .map(|window| {
        crate::macos::configure_overlay(&window);
    })
    .map_err(|error| error.to_string())
}

pub fn handle_event(app: AppHandle, state: Arc<AppState>, event: AgentEvent) {
    let generation = state.bump_session_generation(&event.session_id);
    state.record_event(event.clone());
    let _ = app.emit_to("main", "agent://event", &event);

    if event.kind == AgentEventKind::TurnFinished {
        let debounce_secs = app.state::<Arc<AppConfig>>().preferences().debounce_secs;
        let app = app.clone();
        let state = state.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(debounce_secs));
            let preferences = app.state::<Arc<AppConfig>>().preferences();
            if state.session_generation(&event.session_id) == generation
                && preferences.notify_turn_finished
            {
                show(
                    &app,
                    &event,
                    preferences.include_agent_summary,
                    &preferences.delivery,
                );
            }
        });
        return;
    }

    let preferences = app.state::<Arc<AppConfig>>().preferences();
    let enabled = match event.kind {
        AgentEventKind::NeedsInput => preferences.notify_needs_input,
        AgentEventKind::Completed => preferences.notify_task_completed,
        AgentEventKind::TurnFinished => preferences.notify_turn_finished,
    };
    if enabled {
        show(
            &app,
            &event,
            preferences.include_agent_summary,
            &preferences.delivery,
        );
    }
}

fn show(app: &AppHandle, event: &AgentEvent, include_summary: bool, delivery: &str) {
    let project = event.project_label();
    let title = match event.kind {
        AgentEventKind::NeedsInput => format!("{} needs input", event.agent_label()),
        AgentEventKind::Completed => format!("{} completed a task", event.agent_label()),
        AgentEventKind::TurnFinished => format!("{} finished a turn", event.agent_label()),
    };
    let fallback = match event.kind {
        AgentEventKind::NeedsInput => format!("{project} is waiting for you."),
        AgentEventKind::Completed => format!("{project} completed a task."),
        AgentEventKind::TurnFinished => format!("{project} finished responding."),
    };
    let body = if include_summary {
        event
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(|summary| truncate(summary, 240))
            .unwrap_or(fallback)
    } else {
        fallback
    };

    if let Err(err) = app.notification().builder().title(title).body(&body).show() {
        log::error!("failed to show agent notification: {err}");
    }
    // Notification Center always keeps the record; delivery only decides which
    // extra on-screen presentation runs alongside it.
    match delivery {
        crate::settings::DELIVERY_ISLAND => show_island_update(app, event, body),
        _ => {}
    }
}

fn show_island_update(app: &AppHandle, event: &AgentEvent, summary: String) {
    if app.get_webview_window(ISLAND_LABEL).is_none() {
        if let Err(error) = island_init(app) {
            log::error!("failed to create Samlu Island: {error}");
            return;
        }
    }
    let Some(window) = app.get_webview_window(ISLAND_LABEL) else {
        return;
    };
    // Follows the display you are working on, not the one under the pointer.
    crate::macos::center_overlay_at_top(&window);
    let kind = match event.kind {
        AgentEventKind::NeedsInput => "needs_input",
        AgentEventKind::Completed => "completed",
        AgentEventKind::TurnFinished => "turn_finished",
    };
    let title = match event.kind {
        AgentEventKind::NeedsInput => "Waiting for you",
        AgentEventKind::Completed => "Task completed",
        AgentEventKind::TurnFinished => "Turn finished",
    };
    let payload = IslandPayload {
        kind,
        agent: event.agent_label(),
        title: title.to_string(),
        project: event.project_label(),
        summary,
        persistent: event.kind == AgentEventKind::NeedsInput,
        has_notch: crate::macos::active_display_has_notch(),
    };
    crate::macos::order_front_without_activating(&window);
    if let Some(state) = app.try_state::<Arc<IslandState>>() {
        state.present_or_queue(app, payload);
    } else {
        emit_island(app, payload);
    }
}

fn emit_island(app: &AppHandle, payload: IslandPayload) {
    if let Err(error) = app.emit_to(ISLAND_LABEL, "island://show", payload) {
        log::warn!("failed to present Samlu Island: {error}");
    }
}

#[tauri::command]
pub fn agent_island_dismiss(app: AppHandle) {
    if let Some(window) = app.get_webview_window(ISLAND_LABEL) {
        let _ = window.hide();
    }
}

#[tauri::command]
pub fn agent_island_ready(app: AppHandle, state: tauri::State<'_, Arc<IslandState>>) {
    state.ready.store(true, Ordering::Release);
    if let Some(payload) = state.pending.lock().unwrap().take() {
        emit_island(&app, payload);
    }
}

#[tauri::command]
pub fn agent_island_open_activity(app: AppHandle) {
    agent_island_dismiss(app.clone());
    crate::tray::show_main(&app);
    let _ = app.emit_to("main", "settings://tab", "overview");
}

#[tauri::command]
pub fn test_agent_notification(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) {
    handle_event(
        app,
        state.inner().clone(),
        AgentEvent {
            agent: "codex".to_string(),
            kind: AgentEventKind::Completed,
            project_path: "samlu-companion".to_string(),
            summary: Some(
                "Finished the background interaction pass and verified the macOS build."
                    .to_string(),
            ),
            session_id: "samlu-notification-preview".to_string(),
            timestamp: chrono::Utc::now(),
        },
    );
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", truncated.trim_end())
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("abcdef", 3), "abc...");
        assert_eq!(truncate("नमस्ते", 2), "नम...");
    }
}

//! Notification policy for normalized agent events.

use crate::agents::hub::AgentHub;
use crate::agents::island_queue::QueueSnapshot;
use crate::agents::sounds::SoundKind;
use crate::events::{AgentEvent, AgentEventKind};
use crate::settings::AppConfig;
use crate::state::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{utils::config::Color, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;

const ISLAND_LABEL: &str = "agent-island";
const ISLAND_WIDTH: f64 = 420.0;
/// Plain notices keep the original height; a queue of cards needs room for
/// the action line and its buttons.
const ISLAND_HEIGHT: f64 = 138.0;
const ISLAND_QUEUE_HEIGHT: f64 = 184.0;

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
    /// `bloom` | `buddy`, the presentation chosen in Settings.
    style: &'static str,
    /// The cards waiting for the user. Absent for a plain timed notice; an
    /// empty queue with kind `dismiss` tells the island to leave.
    queue: Option<QueueSnapshot>,
    /// Which presentation this is, counting up. The island hands back the
    /// last one it rendered when it asks to be hidden, so a hide that was
    /// decided before a newer payload arrived can be told apart and ignored.
    seq: u64,
}

pub struct IslandState {
    ready: AtomicBool,
    pending: Mutex<Option<IslandPayload>>,
    /// The window is at the queue height rather than the plain one.
    tall: AtomicBool,
    /// Sequence number of the latest payload sent to the island.
    seq: AtomicU64,
    /// Sequence number of the latest payload that is not a dismiss.
    content_seq: AtomicU64,
    /// Agent and session of the last plain notice, for the island's open button.
    last_notice: Mutex<Option<(String, String)>>,
}

impl IslandState {
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            pending: Mutex::new(None),
            tall: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            content_seq: AtomicU64::new(0),
            last_notice: Mutex::new(None),
        }
    }

    fn present_or_queue(&self, app: &AppHandle, mut payload: IslandPayload) {
        payload.seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
        if payload.kind != "dismiss" {
            self.content_seq.store(payload.seq, Ordering::Release);
        }
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
    let builder = WebviewWindowBuilder::new(
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
    .visible(false);
    crate::platform::create_overlay(|| builder.build())
        .map(|window| {
            crate::platform::configure_overlay(&window);
        })
        .map_err(|error| error.to_string())
}

/// Surfaces an event that history and sessions already know about. The hub
/// calls this after recording it, so nothing here touches either.
pub fn present_event(app: AppHandle, state: Arc<AppState>, event: AgentEvent) {
    let generation = state.bump_session_generation(&event.session_id);

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

    let hub = app.state::<Arc<AgentHub>>();
    hub.play(
        app,
        match event.kind {
            AgentEventKind::NeedsInput => SoundKind::NeedsInput,
            AgentEventKind::Completed => SoundKind::Completed,
            AgentEventKind::TurnFinished => SoundKind::TurnFinished,
        },
    );
    if let Err(err) = app.notification().builder().title(title).body(&body).show() {
        log::error!("failed to show agent notification: {err}");
    }
    // Notification Center always keeps the record; delivery only decides which
    // extra on-screen presentation runs alongside it.
    if delivery != crate::settings::DELIVERY_ISLAND {
        return;
    }
    if event.kind == AgentEventKind::NeedsInput {
        // Needs-input notices wait in the queue until dismissed.
        hub.push_needs_input(app, event, body);
    } else if hub.queue_is_empty() {
        // A completion never replaces cards that are waiting for an answer.
        show_island_update(app, event, body);
    }
}

/// The island window, created on first use.
fn ensure_island(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if app.get_webview_window(ISLAND_LABEL).is_none() {
        island_init(app)?;
    }
    app.get_webview_window(ISLAND_LABEL)
        .ok_or_else(|| "the island window is missing".to_string())
}

/// Switches between the plain and the queue height, only when it changes.
fn fit_height(app: &AppHandle, window: &tauri::WebviewWindow, tall: bool) {
    let Some(state) = app.try_state::<Arc<IslandState>>() else {
        return;
    };
    if state.tall.swap(tall, Ordering::AcqRel) != tall {
        let height = if tall {
            ISLAND_QUEUE_HEIGHT
        } else {
            ISLAND_HEIGHT
        };
        crate::platform::resize_overlay_keeping_top(window, height);
    }
}

/// Shows the card queue, or tells the island to leave when it is empty. An
/// `Err` means the island cannot be shown at all.
pub fn present_queue(app: &AppHandle, queue: QueueSnapshot) -> Result<(), String> {
    let style = app.state::<Arc<AppConfig>>().island_style();
    let Some(front) = queue.cards.get(queue.front) else {
        let showing = app
            .get_webview_window(ISLAND_LABEL)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        if showing {
            emit_payload(
                app,
                IslandPayload {
                    kind: "dismiss",
                    agent: String::new(),
                    title: String::new(),
                    project: String::new(),
                    summary: String::new(),
                    persistent: false,
                    has_notch: false,
                    style,
                    queue: Some(queue),
                    seq: 0,
                },
            );
        }
        return Ok(());
    };
    let window = ensure_island(app)?;
    fit_height(app, &window, true);
    crate::platform::center_overlay_at_top(&window);
    let payload = IslandPayload {
        kind: "needs_input",
        agent: front.agent_label.clone(),
        title: front.title.clone(),
        project: front.project.clone(),
        summary: front.detail.clone(),
        persistent: true,
        has_notch: crate::platform::active_display_has_notch(),
        style,
        queue: Some(queue),
        seq: 0,
    };
    // Stamp and send first: a hide that raced this payload is judged against
    // its sequence number before the window comes forward.
    emit_payload(app, payload);
    crate::platform::order_front_without_activating(&window);
    Ok(())
}

fn emit_payload(app: &AppHandle, payload: IslandPayload) {
    if let Some(state) = app.try_state::<Arc<IslandState>>() {
        state.present_or_queue(app, payload);
    } else {
        emit_island(app, payload);
    }
}

fn show_island_update(app: &AppHandle, event: &AgentEvent, summary: String) {
    let window = match ensure_island(app) {
        Ok(window) => window,
        Err(error) => {
            log::error!("failed to create Samlu Island: {error}");
            return;
        }
    };
    fit_height(app, &window, false);
    // Follows the display you are working on, not the one under the pointer.
    crate::platform::center_overlay_at_top(&window);
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
        has_notch: crate::platform::active_display_has_notch(),
        style: app.state::<Arc<AppConfig>>().island_style(),
        queue: None,
        seq: 0,
    };
    if let Some(state) = app.try_state::<Arc<IslandState>>() {
        *crate::agents::hub::lock(&state.last_notice) =
            Some((event.agent.clone(), event.session_id.clone()));
    }
    emit_payload(app, payload);
    crate::platform::order_front_without_activating(&window);
}

fn emit_island(app: &AppHandle, payload: IslandPayload) {
    if let Err(error) = app.emit_to(ISLAND_LABEL, "island://show", payload) {
        log::warn!("failed to present Samlu Island: {error}");
    }
}

/// Hides the window and nothing else: the island's own exit already ran, and
/// re-emitting here would bring it straight back. Cards still queued stay
/// queued; the hub only stops the chords answering them unseen.
///
/// `seq` is the last payload the island rendered. A newer content payload
/// has been sent since when it is behind, so this hide is stale and the
/// island stays. A newer dismiss payload does not make it stale.
#[tauri::command(async)]
pub fn agent_island_dismiss(app: AppHandle, seq: Option<u64>) {
    let hub = app.state::<Arc<AgentHub>>().inner().clone();
    // Under the hub's present lock, so no queue snapshot is being shown
    // between the check and the hide.
    let _guard = hub.present_guard();
    let latest = app
        .try_state::<Arc<IslandState>>()
        .map_or(0, |state| state.content_seq.load(Ordering::Acquire));
    if !dismiss_accepted(seq, latest) {
        return;
    }
    if let Some(window) = app.get_webview_window(ISLAND_LABEL) {
        let _ = window.hide();
    }
    hub.island_hidden(&app);
}

/// A hide is stale only when content newer than what the island rendered
/// has been sent. No sequence number (native callers) always hides.
fn dismiss_accepted(seq: Option<u64>, latest_content_seq: u64) -> bool {
    seq.is_none_or(|seq| seq >= latest_content_seq)
}

#[tauri::command]
pub fn agent_island_ready(app: AppHandle, state: tauri::State<'_, Arc<IslandState>>) {
    state.ready.store(true, Ordering::Release);
    if let Some(payload) = state.pending.lock().unwrap().take() {
        emit_island(&app, payload);
    }
}

/// The island's open button: goes to the session of the card in front, or of
/// the notice on screen, never to a Samlu page.
#[tauri::command(async)]
pub fn agent_island_open_activity(app: AppHandle, seq: Option<u64>) {
    agent_island_dismiss(app.clone(), seq);
    let target = app.state::<Arc<AgentHub>>().front_session().or_else(|| {
        app.try_state::<Arc<IslandState>>()
            .and_then(|state| crate::agents::hub::lock(&state.last_notice).clone())
    });
    if let Some((agent, session_id)) = target {
        crate::agents::hub::view_session(&app, &agent, &session_id);
    }
}

/// The session id of the preview notification. It is not a real Codex
/// session: it never counts as a Codex hook arriving, and it leaves no
/// session behind.
pub const PREVIEW_SESSION: &str = "samlu-notification-preview";

#[tauri::command(async)]
pub fn test_agent_notification(app: AppHandle) {
    let hub = app.state::<Arc<AgentHub>>();
    hub.handle(
        &app,
        crate::agents::HookMessage::Event(AgentEvent {
            agent: "codex".to_string(),
            kind: AgentEventKind::Completed,
            project_path: "samlu-companion".to_string(),
            summary: Some(
                "Finished the background interaction pass and verified the macOS build."
                    .to_string(),
            ),
            session_id: PREVIEW_SESSION.to_string(),
            timestamp: chrono::Utc::now(),
            origin: Default::default(),
            outcome: None,
        }),
    );
    hub.handle(
        &app,
        crate::agents::HookMessage::Signal(crate::agents::SessionSignal {
            agent: "codex".to_string(),
            session_id: PREVIEW_SESSION.to_string(),
            agent_id: None,
            project_path: String::new(),
            kind: crate::agents::SignalKind::Ended,
            origin: Default::default(),
        }),
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
    use super::{dismiss_accepted, truncate};

    #[test]
    fn a_hide_is_stale_only_behind_newer_content() {
        assert!(dismiss_accepted(Some(5), 5));
        assert!(
            dismiss_accepted(Some(6), 5),
            "a later dismiss payload is fine"
        );
        assert!(!dismiss_accepted(Some(4), 5));
        assert!(dismiss_accepted(None, 99));
    }

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("abcdef", 3), "abc...");
        assert_eq!(truncate("नमस्ते", 2), "नम...");
    }
}

//! Turns a normalized `AgentEvent` into a mascot animation + (sometimes) a
//! native OS notification. This is the ONLY place that knows about pet
//! animation names / phases and debounce timing — adapters never touch it
//! directly, they just produce `AgentEvent`s that land here via the local
//! HTTP server (see server/mod.rs).

use crate::events::{AgentEvent, AgentEventKind};
use crate::pet::{self, PetSignal};
use crate::state::AppState;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

pub fn handle_event(app: AppHandle, state: Arc<AppState>, event: AgentEvent) {
    let generation = state.bump_session_generation(&event.session_id);
    state.record_last_event(format!(
        "[{}] {:?} — {}",
        event.agent,
        event.kind,
        event.summary.as_deref().unwrap_or(&project_label(&event.project_path))
    ));

    match event.kind {
        AgentEventKind::NeedsInput => surface(&app, &state, &event, "agentNeedsInput", "attention"),
        AgentEventKind::Completed => surface(&app, &state, &event, "agentCompleted", "success"),
        AgentEventKind::TurnFinished => {
            let app = app.clone();
            let state = state.clone();
            let event = event.clone();
            let debounce_secs = state.turn_finished_debounce_secs();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(debounce_secs));
                // Only surface if nothing newer arrived for this session
                // while we were sleeping (see AppState::bump_session_generation).
                if state.session_generation(&event.session_id) == generation {
                    surface_quiet(&app, &state, &event);
                }
            });
        }
    }
}

fn project_label(path: &str) -> String {
    if path.is_empty() {
        return "a project".to_string();
    }
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// NeedsInput / Completed: always fires an OS notification, and takes over
/// the mascot's one visible slot unless a higher-priority event from another
/// session already owns it (in which case the mascot stays as-is, but the OS
/// notification still fires independently — the notification center can show
/// more than one at once, the mascot's bubble can't).
fn surface(app: &AppHandle, state: &Arc<AppState>, event: &AgentEvent, event_name: &str, phase: &str) {
    let project = project_label(&event.project_path);

    if state.should_take_highlight(&event.session_id, event.kind) {
        state.set_highlight(&event.session_id, event.kind);
        let signal = PetSignal {
            event: Some(event_name.to_string()),
            phase: Some(phase.to_string()),
            source: Some(project),
            detail: event.summary.clone(),
            // `notify: None` — pet_signal's implicit mapping already covers
            // "agentNeedsInput"/"agentCompleted" with sensible copy.
            ..Default::default()
        };
        let _ = pet::pet_signal(app.clone(), signal);
    } else {
        let (title, body) = match event.kind {
            AgentEventKind::NeedsInput => (
                "Needs your input".to_string(),
                event
                    .summary
                    .clone()
                    .unwrap_or_else(|| format!("{project} is waiting on you.")),
            ),
            _ => (
                "Task complete".to_string(),
                event
                    .summary
                    .clone()
                    .unwrap_or_else(|| format!("{project} finished a task.")),
            ),
        };
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

/// TurnFinished, once the debounce window elapses with nothing newer for
/// this session: a quiet mascot nudge only, no OS notification — this is the
/// noisy "agent finished an ordinary turn" signal, not a real "come look" one.
fn surface_quiet(app: &AppHandle, state: &Arc<AppState>, event: &AgentEvent) {
    if !state.should_take_highlight(&event.session_id, event.kind) {
        return;
    }
    state.set_highlight(&event.session_id, event.kind);
    let signal = PetSignal {
        event: Some("agentTurnFinished".to_string()),
        phase: Some("idle".to_string()),
        source: Some(project_label(&event.project_path)),
        detail: event.summary.clone(),
        ..Default::default()
    };
    let _ = pet::pet_signal(app.clone(), signal);
}

//! The pet overlay — Samlu itself flying across the screen.
//!
//! One transparent, click-through window is stretched over whichever display
//! the cursor is on. It serves two very different animations:
//!
//! * `deliver` — an agent event arrives, the bee flies in from the screen edge,
//!   settles next to the pointer, and drops a card there.
//! * `carry` — a dictation result is about to be inserted, so the bee ferries it
//!   from the recording capsule to the pointer before the paste happens.
//!
//! The window only stops being click-through while a card is actually on
//! screen, so the bee never swallows a click mid-flight.

use crate::events::{AgentEvent, AgentEventKind};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    utils::config::Color, AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

pub const PET_LABEL: &str = "pet";

/// How long the bee spends ferrying a transcript. Deliberately short: this sits
/// between releasing the hotkey and the text appearing, so it is a latency cost
/// on every single dictation.
pub const CARRY_DURATION_MS: u64 = 360;

/// Where the voice capsule sits, mirrored from `voice::BOTTOM_MARGIN` so the
/// bee launches from the capsule rather than from empty screen.
const CAPSULE_BOTTOM_MARGIN: f64 = 28.0;
const CAPSULE_HALF_HEIGHT: f64 = 52.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShowPayload {
    x: f64,
    y: f64,
    kind: &'static str,
    title: String,
    agent: String,
    project: String,
    summary: String,
    action: &'static str,
    persistent: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CarryPayload {
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    duration: u64,
}

#[derive(Clone)]
enum PendingPresentation {
    Delivery(ShowPayload),
    Carry(CarryPayload),
}

/// Hidden webviews load asynchronously. This small handshake prevents a cold
/// launch notification from being emitted before pet.js has attached its
/// listeners.
pub struct PetState {
    ready: AtomicBool,
    generation: AtomicU64,
    pending: Mutex<Option<PendingPresentation>>,
}

impl PetState {
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            pending: Mutex::new(None),
        }
    }

    fn begin_presentation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn present_or_queue(&self, app: &AppHandle, presentation: PendingPresentation) {
        if self.ready.load(Ordering::Acquire) {
            emit_presentation(app, presentation);
        } else {
            *self.pending.lock().unwrap() = Some(presentation);
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct Stage {
    window: WebviewWindow,
    /// Cursor position relative to the window's own top-left, in logical px.
    cursor_x: f64,
    cursor_y: f64,
    width: f64,
    height: f64,
}

pub fn pet_init(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PET_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, PET_LABEL, WebviewUrl::App("pet/pet.html".into()))
        .title("Samlu")
        .inner_size(600.0, 400.0)
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
            crate::macos::set_ignores_mouse_events(&window, true);
        })
        .map_err(|error| error.to_string())
}

/// Stretches the overlay across the display holding the cursor and reports
/// where the cursor landed inside it.
fn stage(app: &AppHandle) -> Option<Stage> {
    if app.get_webview_window(PET_LABEL).is_none() {
        if let Err(error) = pet_init(app) {
            log::error!("failed to create the pet overlay: {error}");
            return None;
        }
    }
    let window = app.get_webview_window(PET_LABEL)?;
    let cursor = window.cursor_position().ok()?;
    let monitor = window
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())?;

    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;

    let _ = window.set_position(PhysicalPosition::new(origin.x as f64, origin.y as f64));
    let _ = window.set_size(LogicalSize::new(width, height));

    Some(Stage {
        window,
        cursor_x: (cursor.x - origin.x as f64) / scale,
        cursor_y: (cursor.y - origin.y as f64) / scale,
        width,
        height,
    })
}

fn present(stage: &Stage) {
    crate::macos::order_front_without_activating(&stage.window);
}

/// An agent event delivered by the pet: fly in, settle, drop the card.
pub fn deliver(app: &AppHandle, event: &AgentEvent, summary: String) {
    let Some(stage) = stage(app) else {
        return;
    };
    let (kind, title, action) = match event.kind {
        AgentEventKind::NeedsInput => ("needs_input", "Waiting for you", "Approve"),
        AgentEventKind::Completed => ("completed", "Task completed", "View"),
        AgentEventKind::TurnFinished => ("turn_finished", "Turn finished", "Open"),
    };
    let persistent = event.kind == AgentEventKind::NeedsInput;
    let payload = ShowPayload {
        x: stage.cursor_x,
        y: stage.cursor_y,
        kind,
        title: title.to_string(),
        agent: event.agent_label(),
        project: event.project_label(),
        summary,
        action,
        persistent,
    };
    present(&stage);
    if let Some(state) = app.try_state::<Arc<PetState>>() {
        let state = state.inner().clone();
        let generation = state.begin_presentation();
        state.present_or_queue(app, PendingPresentation::Delivery(payload));
        if !persistent {
            schedule_failsafe_hide(app.clone(), state, generation, Duration::from_secs(12));
        }
    } else {
        let _ = app.emit_to(PET_LABEL, "pet://show", payload);
    }
}

/// The bee ferries a finished transcript from the capsule to the pointer.
/// Returns false when there is no display to fly across, so the caller can skip
/// waiting for an animation that will not happen.
pub fn carry(app: &AppHandle) -> bool {
    let Some(stage) = stage(app) else {
        return false;
    };
    let payload = CarryPayload {
        from_x: stage.width / 2.0,
        from_y: stage.height - CAPSULE_BOTTOM_MARGIN - CAPSULE_HALF_HEIGHT,
        to_x: stage.cursor_x,
        to_y: stage.cursor_y,
        duration: CARRY_DURATION_MS,
    };
    present(&stage);
    if let Some(state) = app.try_state::<Arc<PetState>>() {
        let state = state.inner().clone();
        let generation = state.begin_presentation();
        state.present_or_queue(app, PendingPresentation::Carry(payload));
        schedule_failsafe_hide(
            app.clone(),
            state,
            generation,
            Duration::from_millis(CARRY_DURATION_MS + 900),
        );
    } else {
        let _ = app.emit_to(PET_LABEL, "pet://carry", payload);
    }
    true
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PET_LABEL) {
        crate::macos::set_ignores_mouse_events(&window, true);
        let _ = window.hide();
    }
}

fn schedule_failsafe_hide(app: AppHandle, state: Arc<PetState>, generation: u64, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if state.generation.load(Ordering::Acquire) == generation {
            log::warn!("pet presentation exceeded its UI lifetime; hiding the native overlay");
            state.invalidate();
            hide(&app);
        }
    });
}

fn emit_presentation(app: &AppHandle, presentation: PendingPresentation) {
    let result = match presentation {
        PendingPresentation::Delivery(payload) => app.emit_to(PET_LABEL, "pet://show", payload),
        PendingPresentation::Carry(payload) => app.emit_to(PET_LABEL, "pet://carry", payload),
    };
    if let Err(error) = result {
        log::warn!("failed to present the pet overlay: {error}");
    }
}

/// Called after pet.js attaches both event listeners. At most the newest cold
/// launch presentation is retained because a newer agent event supersedes an
/// older card before either could have been seen.
#[tauri::command]
pub fn pet_ready(app: AppHandle, state: tauri::State<'_, Arc<PetState>>) {
    state.ready.store(true, Ordering::Release);
    if let Some(pending) = state.pending.lock().unwrap().take() {
        emit_presentation(&app, pending);
    }
}

/// Keep the display-sized flight surface click-through. Once the card lands,
/// the frontend supplies a tight card/bee rectangle and this command shrinks
/// the native window to that rectangle before enabling input. Transparent
/// pixels elsewhere on the display can therefore never swallow user clicks.
#[tauri::command]
pub fn pet_set_interactive(
    app: AppHandle,
    interactive: bool,
    bounds: Option<InteractiveBounds>,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window(PET_LABEL) else {
        return Ok(());
    };
    if interactive {
        let bounds = bounds.ok_or_else(|| "interactive pet bounds are required".to_string())?;
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return Err("interactive pet bounds must be positive".to_string());
        }
        let origin = window.outer_position().map_err(|error| error.to_string())?;
        let scale = window.scale_factor().map_err(|error| error.to_string())?;
        window
            .set_position(PhysicalPosition::new(
                origin.x as f64 + bounds.x * scale,
                origin.y as f64 + bounds.y * scale,
            ))
            .map_err(|error| error.to_string())?;
        window
            .set_size(LogicalSize::new(bounds.width, bounds.height))
            .map_err(|error| error.to_string())?;
    }
    crate::macos::set_ignores_mouse_events(&window, !interactive);
    Ok(())
}

#[tauri::command]
pub fn pet_dismiss(app: AppHandle) {
    if let Some(state) = app.try_state::<Arc<PetState>>() {
        state.invalidate();
    }
    hide(&app);
}

#[tauri::command]
pub fn pet_open_activity(app: AppHandle) {
    pet_dismiss(app.clone());
    crate::tray::show_main(&app);
    let _ = app.emit_to("main", "settings://tab", "overview");
}

//! Agent Pet — overlay window module for Tauri v2.
//!
//! Drop this file next to `lib.rs` as `src-tauri/src/pet.rs`, then in `lib.rs`:
//!
//! ```ignore
//! mod pet;
//!
//! tauri::Builder::default()
//!     .plugin(tauri_plugin_notification::init())
//!     .invoke_handler(tauri::generate_handler![
//!         pet::pet_open,
//!         pet::pet_close,
//!         pet::pet_signal,
//!         pet::pet_set_position,
//!         pet::pet_set_clickthrough,
//!     ])
//!     // ...
//! ```
//!
//! The whole feature is driven by ONE function: `pet_signal`. Call it directly
//! (a plain Rust function call, not IPC — `notify.rs` does this) whenever an
//! agent event arrives — it animates the mascot and can fire a native
//! notification. Adapted from the Kettles project (same author).

use serde::{Deserialize, Serialize};
use tauri::{
    utils::config::Color, AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri_plugin_notification::NotificationExt;

/// Window label of the pet overlay. Used by the capability file too.
pub const PET_LABEL: &str = "pet";

/// Overlay window size (logical px). Sized snug to the mascot + its thought
/// bubble (incl. the finished-state action panel) — nothing is clipped.
pub const PET_W: f64 = 260.0;
pub const PET_H: f64 = 320.0;

/// Optional native-notification payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyOpts {
    pub title: String,
    pub body: String,
}

/// A single instruction for the pet. Every field is optional so callers send
/// only what they have.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PetSignal {
    /// Direct animation state: idle | running | waving | jumping | failed | waiting | review.
    #[serde(default)]
    pub state: Option<String>,
    /// High-level event mapped by `pet.config.json` -> `events`:
    /// timerStart | timerResume | timerPause | timerBreak | timerFinish | timerAbandon.
    #[serde(default)]
    pub event: Option<String>,
    /// Coarse timer phase for card styling: idle | running | paused | finished.
    #[serde(default)]
    pub phase: Option<String>,
    /// Short label for the pet's speech bubble, e.g. the client name.
    #[serde(default)]
    pub source: Option<String>,
    /// Detail line, e.g. the task name or the countdown "24:59".
    #[serde(default)]
    pub detail: Option<String>,
    /// Force a desktop notification regardless of `event`.
    #[serde(default)]
    pub notify: Option<NotifyOpts>,
}

/// Build the pet overlay window (hidden). MUST run on the main thread — call
/// this from `setup()`. Creating a webview window from an IPC command thread
/// deadlocks `build()` on Windows, so the window is created once at startup and
/// only shown/hidden afterwards.
pub fn pet_init(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PET_LABEL).is_some() {
        return Ok(());
    }

    log::info!("pet_init: creating pet overlay window (hidden)");
    let builder =
        WebviewWindowBuilder::new(app, PET_LABEL, WebviewUrl::App("pet/pet.html".into()))
            .title("Agent Pet")
            .inner_size(PET_W, PET_H)
            .decorations(false)
            .transparent(true)
            .background_color(Color(0, 0, 0, 0))
            .devtools(cfg!(debug_assertions))
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .position(200.0, 200.0)
            .visible(false);

    builder.build().map_err(|e| {
        log::error!("pet_init: builder.build() FAILED: {e}");
        e.to_string()
    })?;
    log::info!("pet_init: pet overlay window created");
    Ok(())
}

/// Open (or reveal) the always-on-top pet overlay window.
/// Pass `x`/`y` (physical px) to place it, e.g. next to the mini-widget.
#[tauri::command]
pub fn pet_open(app: AppHandle, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    // The window is created once at startup by `pet_init` (on the main thread).
    // If it's missing for any reason, recreate it on the main thread to avoid
    // the Windows deadlock that happens when building from a command thread.
    if app.get_webview_window(PET_LABEL).is_none() {
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = pet_init(&handle);
        })
        .map_err(|e| e.to_string())?;
    }

    if let Some(win) = app.get_webview_window(PET_LABEL) {
        log::info!("showing pet overlay window");
        if let (Some(x), Some(y)) = (x, y) {
            let _ = win.set_position(PhysicalPosition::new(x, y));
        }
        win.show().map_err(|e| e.to_string())?;
        let _ = win.set_always_on_top(true);
    }
    Ok(())
}

/// Hide the pet overlay window. The window is kept alive (not destroyed) so it
/// can be re-shown instantly without rebuilding the webview — rebuilding from a
/// command thread deadlocks on Windows.
#[tauri::command]
pub fn pet_close(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        log::info!("hiding pet overlay window");
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The one call you need: animate the pet and (on attention-worthy events)
/// notify. Called directly by `notify.rs` (a plain Rust function call, not
/// IPC) whenever an agent event arrives.
///
/// Examples:
///   pet_signal({ event: "agentNeedsInput", phase: "attention", source: "my-project", detail: "Waiting for permission" })
///   pet_signal({ event: "agentCompleted",  phase: "success",   source: "my-project", detail: "Finished a task" })
///   pet_signal({ event: "agentTurnFinished" })   // quiet nudge, no OS notification
#[tauri::command]
pub fn pet_signal(app: AppHandle, signal: PetSignal) -> Result<(), String> {
    // 1. Drive the animation + bubble inside the pet window.
    app.emit_to(PET_LABEL, "pet://state", &signal)
        .map_err(|e| e.to_string())?;

    // 2. Native desktop notification: explicit, or implied by the event kind.
    // agentTurnFinished intentionally has no implicit notification — it's the
    // noisy "ordinary turn ended" signal, meant to only nudge the mascot.
    let notify = signal
        .notify
        .clone()
        .or_else(|| match signal.event.as_deref() {
            Some("agentNeedsInput") => Some(NotifyOpts {
                title: "Needs your input".into(),
                body: signal
                    .detail
                    .clone()
                    .unwrap_or_else(|| "An agent is waiting on you.".into()),
            }),
            Some("agentCompleted") => Some(NotifyOpts {
                title: "Task complete".into(),
                body: signal
                    .detail
                    .clone()
                    .unwrap_or_else(|| "Your agent finished.".into()),
            }),
            _ => None,
        });

    if let Some(n) = notify {
        app.notification()
            .builder()
            .title(&n.title)
            .body(&n.body)
            .show()
            .map_err(|e| e.to_string())?;

        // Re-assert the overlay on top so the user actually sees the reaction.
        if let Some(win) = app.get_webview_window(PET_LABEL) {
            let _ = win.show();
            let _ = win.set_always_on_top(true);
        }
    }

    Ok(())
}

/// Move the pet window (physical px). Handy to dock it beside the mini-widget.
#[tauri::command]
pub fn pet_set_position(app: AppHandle, x: f64, y: f64) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        win.set_position(PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Toggle click-through. `true` = mouse passes through the pet to windows
/// behind it (pet becomes purely decorative; click/hover/drag stop working).
#[tauri::command]
pub fn pet_set_clickthrough(app: AppHandle, enabled: bool) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        win.set_ignore_cursor_events(enabled)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

//! Shared app state: the local server's auth token/port, per-session
//! "generation" counters used to debounce noisy TurnFinished events, and
//! which event is currently occupying the mascot's one visible slot.

use crate::events::AgentEventKind;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct AppState {
    pub token: String,
    pub port: u16,
    /// Last event received, per session — surfaced in the Status tab so
    /// there's a way to tell the listener is actually working without
    /// needing to synthesize a fake hook call.
    last_event_summary: Mutex<Option<String>>,
    server_running: AtomicBool,
    sessions: Mutex<HashMap<String, u64>>,
    turn_finished_debounce_secs: Mutex<u64>,
    /// (session_id, kind) of whatever the mascot is currently displaying —
    /// used to arbitrate between concurrent sessions competing for the
    /// mascot's single visible slot (NeedsInput > Completed > TurnFinished).
    current_highlight: Mutex<Option<(String, AgentEventKind)>>,
}

impl AppState {
    pub fn new(token: String, port: u16) -> Self {
        Self {
            token,
            port,
            last_event_summary: Mutex::new(None),
            server_running: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
            turn_finished_debounce_secs: Mutex::new(20),
            current_highlight: Mutex::new(None),
        }
    }

    pub fn set_server_running(&self, running: bool) {
        self.server_running.store(running, Ordering::Relaxed);
    }

    pub fn is_server_running(&self) -> bool {
        self.server_running.load(Ordering::Relaxed)
    }

    pub fn record_last_event(&self, summary: String) {
        *self.last_event_summary.lock().unwrap() = Some(summary);
    }

    pub fn last_event_summary(&self) -> Option<String> {
        self.last_event_summary.lock().unwrap().clone()
    }

    /// Records a new event for this session and returns the resulting
    /// generation number. A debounced (TurnFinished) surface should only
    /// actually fire if the generation is unchanged when its timer elapses —
    /// otherwise a newer event for the same session has superseded it.
    pub fn bump_session_generation(&self, session_id: &str) -> u64 {
        let mut sessions = self.sessions.lock().unwrap();
        let generation = sessions.entry(session_id.to_string()).or_insert(0);
        *generation += 1;
        *generation
    }

    pub fn session_generation(&self, session_id: &str) -> u64 {
        let sessions = self.sessions.lock().unwrap();
        *sessions.get(session_id).unwrap_or(&0)
    }

    pub fn turn_finished_debounce_secs(&self) -> u64 {
        *self.turn_finished_debounce_secs.lock().unwrap()
    }

    pub fn set_turn_finished_debounce_secs(&self, secs: u64) {
        *self.turn_finished_debounce_secs.lock().unwrap() = secs;
    }

    /// True if `session_id`/`kind` should take over the mascot's visible
    /// slot right now, given whatever's currently showing.
    pub fn should_take_highlight(&self, session_id: &str, kind: AgentEventKind) -> bool {
        let current = self.current_highlight.lock().unwrap();
        match current.as_ref() {
            None => true,
            Some((current_session, current_kind)) => {
                current_session == session_id || kind.priority() >= current_kind.priority()
            }
        }
    }

    pub fn set_highlight(&self, session_id: &str, kind: AgentEventKind) {
        *self.current_highlight.lock().unwrap() = Some((session_id.to_string(), kind));
    }

    /// Called when the user clicks the mascot (acknowledges whatever it's
    /// showing) — frees the slot for the next event of any priority.
    pub fn clear_highlight(&self) {
        *self.current_highlight.lock().unwrap() = None;
    }
}

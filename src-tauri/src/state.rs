//! Shared runtime state for the local hook listener and recent agent events.

use crate::events::AgentEvent;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// How many recent events the Status tab's history list keeps.
const EVENT_HISTORY_CAP: usize = 20;

pub struct AppState {
    pub token: String,
    pub port: u16,
    history_path: PathBuf,
    event_history: Mutex<VecDeque<AgentEvent>>,
    server_running: AtomicBool,
    sessions: Mutex<HashMap<String, u64>>,
}

impl AppState {
    pub fn new(token: String, port: u16, app_data_dir: &Path) -> Self {
        let history_path = app_data_dir.join("event-history.json");
        let event_history = std::fs::read_to_string(&history_path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            token,
            port,
            history_path,
            event_history: Mutex::new(event_history),
            server_running: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_server_running(&self, running: bool) {
        self.server_running.store(running, Ordering::Relaxed);
    }

    pub fn is_server_running(&self) -> bool {
        self.server_running.load(Ordering::Relaxed)
    }

    pub fn record_event(&self, event: AgentEvent) {
        let mut history = self.event_history.lock().unwrap();
        history.push_front(event);
        while history.len() > EVENT_HISTORY_CAP {
            history.pop_back();
        }
        if let Some(parent) = self.history_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&*history) {
            let _ = std::fs::write(&self.history_path, json);
        }
    }

    /// Recent events, most-recent-first.
    pub fn event_history(&self) -> Vec<AgentEvent> {
        self.event_history.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear_event_history(&self) {
        self.event_history.lock().unwrap().clear();
        let _ = std::fs::write(&self.history_path, "[]");
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
}

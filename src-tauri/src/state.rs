//! Shared runtime state for the local hook listener and recent agent events.

use crate::events::AgentEvent;
use chrono::{DateTime, Utc};
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

    /// Sets how a held approval ended on the event stored for it. Returns the
    /// updated event, or `None` once it has left the history.
    pub fn set_event_outcome(
        &self,
        agent: &str,
        session_id: &str,
        timestamp: DateTime<Utc>,
        outcome: &str,
    ) -> Option<AgentEvent> {
        let mut history = self.event_history.lock().unwrap();
        let event = history.iter_mut().find(|event| {
            event.agent == agent && event.session_id == session_id && event.timestamp == timestamp
        })?;
        event.outcome = Some(outcome.to_string());
        let updated = event.clone();
        if let Ok(json) = serde_json::to_string_pretty(&*history) {
            let _ = std::fs::write(&self.history_path, json);
        }
        Some(updated)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AgentEventKind;

    fn event(session: &str) -> AgentEvent {
        AgentEvent {
            agent: "codex".to_string(),
            kind: AgentEventKind::NeedsInput,
            project_path: "/p".to_string(),
            summary: Some("ls".to_string()),
            session_id: session.to_string(),
            timestamp: Utc::now(),
            origin: Default::default(),
            outcome: None,
        }
    }

    #[test]
    fn outcome_lands_on_the_matching_event_and_persists() {
        let dir = std::env::temp_dir().join(format!("samlu-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState::new("t".to_string(), 1, &dir);
        let first = event("s1");
        let second = event("s2");
        state.record_event(first.clone());
        state.record_event(second.clone());

        let updated = state
            .set_event_outcome("codex", "s1", first.timestamp, "approved in Samlu")
            .unwrap();
        assert_eq!(updated.outcome.as_deref(), Some("approved in Samlu"));
        assert!(state
            .set_event_outcome("codex", "gone", first.timestamp, "x")
            .is_none());

        let reloaded = AppState::new("t".to_string(), 1, &dir);
        let history = reloaded.event_history();
        assert_eq!(history[0].outcome, None);
        assert_eq!(history[1].outcome.as_deref(), Some("approved in Samlu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_written_before_outcomes_still_loads() {
        let dir = std::env::temp_dir().join(format!("samlu-state-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("event-history.json"),
            r#"[{"agent":"codex","kind":"completed","project_path":"/p","summary":null,"session_id":"s","timestamp":"2026-10-20T09:00:00Z"}]"#,
        )
        .unwrap();
        let state = AppState::new("t".to_string(), 1, &dir);
        assert_eq!(state.event_history().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

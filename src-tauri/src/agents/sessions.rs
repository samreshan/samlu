//! Live agent sessions, in memory only.

use super::origin::SessionOrigin;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    Waiting,
    Idle,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent: String,
    pub agent_label: String,
    pub session_id: String,
    pub project_path: String,
    pub project_label: String,
    pub state: SessionState,
    pub origin: SessionOrigin,
    pub started_at: DateTime<Utc>,
    pub last_event_at: DateTime<Utc>,
}

pub struct Sessions {
    #[allow(dead_code)]
    sessions: Mutex<Vec<SessionInfo>>,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(Vec::new()),
        }
    }

    pub fn touch(
        &self,
        agent: &str,
        session_id: &str,
        project_path: &str,
        origin: &SessionOrigin,
        state: Option<SessionState>,
        now: DateTime<Utc>,
    ) -> bool {
        let _ = (agent, session_id, project_path, origin, state, now);
        false
    }

    pub fn end(&self, agent: &str, session_id: &str) -> bool {
        let _ = (agent, session_id);
        false
    }

    pub fn get(&self, agent: &str, session_id: &str) -> Option<SessionInfo> {
        let _ = (agent, session_id);
        None
    }

    /// Waiting first, then most recent event.
    pub fn list(&self) -> Vec<SessionInfo> {
        Vec::new()
    }

    pub fn waiting_count(&self) -> usize {
        0
    }

    /// Drops sessions silent for more than 12 hours.
    pub fn prune(&self, now: DateTime<Utc>) -> bool {
        let _ = now;
        false
    }
}

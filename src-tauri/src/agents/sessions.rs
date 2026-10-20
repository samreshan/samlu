//! Live agent sessions, in memory only.

use super::origin::SessionOrigin;
use super::{agent_label, project_label};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::sync::Mutex;

/// Sessions silent for longer than this are dropped by `prune`.
const STALE_AFTER: Duration = Duration::hours(12);

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

    /// Registers the session on first sight and refreshes it on every later
    /// event. `None` keeps the current state (or starts Idle for a new session).
    /// Returns true: every touch changes the registry (last event time).
    pub fn touch(
        &self,
        agent: &str,
        session_id: &str,
        project_path: &str,
        origin: &SessionOrigin,
        state: Option<SessionState>,
        now: DateTime<Utc>,
    ) -> bool {
        let mut sessions = self.lock();
        match sessions
            .iter_mut()
            .find(|s| s.agent == agent && s.session_id == session_id)
        {
            Some(session) => {
                if !project_path.is_empty() && session.project_path != project_path {
                    session.project_path = project_path.to_string();
                    session.project_label = project_label(project_path);
                }
                merge_origin(&mut session.origin, origin);
                if let Some(state) = state {
                    session.state = state;
                }
                session.last_event_at = now;
            }
            None => sessions.push(SessionInfo {
                agent: agent.to_string(),
                agent_label: agent_label(agent),
                session_id: session_id.to_string(),
                project_path: project_path.to_string(),
                project_label: project_label(project_path),
                state: state.unwrap_or(SessionState::Idle),
                origin: origin.clone(),
                started_at: now,
                last_event_at: now,
            }),
        }
        true
    }

    /// Removes the session. Returns true if it was registered.
    pub fn end(&self, agent: &str, session_id: &str) -> bool {
        let mut sessions = self.lock();
        let before = sessions.len();
        sessions.retain(|s| !(s.agent == agent && s.session_id == session_id));
        sessions.len() != before
    }

    pub fn get(&self, agent: &str, session_id: &str) -> Option<SessionInfo> {
        self.lock()
            .iter()
            .find(|s| s.agent == agent && s.session_id == session_id)
            .cloned()
    }

    /// Waiting first, then most recent event.
    pub fn list(&self) -> Vec<SessionInfo> {
        let mut sessions = self.lock().clone();
        sessions.sort_by(|a, b| {
            let a_waiting = a.state == SessionState::Waiting;
            let b_waiting = b.state == SessionState::Waiting;
            b_waiting
                .cmp(&a_waiting)
                .then(b.last_event_at.cmp(&a.last_event_at))
        });
        sessions
    }

    pub fn waiting_count(&self) -> usize {
        self.lock()
            .iter()
            .filter(|s| s.state == SessionState::Waiting)
            .count()
    }

    /// Drops sessions silent for more than 12 hours.
    pub fn prune(&self, now: DateTime<Utc>) -> bool {
        let mut sessions = self.lock();
        let before = sessions.len();
        sessions.retain(|s| now - s.last_event_at <= STALE_AFTER);
        sessions.len() != before
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<SessionInfo>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Fields present in `newer` replace ours; absent ones keep ours. Same rule as
/// `SessionOrigin::merge`, kept local until that stub is filled in.
fn merge_origin(existing: &mut SessionOrigin, newer: &SessionOrigin) {
    fn take(slot: &mut Option<String>, newer: &Option<String>) {
        if newer.is_some() {
            *slot = newer.clone();
        }
    }
    take(&mut existing.entrypoint, &newer.entrypoint);
    take(&mut existing.host_session, &newer.host_session);
    take(&mut existing.warp_focus_url, &newer.warp_focus_url);
    take(&mut existing.term_program, &newer.term_program);
    take(&mut existing.bundle_id, &newer.bundle_id);
    take(&mut existing.originator, &newer.originator);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 9, 0, 0).unwrap() + Duration::minutes(minutes)
    }

    fn touch(
        sessions: &Sessions,
        session_id: &str,
        state: Option<SessionState>,
        now: DateTime<Utc>,
    ) -> bool {
        sessions.touch(
            "claude-code",
            session_id,
            "/Users/me/projects/samlu",
            &SessionOrigin::default(),
            state,
            now,
        )
    }

    #[test]
    fn touch_registers_and_updates_state() {
        let sessions = Sessions::new();
        assert!(touch(&sessions, "s1", Some(SessionState::Working), at(0)));
        let list = sessions.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, SessionState::Working);
        assert_eq!(list[0].agent_label, "Claude Code");
        assert_eq!(list[0].project_label, "samlu");

        touch(&sessions, "s1", Some(SessionState::Waiting), at(1));
        assert_eq!(sessions.list()[0].state, SessionState::Waiting);

        touch(&sessions, "s1", None, at(2));
        assert_eq!(sessions.list()[0].state, SessionState::Waiting);
        assert_eq!(sessions.list()[0].last_event_at, at(2));
        assert_eq!(sessions.list()[0].started_at, at(0));
    }

    #[test]
    fn waiting_sorted_first_then_recent() {
        let sessions = Sessions::new();
        touch(&sessions, "old-idle", Some(SessionState::Idle), at(0));
        touch(&sessions, "new-idle", Some(SessionState::Idle), at(10));
        touch(&sessions, "waiting", Some(SessionState::Waiting), at(5));

        let ids: Vec<String> = sessions.list().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, ["waiting", "new-idle", "old-idle"]);
    }

    #[test]
    fn waiting_count_counts_sessions() {
        let sessions = Sessions::new();
        touch(&sessions, "a", Some(SessionState::Waiting), at(0));
        touch(&sessions, "b", Some(SessionState::Waiting), at(0));
        touch(&sessions, "c", Some(SessionState::Working), at(0));
        assert_eq!(sessions.waiting_count(), 2);
    }

    #[test]
    fn end_removes() {
        let sessions = Sessions::new();
        touch(&sessions, "s1", Some(SessionState::Working), at(0));
        assert!(sessions.get("claude-code", "s1").is_some());
        assert!(sessions.end("claude-code", "s1"));
        assert!(sessions.get("claude-code", "s1").is_none());
        assert!(!sessions.end("claude-code", "s1"));
    }

    #[test]
    fn prune_drops_after_12h() {
        let sessions = Sessions::new();
        touch(&sessions, "stale", Some(SessionState::Idle), at(0));
        touch(&sessions, "fresh", Some(SessionState::Idle), at(2));
        let now = at(0) + Duration::hours(12) + Duration::minutes(1);
        assert!(sessions.prune(now));
        let ids: Vec<String> = sessions.list().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, ["fresh"]);

        let kept = Sessions::new();
        touch(&kept, "s1", Some(SessionState::Idle), at(0));
        let almost = at(0) + Duration::hours(11) + Duration::minutes(59);
        assert!(!kept.prune(almost));
        assert_eq!(kept.list().len(), 1);
    }

    #[test]
    fn origin_merges_on_touch() {
        let sessions = Sessions::new();
        let first = SessionOrigin {
            host_session: Some("local_abc".to_string()),
            term_program: Some("Apple_Terminal".to_string()),
            ..SessionOrigin::default()
        };
        sessions.touch("claude-code", "s1", "/p", &first, None, at(0));

        let second = SessionOrigin {
            term_program: Some("WarpTerminal".to_string()),
            ..SessionOrigin::default()
        };
        sessions.touch("claude-code", "s1", "/p", &second, None, at(1));

        let origin = sessions.get("claude-code", "s1").unwrap().origin;
        assert_eq!(origin.host_session.as_deref(), Some("local_abc"));
        assert_eq!(origin.term_program.as_deref(), Some("WarpTerminal"));
    }
}

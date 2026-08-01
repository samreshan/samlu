//! Shared event model. Every agent adapter normalizes into `AgentEvent` here;
//! notification delivery only deals with this type, never with a specific
//! tool's raw hook payload. That lets another adapter be added later without
//! touching notification delivery.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Adapter name, e.g. "claude-code".
    pub agent: String,
    pub kind: AgentEventKind,
    pub project_path: String,
    pub summary: Option<String>,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
}

impl AgentEvent {
    /// "claude-code" -> "Claude Code" — every adapter's `name()` is a
    /// kebab-case registry key, not user-facing copy.
    pub fn agent_label(&self) -> String {
        self.agent
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Last path segment for compact display in notifications and history.
    pub fn project_label(&self) -> String {
        if self.project_path.is_empty() {
            return "a project".to_string();
        }
        self.project_path
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(&self.project_path)
            .to_string()
    }
}

/// Read-only, frontend-friendly view of a stored event for the Status tab's
/// history list — same data as `AgentEvent` plus the display strings already
/// computed on the Rust side, so the UI doesn't have to re-derive them.
#[derive(Debug, Clone, Serialize)]
pub struct EventHistoryItem {
    pub agent: String,
    pub agent_label: String,
    pub kind_label: &'static str,
    pub project_label: String,
    pub summary: Option<String>,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
}

impl From<&AgentEvent> for EventHistoryItem {
    fn from(event: &AgentEvent) -> Self {
        Self {
            agent: event.agent.clone(),
            agent_label: event.agent_label(),
            kind_label: event.kind.label(),
            project_label: event.project_label(),
            summary: event.summary.clone(),
            session_id: event.session_id.clone(),
            timestamp: event.timestamp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    /// The agent is blocked waiting on you (permission prompt, idle prompt,
    /// or an explicit "needs input" signal).
    NeedsInput,
    /// The agent finished a whole task/turn it flagged as complete.
    Completed,
    /// The agent finished responding, but didn't explicitly flag "done" —
    /// noisier signal, needs debouncing before surfacing to the user.
    TurnFinished,
}

impl AgentEventKind {
    /// Human-readable label for agent activity.
    pub fn label(self) -> &'static str {
        match self {
            AgentEventKind::NeedsInput => "needs your input",
            AgentEventKind::Completed => "finished a task",
            AgentEventKind::TurnFinished => "finished a turn",
        }
    }
}

/// Why parsing a hook payload failed (or intentionally produced nothing).
#[derive(Debug)]
pub enum AdapterError {
    /// The body wasn't valid/expected JSON for this adapter.
    InvalidJson(String),
    /// No adapter is registered for this name, or no route for this event.
    UnknownRoute(String),
    /// Valid payload, but this specific event type is intentionally a no-op
    /// (e.g. Claude Code's `auth_success`/`elicitation_complete` — purely
    /// informational, nothing for the user to react to).
    Ignored,
}

/// One agent CLI's integration: turns its raw hook payload into an
/// `AgentEvent`. Implement this trait to add support for a new tool.
pub trait AgentAdapter: Send + Sync {
    /// Registry key, matched against the `{adapter}` path segment in
    /// `/hooks/{adapter}/{event}/{token}`.
    fn name(&self) -> &'static str;

    /// `event_route` is whatever came after the adapter name in the URL
    /// (e.g. "notification" or "stop" for Claude Code), so one adapter can
    /// handle multiple hook endpoints with different payload shapes.
    fn parse(&self, event_route: &str, body: &[u8]) -> Result<AgentEvent, AdapterError>;
}

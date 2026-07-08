//! Shared event model. Every agent adapter normalizes into `AgentEvent` here;
//! `notify.rs` and `pet.rs` only ever deal with this type, never with a
//! specific tool's raw hook payload — that's what lets a new adapter (Codex,
//! Cursor, ...) be added later without touching notification/pet code.

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
    /// Higher = more important to keep on screen when multiple events are
    /// competing for the single mascot's one visible slot.
    pub fn priority(self) -> u8 {
        match self {
            AgentEventKind::NeedsInput => 2,
            AgentEventKind::Completed => 1,
            AgentEventKind::TurnFinished => 0,
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

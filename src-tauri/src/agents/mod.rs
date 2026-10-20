//! Agent sessions, approvals and alert sounds. The listener turns every hook
//! into a `HookMessage`; the hub decides what that means for sessions, cards,
//! held approvals and sounds.

pub mod action;
pub mod approvals;
pub mod hub;
pub mod island_queue;
pub mod origin;
pub mod sessions;
pub mod shortcuts;
pub mod sounds;
pub mod view;

use crate::events::AgentEvent;
use origin::SessionOrigin;
use serde::Serialize;

/// What one hook request turned into.
#[derive(Debug, Clone)]
pub enum HookMessage {
    /// A notification-style event, handled by the existing notify path.
    Event(AgentEvent),
    /// A tool call waiting on the user's decision; the listener holds it open.
    Approval(ApprovalRequest),
    /// Session bookkeeping only: no card, no sound.
    Signal(SessionSignal),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub agent: String,
    pub session_id: String,
    /// Set when a subagent asks: it shares its parent's `session_id`.
    pub agent_id: Option<String>,
    pub project_path: String,
    pub tool_name: String,
    pub action: String,
    /// Claude only: the first `addRules` + `behavior: "allow"` permission suggestion, verbatim.
    pub always_allow: Option<serde_json::Value>,
    pub origin: SessionOrigin,
}

#[derive(Debug, Clone)]
pub struct SessionSignal {
    pub agent: String,
    pub session_id: String,
    /// Set when a subagent's tool call finished.
    pub agent_id: Option<String>,
    pub project_path: String,
    pub kind: SignalKind,
    pub origin: SessionOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Started,
    Ended,
    Working,
    ToolFinished,
}

/// "claude-code" -> "Claude Code" — every adapter's `name()` is a
/// kebab-case registry key, not user-facing copy.
pub fn agent_label(agent: &str) -> String {
    match agent {
        "claude-code" => return "Claude Code".to_string(),
        "codex" => return "Codex".to_string(),
        "antigravity" => return "Antigravity".to_string(),
        "gemini-cli" => return "Gemini CLI".to_string(),
        _ => {}
    }
    agent
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
pub fn project_label(path: &str) -> String {
    if path.is_empty() {
        return "a project".to_string();
    }
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_string()
}

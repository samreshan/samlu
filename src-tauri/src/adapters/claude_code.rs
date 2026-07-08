//! Claude Code adapter — parses the `Notification`/`Stop` hook payloads
//! Claude Code POSTs to our local server, per the exact schema documented at
//! https://code.claude.com/docs/en/hooks.
//!
//! Hook config a user's ~/.claude/settings.json (or project-level
//! .claude/settings.json) needs, written by setup/claude_hooks.rs:
//!
//! ```jsonc
//! {
//!   "hooks": {
//!     "Notification": [{ "hooks": [{ "type": "http", "url": "http://127.0.0.1:47823/hooks/claude-code/notification/<TOKEN>", "timeout": 5 }] }],
//!     "Stop":         [{ "hooks": [{ "type": "http", "url": "http://127.0.0.1:47823/hooks/claude-code/stop/<TOKEN>",         "timeout": 5 }] }]
//!   }
//! }
//! ```

use crate::events::{AdapterError, AgentAdapter, AgentEvent, AgentEventKind};
use chrono::Utc;
use serde::Deserialize;

pub struct ClaudeCodeAdapter;

#[derive(Debug, Deserialize)]
struct NotificationPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
    notification_type: String,
}

#[derive(Debug, Deserialize)]
struct StopPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    last_assistant_message: Option<String>,
    /// Present only for SubagentStop — internal orchestration noise we
    /// intentionally ignore in v1 (see AdapterError::Ignored below).
    #[serde(default)]
    agent_id: Option<String>,
}

impl AgentAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn parse(&self, event_route: &str, body: &[u8]) -> Result<AgentEvent, AdapterError> {
        match event_route {
            "notification" => parse_notification(body),
            "stop" => parse_stop(body),
            other => Err(AdapterError::UnknownRoute(other.to_string())),
        }
    }
}

fn parse_notification(body: &[u8]) -> Result<AgentEvent, AdapterError> {
    let payload: NotificationPayload =
        serde_json::from_slice(body).map_err(|e| AdapterError::InvalidJson(e.to_string()))?;

    let kind = match payload.notification_type.as_str() {
        "permission_prompt" | "agent_needs_input" | "elicitation_dialog" | "idle_prompt" => {
            AgentEventKind::NeedsInput
        }
        "agent_completed" => AgentEventKind::Completed,
        // auth_success / elicitation_complete / elicitation_response are purely
        // informational — nothing for the user to act on.
        _ => return Err(AdapterError::Ignored),
    };

    Ok(AgentEvent {
        agent: "claude-code".to_string(),
        kind,
        project_path: payload.cwd,
        summary: Some(describe_notification(&payload.notification_type)),
        session_id: payload.session_id,
        timestamp: Utc::now(),
    })
}

fn parse_stop(body: &[u8]) -> Result<AgentEvent, AdapterError> {
    let payload: StopPayload =
        serde_json::from_slice(body).map_err(|e| AdapterError::InvalidJson(e.to_string()))?;

    if payload.agent_id.is_some() {
        // SubagentStop — ignored in v1, see module doc comment.
        return Err(AdapterError::Ignored);
    }

    Ok(AgentEvent {
        agent: "claude-code".to_string(),
        kind: AgentEventKind::TurnFinished,
        project_path: payload.cwd,
        summary: payload.last_assistant_message,
        session_id: payload.session_id,
        timestamp: Utc::now(),
    })
}

fn describe_notification(notification_type: &str) -> String {
    match notification_type {
        "permission_prompt" => "Waiting for permission to continue".to_string(),
        "agent_needs_input" => "Needs your input".to_string(),
        "elicitation_dialog" => "Waiting on a response".to_string(),
        "idle_prompt" => "Has been idle a while".to_string(),
        "agent_completed" => "Finished a task".to_string(),
        other => other.to_string(),
    }
}

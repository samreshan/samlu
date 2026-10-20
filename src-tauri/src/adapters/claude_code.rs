//! Claude Code adapter — parses the `Notification`/`Stop` hook payloads
//! Claude Code POSTs to our local server, per the exact schema documented at
//! https://code.claude.com/docs/en/hooks.
//!
//! Eight routes are handled: notification/stop/task-completed become events,
//! permission becomes an approval to hold, and session-start/session-end/
//! prompt/post-tool are session signals only. Every payload carries
//! `session_id` and `cwd`.
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

use crate::agents::action::action_line;
use crate::agents::origin::SessionOrigin;
use crate::agents::{ApprovalRequest, HookMessage, SessionSignal, SignalKind};
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

#[derive(Debug, Deserialize)]
struct TaskCompletedPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
    task_subject: String,
    #[serde(default)]
    task_description: Option<String>,
    #[serde(default)]
    teammate_name: Option<String>,
}

/// The fields every Claude hook payload carries.
#[derive(Debug, Deserialize)]
struct CommonPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
}

#[derive(Debug, Deserialize)]
struct PermissionPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    permission_suggestions: Vec<serde_json::Value>,
}

impl AgentAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn parse(
        &self,
        event_route: &str,
        body: &[u8],
        origin: &SessionOrigin,
    ) -> Result<HookMessage, AdapterError> {
        let event = match event_route {
            "notification" => parse_notification(body),
            "stop" => parse_stop(body),
            "task-completed" => parse_task_completed(body),
            "permission" => return parse_permission(body, origin),
            "session-start" => return parse_signal(body, origin, SignalKind::Started),
            "session-end" => return parse_signal(body, origin, SignalKind::Ended),
            "prompt" => return parse_signal(body, origin, SignalKind::Working),
            "post-tool" => return parse_signal(body, origin, SignalKind::ToolFinished),
            other => Err(AdapterError::UnknownRoute(other.to_string())),
        };
        let mut event = event?;
        event.origin = origin.clone();
        Ok(HookMessage::Event(event))
    }
}

/// Subagent permission requests (`agent_id` present) are still approvals: the
/// user is the only one who can answer them.
fn parse_permission(body: &[u8], origin: &SessionOrigin) -> Result<HookMessage, AdapterError> {
    let payload: PermissionPayload =
        serde_json::from_slice(body).map_err(|e| AdapterError::InvalidJson(e.to_string()))?;
    let action = action_line(&payload.tool_name, &payload.tool_input);
    let always_allow = payload
        .permission_suggestions
        .into_iter()
        .find(|suggestion| {
            suggestion.get("type").and_then(|t| t.as_str()) == Some("addRules")
                && suggestion.get("behavior").and_then(|b| b.as_str()) == Some("allow")
        });
    Ok(HookMessage::Approval(ApprovalRequest {
        agent: "claude-code".to_string(),
        session_id: payload.session_id,
        project_path: payload.cwd,
        tool_name: payload.tool_name,
        action,
        always_allow,
        origin: origin.clone(),
    }))
}

fn parse_signal(
    body: &[u8],
    origin: &SessionOrigin,
    kind: SignalKind,
) -> Result<HookMessage, AdapterError> {
    let payload: CommonPayload =
        serde_json::from_slice(body).map_err(|e| AdapterError::InvalidJson(e.to_string()))?;
    Ok(HookMessage::Signal(SessionSignal {
        agent: "claude-code".to_string(),
        session_id: payload.session_id,
        project_path: payload.cwd,
        kind,
        origin: origin.clone(),
    }))
}

fn parse_task_completed(body: &[u8]) -> Result<AgentEvent, AdapterError> {
    let payload: TaskCompletedPayload =
        serde_json::from_slice(body).map_err(|e| AdapterError::InvalidJson(e.to_string()))?;
    let mut summary = payload.task_subject;
    if let Some(description) = payload
        .task_description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        summary.push_str(": ");
        summary.push_str(description);
    }
    if let Some(teammate) = payload
        .teammate_name
        .as_deref()
        .map(str::trim)
        .filter(|teammate| !teammate.is_empty())
    {
        summary.push_str(" (");
        summary.push_str(teammate);
        summary.push(')');
    }
    Ok(AgentEvent {
        agent: "claude-code".to_string(),
        kind: AgentEventKind::Completed,
        project_path: payload.cwd,
        summary: Some(summary),
        session_id: payload.session_id,
        timestamp: Utc::now(),
        origin: SessionOrigin::default(),
        outcome: None,
    })
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
        origin: SessionOrigin::default(),
        outcome: None,
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
        origin: SessionOrigin::default(),
        outcome: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIVE_PERMISSION: &str = r#"{"session_id":"dc7deea7-ac39-4614-a336-7c399922481f","transcript_path":"/Users/samreshan/.claude/projects/-Users-samreshan-Programming-samlu-companion/dc7deea7-ac39-4614-a336-7c399922481f.jsonl","cwd":"/Users/samreshan/Programming/samlu-companion","permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"curl -sI https://example.com","description":"Fetch response headers from example.com"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"curl -sI https://example.com"}],"behavior":"allow","destination":"localSettings"}]}"#;

    fn origin() -> SessionOrigin {
        SessionOrigin {
            entrypoint: Some("cli".to_string()),
            term_program: Some("ghostty".to_string()),
            ..SessionOrigin::default()
        }
    }

    fn parse(route: &str, body: &str) -> Result<HookMessage, AdapterError> {
        ClaudeCodeAdapter.parse(route, body.as_bytes(), &origin())
    }

    #[test]
    fn permission_route_builds_approval() {
        let Ok(HookMessage::Approval(approval)) = parse("permission", LIVE_PERMISSION) else {
            panic!("expected approval");
        };
        let payload: serde_json::Value = serde_json::from_str(LIVE_PERMISSION).unwrap();
        assert_eq!(approval.agent, "claude-code");
        assert_eq!(approval.session_id, "dc7deea7-ac39-4614-a336-7c399922481f");
        assert_eq!(
            approval.project_path,
            "/Users/samreshan/Programming/samlu-companion"
        );
        assert_eq!(approval.tool_name, "Bash");
        assert_eq!(approval.action, action_line("Bash", &payload["tool_input"]));
        assert_eq!(
            approval.always_allow,
            Some(payload["permission_suggestions"][0].clone())
        );
        assert_eq!(approval.origin, origin());
    }

    #[test]
    fn permission_without_allow_suggestion_has_no_always() {
        let body = json!({
            "session_id": "s", "cwd": "/p", "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "permission_suggestions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
                {"type": "addRules", "behavior": "deny", "rules": []}
            ]
        })
        .to_string();
        let Ok(HookMessage::Approval(approval)) = parse("permission", &body) else {
            panic!("expected approval");
        };
        assert_eq!(approval.always_allow, None);

        let bare = json!({"session_id": "s", "cwd": "/p", "tool_name": "Read"}).to_string();
        let Ok(HookMessage::Approval(approval)) = parse("permission", &bare) else {
            panic!("expected approval");
        };
        assert_eq!(approval.always_allow, None);
    }

    #[test]
    fn subagent_permission_is_still_an_approval() {
        let body = json!({
            "session_id": "s", "cwd": "/p", "agent_id": "a1", "tool_name": "Bash",
            "tool_input": {"command": "ls"}
        })
        .to_string();
        assert!(matches!(
            parse("permission", &body),
            Ok(HookMessage::Approval(_))
        ));
    }

    #[test]
    fn session_routes_are_signals() {
        let body = json!({"session_id": "s1", "cwd": "/p"}).to_string();
        for (route, expected) in [
            ("session-start", SignalKind::Started),
            ("session-end", SignalKind::Ended),
            ("prompt", SignalKind::Working),
            ("post-tool", SignalKind::ToolFinished),
        ] {
            let Ok(HookMessage::Signal(signal)) = parse(route, &body) else {
                panic!("{route}: expected signal");
            };
            assert_eq!(signal.kind, expected, "{route}");
            assert_eq!(signal.agent, "claude-code");
            assert_eq!(signal.session_id, "s1");
            assert_eq!(signal.project_path, "/p");
            assert_eq!(signal.origin, origin());
        }
    }

    #[test]
    fn stop_and_notification_carry_origin() {
        let stop = json!({"session_id": "s", "cwd": "/p"}).to_string();
        let Ok(HookMessage::Event(event)) = parse("stop", &stop) else {
            panic!("expected event");
        };
        assert_eq!(event.kind, AgentEventKind::TurnFinished);
        assert_eq!(event.origin, origin());

        let note =
            json!({"session_id": "s", "cwd": "/p", "notification_type": "permission_prompt"})
                .to_string();
        let Ok(HookMessage::Event(event)) = parse("notification", &note) else {
            panic!("expected event");
        };
        assert_eq!(event.kind, AgentEventKind::NeedsInput);
        assert_eq!(event.origin, origin());
    }

    #[test]
    fn unknown_route_and_bad_json_are_errors() {
        assert!(matches!(
            parse("bogus", "{}"),
            Err(AdapterError::UnknownRoute(_))
        ));
        assert!(matches!(
            parse("prompt", "not json"),
            Err(AdapterError::InvalidJson(_))
        ));
    }
}

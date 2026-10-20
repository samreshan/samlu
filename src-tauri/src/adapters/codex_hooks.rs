//! Codex CLI hook adapter. Codex runs the commands Samlu installed under
//! `[[hooks.<Event>]]` in `~/.codex/config.toml` and pipes the event JSON to
//! them; the command forwards it to `/hooks/codex/<route>/<token>`.

use crate::agents::action::action_line;
use crate::agents::{
    origin::SessionOrigin, ApprovalRequest, HookMessage, SessionSignal, SignalKind,
};
use crate::events::{AdapterError, AgentAdapter, AgentEvent, AgentEventKind};
use chrono::Utc;
use serde::Deserialize;

pub struct CodexHooksAdapter;

/// Fields every Codex hook payload carries, plus the ones only some events
/// add. Everything but the session id is optional so a trimmed payload still
/// parses.
#[derive(Debug, Deserialize)]
struct Payload {
    session_id: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    last_assistant_message: Option<String>,
}

impl AgentAdapter for CodexHooksAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn parse(
        &self,
        event_route: &str,
        body: &[u8],
        origin: &SessionOrigin,
    ) -> Result<HookMessage, AdapterError> {
        let signal = match event_route {
            "session-start" => Some(SignalKind::Started),
            "session-end" => Some(SignalKind::Ended),
            "prompt" => Some(SignalKind::Working),
            "post-tool" => Some(SignalKind::ToolFinished),
            "permission" | "stop" => None,
            other => return Err(AdapterError::UnknownRoute(other.to_string())),
        };
        let payload: Payload = serde_json::from_slice(body)
            .map_err(|error| AdapterError::InvalidJson(error.to_string()))?;

        if let Some(kind) = signal {
            return Ok(HookMessage::Signal(SessionSignal {
                agent: "codex".to_string(),
                session_id: payload.session_id,
                agent_id: payload.agent_id,
                project_path: payload.cwd,
                kind,
                origin: origin.clone(),
            }));
        }
        if event_route == "permission" {
            return Ok(HookMessage::Approval(ApprovalRequest {
                agent: "codex".to_string(),
                session_id: payload.session_id,
                agent_id: payload.agent_id,
                project_path: payload.cwd,
                action: action_line(&payload.tool_name, &payload.tool_input),
                tool_name: payload.tool_name,
                always_allow: None,
                origin: origin.clone(),
            }));
        }
        Ok(HookMessage::Event(AgentEvent {
            agent: "codex".to_string(),
            kind: AgentEventKind::Completed,
            project_path: payload.cwd,
            summary: payload.last_assistant_message,
            session_id: payload.session_id,
            timestamp: Utc::now(),
            origin: origin.clone(),
            outcome: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(route: &str, body: &str) -> Result<HookMessage, AdapterError> {
        CodexHooksAdapter.parse(route, body.as_bytes(), &SessionOrigin::default())
    }

    #[test]
    fn permission_payload() {
        let body = r#"{"session_id":"t1","turn_id":"u","cwd":"/p","hook_event_name":"PermissionRequest","tool_name":"exec","tool_input":{"command":"ls","description":null}}"#;
        let HookMessage::Approval(request) = parse("permission", body).unwrap() else {
            panic!("expected an approval");
        };
        assert_eq!(request.agent, "codex");
        assert_eq!(request.session_id, "t1");
        assert_eq!(request.project_path, "/p");
        assert_eq!(request.tool_name, "exec");
        let input = serde_json::json!({"command":"ls","description":null});
        assert_eq!(request.action, action_line("exec", &input));
        assert!(request.always_allow.is_none());
    }

    #[test]
    fn stop_is_completed_event() {
        let body = r#"{"session_id":"t1","turn_id":"u","cwd":"/p","hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"All done"}"#;
        let HookMessage::Event(event) = parse("stop", body).unwrap() else {
            panic!("expected an event");
        };
        assert_eq!(event.kind, AgentEventKind::Completed);
        assert_eq!(event.summary.as_deref(), Some("All done"));
        assert_eq!(event.session_id, "t1");
    }

    #[test]
    fn session_routes_are_signals() {
        for (route, expected) in [
            ("session-start", SignalKind::Started),
            ("session-end", SignalKind::Ended),
            ("prompt", SignalKind::Working),
            ("post-tool", SignalKind::ToolFinished),
        ] {
            let body = r#"{"session_id":"t1","cwd":"/p","model":"gpt-5"}"#;
            let HookMessage::Signal(signal) = parse(route, body).unwrap() else {
                panic!("{route} should be a signal");
            };
            assert_eq!(signal.kind, expected, "{route}");
            assert_eq!(signal.agent, "codex");
            assert_eq!(signal.project_path, "/p");
        }
    }

    #[test]
    fn unknown_route_and_bad_json_are_errors() {
        assert!(matches!(
            parse("nope", "{}"),
            Err(AdapterError::UnknownRoute(_))
        ));
        assert!(matches!(
            parse("stop", "not json"),
            Err(AdapterError::InvalidJson(_))
        ));
    }

    #[test]
    fn origin_is_carried_through() {
        let origin = SessionOrigin {
            term_program: Some("WarpTerminal".into()),
            ..Default::default()
        };
        let message = CodexHooksAdapter
            .parse("session-start", br#"{"session_id":"t1"}"#, &origin)
            .unwrap();
        let HookMessage::Signal(signal) = message else {
            panic!("expected a signal");
        };
        assert_eq!(signal.origin, origin);
    }
}

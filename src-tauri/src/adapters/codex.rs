//! Codex CLI notification adapter. Codex appends one JSON argument to the
//! command configured under `notify` in `~/.codex/config.toml`.

use crate::events::{AgentEvent, AgentEventKind};
use chrono::Utc;
use serde::Deserialize;

#[derive(Deserialize)]
struct CodexNotification {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "thread-id")]
    thread_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default, rename = "last-assistant-message")]
    last_assistant_message: Option<String>,
}

pub fn parse_cli_payload(payload: &str) -> Result<AgentEvent, String> {
    let notification: CodexNotification =
        serde_json::from_str(payload).map_err(|error| format!("invalid Codex payload: {error}"))?;
    if notification.event_type != "agent-turn-complete" {
        return Err(format!(
            "unsupported Codex notification type: {}",
            notification.event_type
        ));
    }
    Ok(AgentEvent {
        agent: "codex".to_string(),
        kind: AgentEventKind::Completed,
        project_path: notification.cwd,
        summary: notification.last_assistant_message,
        session_id: notification.thread_id,
        timestamp: Utc::now(),
    })
}

pub fn payload_from_args(args: &[String]) -> Option<&str> {
    args.iter()
        .position(|arg| arg == "--codex-notify")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::parse_cli_payload;
    use crate::events::AgentEventKind;

    #[test]
    fn parses_agent_turn_complete() {
        let payload = r#"{
            "type":"agent-turn-complete",
            "thread-id":"thread-1",
            "cwd":"/tmp/project",
            "last-assistant-message":"Tests passed"
        }"#;
        let event = parse_cli_payload(payload).unwrap();
        assert_eq!(event.agent, "codex");
        assert_eq!(event.kind, AgentEventKind::Completed);
        assert_eq!(event.summary.as_deref(), Some("Tests passed"));
    }
}

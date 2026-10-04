//! Parsers shared by command-hook integrations that forward their JSON stdin
//! to Samlu's authenticated localhost listener.

use crate::events::{AdapterError, AgentEvent, AgentEventKind};
use chrono::Utc;
use serde_json::Value;

pub fn parse(agent: &str, route: &str, body: &[u8]) -> Result<AgentEvent, AdapterError> {
    let payload: Value = serde_json::from_slice(body)
        .map_err(|error| AdapterError::InvalidJson(error.to_string()))?;
    let object = payload
        .as_object()
        .ok_or_else(|| AdapterError::InvalidJson("payload must be a JSON object".to_string()))?;

    let kind = match route {
        "completed" | "stop" | "after-agent" => AgentEventKind::Completed,
        "needs-input" | "notification" => AgentEventKind::NeedsInput,
        other => return Err(AdapterError::UnknownRoute(other.to_string())),
    };

    let session_id = string(object, &["session_id", "conversation_id", "thread_id"])
        .unwrap_or_else(|| "unknown-session".to_string());
    let project_path = string(object, &["cwd", "workspace", "project_path"]).unwrap_or_default();
    let summary = string(
        object,
        &[
            "prompt_response",
            "last_assistant_message",
            "message",
            "reason",
            "status",
        ],
    )
    .or_else(|| {
        object
            .get("details")
            .and_then(Value::as_object)
            .and_then(|details| string(details, &["message", "reason", "tool_name"]))
    });

    Ok(AgentEvent {
        agent: agent.to_string(),
        kind,
        project_path,
        summary,
        session_id,
        timestamp: Utc::now(),
    })
}

fn string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::events::AgentEventKind;

    #[test]
    fn parses_completion_and_permission_payloads() {
        let completion = parse(
            "gemini-cli",
            "after-agent",
            br#"{"session_id":"abc","cwd":"/tmp/demo","prompt_response":"Done"}"#,
        )
        .unwrap();
        assert_eq!(completion.kind, AgentEventKind::Completed);
        assert_eq!(completion.summary.as_deref(), Some("Done"));

        let input = parse(
            "antigravity",
            "needs-input",
            br#"{"conversation_id":"xyz","cwd":"/tmp/demo","message":"Permission needed"}"#,
        )
        .unwrap();
        assert_eq!(input.kind, AgentEventKind::NeedsInput);
    }
}

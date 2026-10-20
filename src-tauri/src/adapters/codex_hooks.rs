//! Codex CLI hook adapter (`~/.codex/hooks.json`). Stub until Task 4.

use crate::agents::{origin::SessionOrigin, HookMessage};
use crate::events::{AdapterError, AgentAdapter};

pub struct CodexHooksAdapter;

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
        let _ = (body, origin);
        Err(AdapterError::UnknownRoute(event_route.to_string()))
    }
}

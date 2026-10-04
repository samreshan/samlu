//! Compile-time adapter registry. Not a dynamic plugin loader — v1 ships one
//! real adapter (Claude Code); a future Codex/Cursor/etc. adapter is added by
//! implementing `AgentAdapter` in a new module here and registering it in
//! `AdapterRegistry::new()`. The server and notification policy never
//! need to change when a new adapter is added — normalization happens here.

mod claude_code;
pub mod codex;
mod hook_json;

use crate::events::{AdapterError, AgentAdapter, AgentEvent};
use std::collections::HashMap;

pub struct AdapterRegistry {
    adapters: HashMap<&'static str, Box<dyn AgentAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        let mut adapters: HashMap<&'static str, Box<dyn AgentAdapter>> = HashMap::new();
        let claude = claude_code::ClaudeCodeAdapter;
        adapters.insert(claude.name(), Box::new(claude));
        let antigravity = JsonHookAdapter::new("antigravity");
        adapters.insert(antigravity.name(), Box::new(antigravity));
        let gemini = JsonHookAdapter::new("gemini-cli");
        adapters.insert(gemini.name(), Box::new(gemini));
        Self { adapters }
    }

    /// `adapter_name` is the `{adapter}` path segment in
    /// `/hooks/{adapter}/{event}/{token}` (e.g. "claude-code").
    pub fn parse(
        &self,
        adapter_name: &str,
        event_route: &str,
        body: &[u8],
    ) -> Result<AgentEvent, AdapterError> {
        match self.adapters.get(adapter_name) {
            Some(adapter) => adapter.parse(event_route, body),
            None => Err(AdapterError::UnknownRoute(adapter_name.to_string())),
        }
    }
}

struct JsonHookAdapter {
    name: &'static str,
}

impl JsonHookAdapter {
    const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl AgentAdapter for JsonHookAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn parse(&self, event_route: &str, body: &[u8]) -> Result<AgentEvent, AdapterError> {
        hook_json::parse(self.name, event_route, body)
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

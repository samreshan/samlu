//! Compile-time adapter registry. Not a dynamic plugin loader — v1 ships one
//! real adapter (Claude Code); a future Codex/Cursor/etc. adapter is added by
//! implementing `AgentAdapter` in a new module here and registering it in
//! `AdapterRegistry::new()`. `server/mod.rs`, `notify.rs`, and `pet.rs` never
//! need to change when a new adapter is added — normalization happens here.

mod claude_code;

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

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

//! Held approvals: each one parks a listener thread until the user (or a
//! deadline) answers through a Tauri command.

use super::ApprovalRequest;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;

/// `NoDecision` is the contract name: it answers the hook with nothing.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    AllowAlways,
    Deny,
    NoDecision,
}

pub struct PendingApprovals {
    #[allow(dead_code)]
    pending: Mutex<Vec<(String, ApprovalRequest)>>,
}

impl Default for PendingApprovals {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingApprovals {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    pub fn open(&self, request: ApprovalRequest) -> (String, Receiver<Decision>) {
        let _ = request;
        let (_sender, receiver) = mpsc::channel();
        (String::new(), receiver)
    }

    pub fn answer(&self, request_id: &str, decision: Decision) -> Option<ApprovalRequest> {
        let _ = (request_id, decision);
        None
    }

    pub fn retire_session(&self, agent: &str, session_id: &str) -> Vec<String> {
        let _ = (agent, session_id);
        Vec::new()
    }

    pub fn get(&self, request_id: &str) -> Option<ApprovalRequest> {
        let _ = request_id;
        None
    }

    pub fn len(&self) -> usize {
        0
    }
}

/// The HTTP body that answers the hook for `decision`; empty/`{}` means
/// "no decision" so the agent falls back to asking in its own terminal.
pub fn response_body(agent: &str, decision: Decision, request: &ApprovalRequest) -> Vec<u8> {
    let _ = (agent, decision, request);
    Vec::new()
}

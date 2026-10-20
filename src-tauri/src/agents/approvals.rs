//! Held approvals: each one parks a listener thread until the user (or a
//! deadline) answers through a Tauri command.

use super::ApprovalRequest;
use rand::Rng;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
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

struct Entry {
    id: String,
    request: ApprovalRequest,
    sender: SyncSender<Decision>,
}

pub struct PendingApprovals {
    pending: Mutex<Vec<Entry>>,
    counter: AtomicU64,
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
            counter: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        // A poisoned lock only means another thread panicked mid-push; the
        // list itself is still coherent, and approvals must keep working.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a held request. The random suffix keeps ids unguessable
    /// even though they only ever travel through Tauri commands.
    pub fn open(&self, request: ApprovalRequest) -> (String, Receiver<Decision>) {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let suffix: u32 = rand::thread_rng().gen_range(0..0x100_0000);
        let id = format!("req-{n}{suffix:06x}");
        let (sender, receiver) = mpsc::sync_channel(1);
        self.lock().push(Entry {
            id: id.clone(),
            request,
            sender,
        });
        (id, receiver)
    }

    /// Removes the request first, then delivers; a dropped receiver is fine.
    pub fn answer(&self, request_id: &str, decision: Decision) -> Option<ApprovalRequest> {
        let entry = {
            let mut list = self.lock();
            let pos = list.iter().position(|e| e.id == request_id)?;
            list.remove(pos)
        };
        let _ = entry.sender.try_send(decision);
        Some(entry.request)
    }

    /// Answers every pending request of one session with `NoDecision`.
    pub fn retire_session(&self, agent: &str, session_id: &str) -> Vec<String> {
        let removed: Vec<Entry> = {
            let mut list = self.lock();
            let (gone, keep): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut *list)
                .into_iter()
                .partition(|e| e.request.agent == agent && e.request.session_id == session_id);
            *list = keep;
            gone
        };
        removed
            .into_iter()
            .map(|e| {
                let _ = e.sender.try_send(Decision::NoDecision);
                e.id
            })
            .collect()
    }

    #[cfg(test)]
    pub fn get(&self, request_id: &str) -> Option<ApprovalRequest> {
        self.lock()
            .iter()
            .find(|e| e.id == request_id)
            .map(|e| e.request.clone())
    }

    #[cfg(test)]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

/// The HTTP body that answers the hook for `decision`; empty/`{}` means
/// "no decision" so the agent falls back to asking in its own terminal.
/// Only Claude Code understands `interrupt` and `updatedPermissions`; every
/// other agent is treated like Codex.
pub fn response_body(agent: &str, decision: Decision, request: &ApprovalRequest) -> Vec<u8> {
    let claude = agent == "claude-code";
    let decision_obj = match decision {
        Decision::NoDecision => return if claude { b"{}".to_vec() } else { Vec::new() },
        Decision::Allow => json!({"behavior": "allow"}),
        Decision::AllowAlways => match (&request.always_allow, claude) {
            (Some(rule), true) => json!({"behavior": "allow", "updatedPermissions": [rule]}),
            _ => json!({"behavior": "allow"}),
        },
        Decision::Deny if claude => {
            json!({"behavior": "deny", "message": "Denied from Samlu", "interrupt": false})
        }
        Decision::Deny => json!({"behavior": "deny", "message": "Denied from Samlu"}),
    };
    let body = json!({"hookSpecificOutput": {
        "hookEventName": "PermissionRequest",
        "decision": decision_obj,
    }});
    serde_json::to_vec(&body).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::origin::SessionOrigin;
    use serde_json::Value;

    fn req(agent: &str, session: &str) -> ApprovalRequest {
        ApprovalRequest {
            agent: agent.into(),
            session_id: session.into(),
            project_path: "/p".into(),
            tool_name: "Bash".into(),
            action: "ls".into(),
            always_allow: None,
            origin: SessionOrigin::default(),
        }
    }

    fn parse(b: Vec<u8>) -> Value {
        serde_json::from_slice(&b).unwrap()
    }

    fn decision_of(b: Vec<u8>) -> Value {
        parse(b)["hookSpecificOutput"]["decision"].clone()
    }

    #[test]
    fn answer_delivers_once() {
        let p = PendingApprovals::new();
        let (id, rx) = p.open(req("claude-code", "s1"));
        assert!(p.answer(&id, Decision::Allow).is_some());
        assert_eq!(rx.recv().unwrap(), Decision::Allow);
        assert!(p.answer(&id, Decision::Deny).is_none());
    }

    #[test]
    fn ids_are_unique_and_prefixed() {
        let p = PendingApprovals::new();
        let (a, _ra) = p.open(req("codex", "s"));
        let (b, _rb) = p.open(req("codex", "s"));
        assert_ne!(a, b);
        assert!(a.starts_with("req-1"));
        assert!(p.get(&a).is_some());
    }

    #[test]
    fn answer_with_dropped_receiver_is_ignored() {
        let p = PendingApprovals::new();
        let (id, rx) = p.open(req("codex", "s"));
        drop(rx);
        assert!(p.answer(&id, Decision::Allow).is_some());
    }

    #[test]
    fn retire_session_only_touches_that_session() {
        let p = PendingApprovals::new();
        let (a, ra) = p.open(req("claude-code", "s1"));
        let (b, rb) = p.open(req("claude-code", "s1"));
        let (_c, _rc) = p.open(req("claude-code", "s2"));
        let (_d, _rd) = p.open(req("codex", "s1"));
        let mut ids = p.retire_session("claude-code", "s1");
        ids.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(ids, want);
        assert_eq!(ra.recv().unwrap(), Decision::NoDecision);
        assert_eq!(rb.recv().unwrap(), Decision::NoDecision);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn claude_allow_body() {
        let r = req("claude-code", "s");
        assert_eq!(
            parse(response_body("claude-code", Decision::Allow, &r)),
            json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}})
        );
    }

    #[test]
    fn claude_always_body_carries_suggestion() {
        let mut r = req("claude-code", "s");
        let s = json!({"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"ls:*"}],"behavior":"allow","destination":"localSettings"});
        r.always_allow = Some(s.clone());
        let d = decision_of(response_body("claude-code", Decision::AllowAlways, &r));
        assert_eq!(d["behavior"], "allow");
        assert_eq!(d["updatedPermissions"], json!([s]));
    }

    #[test]
    fn claude_always_without_suggestion_is_plain_allow() {
        let r = req("claude-code", "s");
        let d = decision_of(response_body("claude-code", Decision::AllowAlways, &r));
        assert_eq!(d, json!({"behavior": "allow"}));
    }

    #[test]
    fn claude_deny_body() {
        let r = req("claude-code", "s");
        let d = decision_of(response_body("claude-code", Decision::Deny, &r));
        assert_eq!(
            d,
            json!({"behavior":"deny","message":"Denied from Samlu","interrupt":false})
        );
    }

    #[test]
    fn codex_deny_body_has_no_interrupt() {
        let r = req("codex", "s");
        let d = decision_of(response_body("codex", Decision::Deny, &r));
        assert_eq!(d, json!({"behavior":"deny","message":"Denied from Samlu"}));
    }

    #[test]
    fn codex_always_falls_back_to_allow() {
        let mut r = req("codex", "s");
        r.always_allow = Some(json!({"x": 1}));
        let d = decision_of(response_body("codex", Decision::AllowAlways, &r));
        assert_eq!(d, json!({"behavior": "allow"}));
    }

    #[test]
    fn unknown_agent_is_treated_like_codex() {
        let mut r = req("other", "s");
        r.always_allow = Some(json!({"x": 1}));
        let d = decision_of(response_body("other", Decision::Deny, &r));
        assert!(d.get("interrupt").is_none());
        assert!(response_body("other", Decision::NoDecision, &r).is_empty());
    }

    #[test]
    fn no_decision_bodies() {
        let r = req("claude-code", "s");
        assert_eq!(
            response_body("claude-code", Decision::NoDecision, &r),
            b"{}"
        );
        assert!(response_body("codex", Decision::NoDecision, &r).is_empty());
    }
}

//! The one place that turns hook messages into sessions, cards, held
//! approvals and sounds. Stubs until Task 12.

use super::approvals::{Decision, PendingApprovals};
use super::island_queue::IslandQueue;
use super::sessions::{SessionInfo, Sessions};
use super::{ApprovalRequest, HookMessage};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub struct AgentHub {
    app_data_dir: PathBuf,
    sessions: Sessions,
    approvals: PendingApprovals,
    queue: Mutex<IslandQueue>,
}

impl AgentHub {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            app_data_dir: app_data_dir.to_path_buf(),
            sessions: Sessions::new(),
            approvals: PendingApprovals::new(),
            queue: Mutex::new(IslandQueue::new()),
        }
    }

    pub fn handle(&self, app: &AppHandle, message: HookMessage) {
        let _ = (app, message);
    }

    /// Opens a held approval and returns (request id, receiver, hold).
    pub fn begin_approval(
        &self,
        app: &AppHandle,
        request: ApprovalRequest,
    ) -> (String, Receiver<Decision>, Duration) {
        let _ = (app, request);
        let (_sender, receiver) = mpsc::channel();
        (String::new(), receiver, Duration::from_secs(0))
    }

    pub fn hold_elapsed(&self, app: &AppHandle, request_id: &str) {
        let _ = (app, request_id);
    }

    pub fn client_gone(&self, app: &AppHandle, request_id: &str) {
        let _ = (app, request_id);
    }
}

/// Used by the tray and `agent_session_view`.
pub fn view_session(app: &AppHandle, agent: &str, session_id: &str) {
    let _ = (app, agent, session_id);
}

/// `approve` | `always` | `deny` | `view` | `dismiss`
#[tauri::command]
pub fn agent_card_action(app: AppHandle, card_id: String, action: String) -> Result<(), String> {
    let _ = (app, card_id, action);
    Ok(())
}

#[tauri::command]
pub fn agent_queue_step(app: AppHandle, delta: i32) {
    let _ = (app, delta);
}

#[tauri::command]
pub fn agent_sessions(app: AppHandle) -> Vec<SessionInfo> {
    let _ = app;
    Vec::new()
}

#[tauri::command]
pub fn agent_session_view(app: AppHandle, agent: String, session_id: String) -> Result<(), String> {
    view_session(&app, &agent, &session_id);
    Ok(())
}

/// `{"system": [names]}`
#[tauri::command]
pub fn agent_sound_catalog() -> serde_json::Value {
    serde_json::json!({ "system": super::sounds::system_sounds() })
}

#[tauri::command]
pub fn agent_sound_preview(app: AppHandle, choice: String) {
    let _ = (app, choice);
}

#[tauri::command]
pub async fn agent_sound_choose_custom(app: AppHandle) -> Result<Option<String>, String> {
    let _ = app;
    Ok(None)
}

/// The listener's view of the hub.
pub struct HubHost {
    app: AppHandle,
}

impl HubHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn hub(&self) -> Arc<AgentHub> {
        self.app.state::<Arc<AgentHub>>().inner().clone()
    }
}

impl crate::server::HookHost for HubHost {
    fn handle(&self, message: HookMessage) {
        self.hub().handle(&self.app, message);
    }

    fn begin_approval(&self, request: ApprovalRequest) -> crate::server::OpenApproval {
        let (id, decisions, hold) = self.hub().begin_approval(&self.app, request);
        crate::server::OpenApproval {
            id,
            decisions,
            hold,
        }
    }

    fn hold_elapsed(&self, request_id: &str) {
        self.hub().hold_elapsed(&self.app, request_id);
    }

    fn client_gone(&self, request_id: &str) {
        self.hub().client_gone(&self.app, request_id);
    }
}

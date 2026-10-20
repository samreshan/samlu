//! Approve/deny global shortcuts, registered only while an approvable card
//! is in front.

use tauri::AppHandle;

pub fn arm(app: &AppHandle, approve: &str, deny: &str) -> Result<(), String> {
    let _ = (app, approve, deny);
    Ok(())
}

pub fn disarm(app: &AppHandle) {
    let _ = app;
}

pub fn is_armed() -> bool {
    false
}

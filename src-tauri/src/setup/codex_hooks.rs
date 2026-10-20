//! Installs Samlu's hooks into `~/.codex/hooks.json`. Stubs until Task 6.

// Wired in Task 12.
#![allow(dead_code)]

use super::InstallState;
use std::path::Path;

pub fn status(port: u16, token: &str) -> Result<InstallState, String> {
    let _ = (port, token);
    Ok(InstallState::NotInstalled)
}

/// The JSON that `apply` would write, for the setup wizard's diff view.
pub fn preview(port: u16, token: &str) -> Result<String, String> {
    let _ = (port, token);
    Ok(String::new())
}

pub fn apply(app_data_dir: &Path, port: u16, token: &str) -> Result<(), String> {
    let _ = (app_data_dir, port, token);
    Ok(())
}

pub fn uninstall(app_data_dir: &Path) -> Result<(), String> {
    let _ = app_data_dir;
    Ok(())
}

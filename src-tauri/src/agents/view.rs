//! Finding and opening the window a session lives in.

use super::origin::SessionOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewTarget {
    Url(String),
    App(String),
    NotFound,
}

pub fn resolve(agent: &str, session_id: &str, origin: &SessionOrigin) -> ViewTarget {
    let _ = (agent, session_id, origin);
    ViewTarget::NotFound
}

pub fn open(target: &ViewTarget) -> Result<(), String> {
    let _ = target;
    Ok(())
}

//! Where a session is running, taken from the `X-Samlu-*` headers the hook
//! command adds. Used to bring the user back to the right window.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionOrigin {
    /// X-Samlu-Entrypoint
    pub entrypoint: Option<String>,
    /// X-Samlu-Host-Session
    pub host_session: Option<String>,
    /// X-Samlu-Warp-Focus
    pub warp_focus_url: Option<String>,
    /// X-Samlu-Term
    pub term_program: Option<String>,
    /// X-Samlu-Bundle
    pub bundle_id: Option<String>,
    /// X-Samlu-Originator
    pub originator: Option<String>,
}

impl SessionOrigin {
    pub fn from_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let _ = headers;
        Self::default()
    }

    /// Fields present in `newer` replace ours; absent ones keep ours.
    pub fn merge(&mut self, newer: &SessionOrigin) {
        let _ = newer;
    }
}

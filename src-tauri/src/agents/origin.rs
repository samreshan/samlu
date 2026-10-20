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
        let mut origin = Self::default();
        for (name, value) in headers {
            let Some(value) = clean(value) else { continue };
            match name.to_ascii_lowercase().as_str() {
                "x-samlu-entrypoint" => origin.entrypoint = Some(value),
                "x-samlu-host-session" => origin.host_session = Some(value),
                "x-samlu-warp-focus" => origin.warp_focus_url = Some(value),
                "x-samlu-term" => origin.term_program = Some(value),
                "x-samlu-bundle" => origin.bundle_id = Some(value),
                "x-samlu-originator" => origin.originator = Some(value),
                _ => {}
            }
        }
        origin
    }

    /// Fields present in `newer` replace ours; absent ones keep ours.
    pub fn merge(&mut self, newer: &SessionOrigin) {
        fn take(ours: &mut Option<String>, newer: &Option<String>) {
            if newer.is_some() {
                *ours = newer.clone();
            }
        }
        take(&mut self.entrypoint, &newer.entrypoint);
        take(&mut self.host_session, &newer.host_session);
        take(&mut self.warp_focus_url, &newer.warp_focus_url);
        take(&mut self.term_program, &newer.term_program);
        take(&mut self.bundle_id, &newer.bundle_id);
        take(&mut self.originator, &newer.originator);
    }
}

/// Trims a header value. Empty values, and values left as a literal
/// `%NAME%` because the shell never expanded the variable, count as absent.
fn clean(value: &str) -> Option<String> {
    let value = value.trim();
    let unexpanded = value.starts_with('%') && value.ends_with('%');
    if value.is_empty() || unexpanded {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_samlu_headers_case_insensitively() {
        let o = SessionOrigin::from_headers([
            ("x-samlu-warp-focus", "warp://session/abc"),
            ("X-Samlu-Entrypoint", "cli"),
            ("X-Samlu-Term", "WarpTerminal"),
        ]);
        assert_eq!(o.warp_focus_url.as_deref(), Some("warp://session/abc"));
        assert_eq!(o.entrypoint.as_deref(), Some("cli"));
        assert_eq!(o.term_program.as_deref(), Some("WarpTerminal"));
    }

    #[test]
    fn empty_and_unexpanded_values_are_absent() {
        let o = SessionOrigin::from_headers([
            ("X-Samlu-Warp-Focus", ""),
            ("X-Samlu-Bundle", "%__CFBundleIdentifier%"),
            ("X-Samlu-Term", "  "),
        ]);
        assert_eq!(o, SessionOrigin::default());
    }

    #[test]
    fn values_are_trimmed() {
        let o = SessionOrigin::from_headers([("X-Samlu-Originator", "  Codex Desktop  ")]);
        assert_eq!(o.originator.as_deref(), Some("Codex Desktop"));
    }

    #[test]
    fn merge_keeps_known_fields() {
        let mut old = SessionOrigin {
            host_session: Some("local_ab-1".into()),
            term_program: Some("WarpTerminal".into()),
            ..Default::default()
        };
        let newer = SessionOrigin {
            term_program: Some("Apple_Terminal".into()),
            ..Default::default()
        };
        old.merge(&newer);
        assert_eq!(old.host_session.as_deref(), Some("local_ab-1"));
        assert_eq!(old.term_program.as_deref(), Some("Apple_Terminal"));
    }
}

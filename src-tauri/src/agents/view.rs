//! Finding and opening the window a session lives in.

use super::origin::SessionOrigin;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewTarget {
    Url(String),
    App(String),
    NotFound,
}

const NOT_FOUND: &str = "Couldn't find this session";

/// Picks the best way back to a session. Order matters: a Warp focus URL
/// is the most precise, then the Claude or Codex desktop apps, then the
/// terminal app named by `TERM_PROGRAM`, then the bundle that hosted it.
pub fn resolve(agent: &str, session_id: &str, origin: &SessionOrigin) -> ViewTarget {
    if let Some(url) = &origin.warp_focus_url {
        return ViewTarget::Url(url.clone());
    }
    if origin.entrypoint.as_deref() == Some("claude-desktop") {
        if let Some(host) = origin
            .host_session
            .as_deref()
            .filter(|id| is_host_session(id))
        {
            return ViewTarget::Url(format!(
                "claude://code/continue?session={}",
                percent_encode(host)
            ));
        }
    }
    if agent == "codex" && is_codex_app(origin) {
        return ViewTarget::Url(format!("codex://threads/{}", percent_encode(session_id)));
    }
    if let Some(bundle) = origin.term_program.as_deref().and_then(term_bundle) {
        return ViewTarget::App(bundle.to_string());
    }
    if let Some(bundle) = &origin.bundle_id {
        return ViewTarget::App(bundle.clone());
    }
    ViewTarget::NotFound
}

pub fn open(target: &ViewTarget) -> Result<(), String> {
    match target {
        ViewTarget::Url(url) => crate::open_path(url),
        ViewTarget::App(bundle_id) => {
            if crate::platform::activate_bundle(bundle_id) {
                Ok(())
            } else {
                Err(NOT_FOUND.to_string())
            }
        }
        ViewTarget::NotFound => Err(NOT_FOUND.to_string()),
    }
}

/// Codex runs as its own desktop app, or in a plain shell with no terminal
/// hint at all. A terminal hint means it runs inside that terminal instead.
fn is_codex_app(origin: &SessionOrigin) -> bool {
    origin.bundle_id.as_deref() == Some("com.openai.codex")
        || origin.originator.as_deref() == Some("Codex Desktop")
        || origin.term_program.is_none()
}

/// Claude desktop host sessions look like `local_<id>`. Anything else would
/// let a crafted header build an arbitrary deep link.
fn is_host_session(id: &str) -> bool {
    match id.strip_prefix("local_") {
        Some(rest) => {
            (1..=64).contains(&rest.len())
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }
        None => false,
    }
}

fn term_bundle(term: &str) -> Option<&'static str> {
    Some(match term {
        "Apple_Terminal" => "com.apple.Terminal",
        "iTerm.app" => "com.googlecode.iterm2",
        "vscode" => "com.microsoft.VSCode",
        "ghostty" => "com.mitchellh.ghostty",
        "WarpTerminal" => "dev.warp.Warp-Stable",
        "WezTerm" => "com.github.wez.wezterm",
        _ => return None,
    })
}

/// Percent-encodes everything except unreserved characters (RFC 3986).
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin_with(f: impl FnOnce(&mut SessionOrigin)) -> SessionOrigin {
        let mut origin = SessionOrigin::default();
        f(&mut origin);
        origin
    }

    #[test]
    fn warp_wins() {
        let origin = origin_with(|o| {
            o.warp_focus_url = Some("warp://session/1".into());
            o.entrypoint = Some("claude-desktop".into());
        });
        assert_eq!(
            resolve("claude-code", "s", &origin),
            ViewTarget::Url("warp://session/1".into())
        );
    }

    #[test]
    fn claude_desktop_link() {
        let origin = origin_with(|o| {
            o.entrypoint = Some("claude-desktop".into());
            o.host_session = Some("local_ab-1".into());
        });
        assert_eq!(
            resolve("claude-code", "s", &origin),
            ViewTarget::Url("claude://code/continue?session=local_ab-1".into())
        );
    }

    #[test]
    fn rejects_bad_host_session() {
        for bad in ["local_a b", "x", "local_", "local_a/../b"] {
            let origin = origin_with(|o| {
                o.entrypoint = Some("claude-desktop".into());
                o.host_session = Some(bad.into());
            });
            assert_eq!(
                resolve("claude-code", "s", &origin),
                ViewTarget::NotFound,
                "host {bad:?} should not produce a claude link"
            );
        }
    }

    #[test]
    fn codex_app_by_bundle_or_originator_or_absence() {
        let by_bundle = origin_with(|o| o.bundle_id = Some("com.openai.codex".into()));
        let by_originator = origin_with(|o| o.originator = Some("Codex Desktop".into()));
        let by_absence = SessionOrigin::default();
        for origin in [by_bundle, by_originator, by_absence] {
            assert_eq!(
                resolve("codex", "T", &origin),
                ViewTarget::Url("codex://threads/T".into())
            );
        }
    }

    #[test]
    fn codex_session_id_is_percent_encoded() {
        assert_eq!(
            resolve("codex", "a/b c", &SessionOrigin::default()),
            ViewTarget::Url("codex://threads/a%2Fb%20c".into())
        );
    }

    #[test]
    fn codex_in_warp_terminal_opens_warp() {
        let origin = origin_with(|o| o.term_program = Some("WarpTerminal".into()));
        assert_eq!(
            resolve("codex", "T", &origin),
            ViewTarget::App("dev.warp.Warp-Stable".into())
        );
    }

    #[test]
    fn term_program_maps_to_app() {
        let cases = [
            ("Apple_Terminal", "com.apple.Terminal"),
            ("iTerm.app", "com.googlecode.iterm2"),
            ("vscode", "com.microsoft.VSCode"),
            ("ghostty", "com.mitchellh.ghostty"),
            ("WarpTerminal", "dev.warp.Warp-Stable"),
            ("WezTerm", "com.github.wez.wezterm"),
        ];
        for (term, bundle) in cases {
            let origin = origin_with(|o| o.term_program = Some(term.into()));
            assert_eq!(
                resolve("claude-code", "s", &origin),
                ViewTarget::App(bundle.into()),
                "term {term}"
            );
        }
    }

    #[test]
    fn nothing_known_is_not_found() {
        assert_eq!(
            resolve("claude-code", "s", &SessionOrigin::default()),
            ViewTarget::NotFound
        );
    }
}

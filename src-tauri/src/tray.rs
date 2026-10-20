//! Menu-bar integration. Samlu stays available for global shortcuts and
//! agent notifications when its settings window is closed.

use std::sync::Arc;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::agents::sessions::{SessionInfo, SessionState};
use crate::updater::UpdaterState;

/// The Agents section lists at most this many sessions so a busy machine
/// cannot push the app's own items off the screen.
const MAX_SESSION_ITEMS: usize = 8;

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let menu = build_menu(app.handle(), &[])?;

    // The only status item: tauri.conf.json has no `trayIcon`, which Tauri
    // would add as a second, menu-less one.
    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(tauri::include_image!("icons/icon.png"))
        .icon_as_template(true)
        .tooltip("Samlu")
        .menu(&menu);
    // On Windows a left click on a tray icon opens the app, and the menu is
    // for a right click.
    #[cfg(windows)]
    let tray = tray.show_menu_on_left_click(false);
    let _tray = tray
        .on_menu_event(|app, event| {
            // The handler is installed once, so session rows are routed here
            // by their id prefix rather than by a per-rebuild closure.
            let id: &str = event.id().as_ref();
            if let Some((agent, session_id)) = parse_menu_id(id) {
                crate::agents::hub::view_session(app, &agent, &session_id);
                return;
            }
            match id {
                "show" => show_main(app),
                "update" => crate::updater::check_from_menu(app),
                "setup" => {
                    if let Err(error) = crate::onboarding::show(app) {
                        log::error!("failed to open the setup guide: {error}");
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Shows and focuses the settings window. Also reused by the launcher's
/// "Samlu Settings" command.
pub(crate) fn show_main(app: &AppHandle) {
    if let Err(error) = crate::lifecycle::present_settings(app) {
        log::error!("failed to present Settings: {error}");
    }
}

/// Builds the full tray menu: the Agents section first (only when there are
/// sessions), then the app's own items. The update item is re-registered on
/// every build because a rebuilt menu replaces the one `UpdaterState` holds.
fn build_menu(app: &AppHandle, sessions: &[SessionInfo]) -> tauri::Result<Menu<Wry>> {
    let settings_item = MenuItemBuilder::with_id("show", "Settings…").build(app)?;
    let update_item = MenuItemBuilder::with_id("update", "Check for Updates…").build(app)?;
    let setup_item = MenuItemBuilder::with_id("setup", "Setup Guide…").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Samlu").build(app)?;

    let mut builder = MenuBuilder::new(app);
    if !sessions.is_empty() {
        let header = MenuItemBuilder::with_id("agents_header", "Agents")
            .enabled(false)
            .build(app)?;
        builder = builder.item(&header);
        for session in sessions.iter().take(MAX_SESSION_ITEMS) {
            let label = session_label(&session.agent_label, &session.project_label, session.state);
            let item =
                MenuItemBuilder::with_id(menu_id(&session.agent, &session.session_id), label)
                    .build(app)?;
            builder = builder.item(&item);
        }
        builder = builder.separator();
    }

    let menu = builder
        .item(&settings_item)
        .item(&update_item)
        .item(&setup_item)
        .separator()
        .item(&quit_item)
        .build()?;

    app.state::<Arc<UpdaterState>>()
        .set_menu_item(update_item.clone());

    Ok(menu)
}

/// Rebuilds the Agents section and updates the waiting count shown beside
/// the tray icon.
pub fn refresh_agents(app: &AppHandle, sessions: &[SessionInfo]) {
    let waiting = sessions
        .iter()
        .filter(|session| session.state == SessionState::Waiting)
        .count();

    match build_menu(app, sessions) {
        Ok(menu) => match app.tray_by_id("main-tray") {
            Some(tray) => {
                if let Err(error) = tray.set_menu(Some(menu)) {
                    log::error!("failed to update the tray menu: {error}");
                }
            }
            None => log::error!("tray icon main-tray is missing; agents not shown"),
        },
        Err(error) => log::error!("failed to build the tray menu: {error}"),
    }

    crate::platform::set_tray_count(app, waiting);
}

const SESSION_ID_PREFIX: &str = "session";

/// Menu id for a session row. The agent name never contains `|`, but a
/// session id may, so parsing splits on the first two separators only.
fn menu_id(agent: &str, session_id: &str) -> String {
    format!("{SESSION_ID_PREFIX}|{agent}|{session_id}")
}

/// Inverse of [`menu_id`]. Returns `None` for any id that is not a session row.
fn parse_menu_id(id: &str) -> Option<(String, String)> {
    let mut parts = id.splitn(3, '|');
    if parts.next()? != SESSION_ID_PREFIX {
        return None;
    }
    let agent = parts.next()?;
    let session_id = parts.next()?;
    Some((agent.to_string(), session_id.to_string()))
}

fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Working => "Working",
        SessionState::Waiting => "Waiting",
        SessionState::Idle => "Idle",
    }
}

fn session_label(agent_label: &str, project_label: &str, state: SessionState) -> String {
    format!("{agent_label} — {project_label} · {}", state_label(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_menu_id_round_trips() {
        let id = menu_id("claude", "abc-123");
        assert_eq!(id, "session|claude|abc-123");
        assert_eq!(
            parse_menu_id(&id),
            Some(("claude".to_string(), "abc-123".to_string()))
        );
    }

    #[test]
    fn session_id_containing_separator_is_kept_whole() {
        let id = menu_id("codex", "a|b|c");
        assert_eq!(
            parse_menu_id(&id),
            Some(("codex".to_string(), "a|b|c".to_string()))
        );
    }

    #[test]
    fn non_session_ids_do_not_parse() {
        assert_eq!(parse_menu_id("show"), None);
        assert_eq!(parse_menu_id("update"), None);
        assert_eq!(parse_menu_id("session|claude"), None);
        assert_eq!(parse_menu_id("sessionx|claude|abc"), None);
    }

    #[test]
    fn session_label_matches_spec_format() {
        assert_eq!(
            session_label("Claude Code", "samlu", SessionState::Waiting),
            "Claude Code — samlu · Waiting"
        );
        assert_eq!(state_label(SessionState::Working), "Working");
        assert_eq!(state_label(SessionState::Idle), "Idle");
    }
}

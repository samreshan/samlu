//! Approve/deny global shortcuts, registered only while an approvable card
//! is in front.

use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// The chords currently registered by `arm`, as (approve, deny).
static ARMED: Mutex<Option<(String, String)>> = Mutex::new(None);

fn armed_slot() -> std::sync::MutexGuard<'static, Option<(String, String)>> {
    ARMED.lock().unwrap_or_else(|e| e.into_inner())
}

/// What bringing the registered chords in line with the wanted ones takes.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Nothing,
    Arm,
    Rearm,
}

/// `registered` is whether the OS still holds both chords: something else
/// (suspend, a rebind of another shortcut) may have dropped them.
fn plan(armed: Option<&(String, String)>, approve: &str, deny: &str, registered: bool) -> Plan {
    match armed {
        None => Plan::Arm,
        Some((a, d)) if a == approve && d == deny => {
            if registered {
                Plan::Nothing
            } else {
                Plan::Rearm
            }
        }
        Some(_) => Plan::Rearm,
    }
}

/// Registers both chords; pressing one answers the card in front.
/// Idempotent, and a changed chord replaces the old one.
pub fn arm(app: &AppHandle, approve: &str, deny: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let registered = shortcuts.is_registered(approve) && shortcuts.is_registered(deny);
    let current = armed_slot().clone();
    match plan(current.as_ref(), approve, deny, registered) {
        Plan::Nothing => return Ok(()),
        Plan::Rearm => disarm(app),
        Plan::Arm => {}
    }

    bind(app, approve, true)?;
    if let Err(error) = bind(app, deny, false) {
        let _ = shortcuts.unregister(approve);
        return Err(error);
    }
    *armed_slot() = Some((approve.to_string(), deny.to_string()));
    Ok(())
}

fn bind(app: &AppHandle, chord: &str, approve: bool) -> Result<(), String> {
    app.global_shortcut()
        .on_shortcut(chord, move |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                // The plugin runs this while holding its shortcut table, and
                // answering re-registers shortcuts: hand off and return.
                let app = app.clone();
                std::thread::spawn(move || {
                    let hub = app.state::<Arc<super::hub::AgentHub>>().inner().clone();
                    hub.shortcut_answer(&app, approve);
                });
            }
        })
        .map_err(|error| format!("could not register {chord}: {error}"))
}

/// Unregisters exactly the chords `arm` registered, and nothing else.
pub fn disarm(app: &AppHandle) {
    let taken = armed_slot().take();
    if let Some((approve, deny)) = taken {
        let shortcuts = app.global_shortcut();
        let _ = shortcuts.unregister(approve.as_str());
        let _ = shortcuts.unregister(deny.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(a: &str, d: &str) -> Option<(String, String)> {
        Some((a.to_string(), d.to_string()))
    }

    #[test]
    fn arms_when_nothing_is_armed() {
        assert_eq!(plan(None, "Alt+Cmd+Y", "Alt+Cmd+N", false), Plan::Arm);
    }

    #[test]
    fn same_chords_still_registered_is_a_no_op() {
        let current = armed("Alt+Cmd+Y", "Alt+Cmd+N");
        assert_eq!(
            plan(current.as_ref(), "Alt+Cmd+Y", "Alt+Cmd+N", true),
            Plan::Nothing
        );
    }

    #[test]
    fn dropped_or_changed_chords_are_rearmed() {
        let current = armed("Alt+Cmd+Y", "Alt+Cmd+N");
        assert_eq!(
            plan(current.as_ref(), "Alt+Cmd+Y", "Alt+Cmd+N", false),
            Plan::Rearm
        );
        assert_eq!(
            plan(current.as_ref(), "Alt+Cmd+U", "Alt+Cmd+N", true),
            Plan::Rearm
        );
    }
}

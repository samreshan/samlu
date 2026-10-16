//! Global shortcuts made only of modifier keys, such as ⌃⌥ or ⌥⌘.
//!
//! The global shortcut plugin registers Carbon hot keys, which always need a
//! key code, so these chords are read from a listen-only event tap instead
//! (`native/modifier_keys.m`). Modifiers are also the start of almost every
//! other shortcut, so a chord only counts while nothing else is pressed: a
//! key or click with it held hands the chord back to whatever it was part of.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::voice::Mode;

const CONTROL: u8 = 1;
const OPTION: u8 = 2;
const SHIFT: u8 = 4;
const COMMAND: u8 = 8;

/// Holding ⌃ ⌥ ⇧ passes through ⌃⌥ on the way, so a voice chord waits this
/// long for more modifiers before it starts Normal instead of Summary.
const VOICE_SETTLE: Duration = Duration::from_millis(90);

/// How often to look for an Accessibility grant while a saved chord waits on
/// one, so it starts working without a relaunch.
const ACCESS_POLL: Duration = Duration::from_secs(2);

#[cfg(target_os = "macos")]
extern "C" {
    fn samlu_start_modifier_monitor(handler: extern "C" fn(i32, u32)) -> bool;
}

#[cfg(target_os = "macos")]
const MONITOR_UNAVAILABLE: &str = "To use a shortcut made only of modifier keys, allow Samlu in System Settings > Privacy & Security > Accessibility.";
#[cfg(not(target_os = "macos"))]
const MONITOR_UNAVAILABLE: &str =
    "Samlu couldn't start listening for modifier keys. Add a regular key to the shortcut instead.";

/// Starts the native listener: an event tap on macOS, low-level keyboard and
/// mouse hooks on Windows.
#[cfg(target_os = "macos")]
fn start_monitor() -> bool {
    unsafe { samlu_start_modifier_monitor(on_modifier_event) }
}

#[cfg(windows)]
fn start_monitor() -> bool {
    crate::platform::start_modifier_monitor(on_modifier_event)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn start_monitor() -> bool {
    false
}

/// The modifier mask for `hotkey` when it is made only of modifiers, in the
/// spelling the settings recorder writes ("Ctrl+Alt") or common aliases.
pub fn modifier_mask(hotkey: &str) -> Option<u8> {
    let mut mask = 0;
    for part in hotkey
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        mask |= match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => CONTROL,
            "alt" | "option" => OPTION,
            "shift" => SHIFT,
            "cmd" | "command" | "super" | "meta" => COMMAND,
            _ => return None,
        };
    }
    (mask != 0).then_some(mask)
}

#[derive(Default)]
struct Monitor {
    launcher: Option<u8>,
    voice: Option<u8>,
    /// Modifiers held right now.
    held: u8,
    /// A key or click landed since the modifiers last all came up.
    tainted: bool,
    /// The launcher chord was built up cleanly and is still held.
    launcher_armed: bool,
    /// A voice chord waiting out `VOICE_SETTLE`.
    voice_pending: Option<Pending>,
    /// The voice chord that has been reported as pressed.
    voice_down: Option<u8>,
    generation: u64,
}

#[derive(Clone, Copy)]
struct Pending {
    mask: u8,
    since: Instant,
    generation: u64,
}

enum Action {
    ToggleLauncher,
    VoicePressed(Mode, Instant),
    VoiceReleased,
    VoiceInterrupted,
    SettleVoice(u64),
}

static MONITOR: Mutex<Monitor> = Mutex::new(Monitor {
    launcher: None,
    voice: None,
    held: 0,
    tainted: false,
    launcher_armed: false,
    voice_pending: None,
    voice_down: None,
    generation: 0,
});
static APP: OnceLock<AppHandle> = OnceLock::new();
static WAITING_FOR_ACCESS: AtomicBool = AtomicBool::new(false);

impl Monitor {
    fn voice_mode(&self, mask: u8) -> Option<Mode> {
        let base = self.voice?;
        if mask == base {
            Some(Mode::Normal)
        } else if mask == base | SHIFT && base & SHIFT == 0 {
            Some(Mode::Summarize)
        } else if mask == base | COMMAND && base & COMMAND == 0 {
            Some(Mode::Prompt)
        } else {
            None
        }
    }

    fn voice_masks(&self) -> Vec<u8> {
        self.voice
            .map(|base| vec![base, base | SHIFT, base | COMMAND])
            .unwrap_or_default()
    }

    /// Forgets any chord in progress, e.g. when a binding changes under it.
    fn reset_chords(&mut self) {
        self.launcher_armed = false;
        self.voice_pending = None;
        self.voice_down = None;
        self.generation += 1;
    }

    fn key_or_click(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        // Judged by what is physically held, not the event's own flags: the
        // ⌘V that dictation posts to paste carries ⌘ with nothing held.
        if self.held == 0 {
            return actions;
        }
        self.tainted = true;
        self.launcher_armed = false;
        if self.voice_pending.take().is_some() {
            self.generation += 1;
        }
        if self.voice_down.take().is_some() {
            actions.push(Action::VoiceInterrupted);
        }
        actions
    }

    fn modifiers_changed(&mut self, modifiers: u8) -> Vec<Action> {
        let mut actions = Vec::new();
        let previous = std::mem::replace(&mut self.held, modifiers);
        if modifiers == previous {
            return actions;
        }
        let releasing = |mask: u8| modifiers & !mask == 0;
        // Only a chord built up from fewer modifiers counts, so letting go of
        // ⌃⌥⇧ one key at a time does not pass back through ⌃⌥.
        let building = previous & !modifiers == 0;

        if let Some(launcher) = self.launcher {
            if self.launcher_armed && modifiers != launcher {
                self.launcher_armed = false;
                if releasing(launcher) {
                    actions.push(Action::ToggleLauncher);
                }
            }
            if modifiers == launcher && building && !self.tainted {
                self.launcher_armed = true;
            }
        }

        if let Some(down) = self.voice_down {
            if modifiers != down {
                self.voice_down = None;
                actions.push(if releasing(down) {
                    Action::VoiceReleased
                } else {
                    // ⇧ or ⌘ joined late: drop this take, the fuller chord
                    // starts its own below.
                    Action::VoiceInterrupted
                });
            }
        }
        if let Some(pending) = self.voice_pending {
            if modifiers != pending.mask {
                self.voice_pending = None;
                self.generation += 1;
                // Released before it settled: a quick tap, which voice reads
                // as "start and keep listening".
                if releasing(pending.mask) && !self.tainted {
                    if let Some(mode) = self.voice_mode(pending.mask) {
                        actions.push(Action::VoicePressed(mode, pending.since));
                        actions.push(Action::VoiceReleased);
                    }
                }
            }
        }
        if self.voice_down.is_none()
            && self.voice_pending.is_none()
            && building
            && !self.tainted
            && self.voice_mode(modifiers).is_some()
        {
            self.generation += 1;
            self.voice_pending = Some(Pending {
                mask: modifiers,
                since: Instant::now(),
                generation: self.generation,
            });
            actions.push(Action::SettleVoice(self.generation));
        }

        if modifiers == 0 {
            self.tainted = false;
        }
        actions
    }

    fn settle(&mut self, generation: u64) -> Vec<Action> {
        match self.voice_pending {
            Some(pending) if pending.generation == generation => {
                self.voice_pending = None;
                self.voice_down = Some(pending.mask);
                self.voice_mode(pending.mask)
                    .map(|mode| vec![Action::VoicePressed(mode, pending.since)])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

fn perform(actions: Vec<Action>) {
    let Some(app) = APP.get() else {
        return;
    };
    for action in actions {
        match action {
            Action::ToggleLauncher => {
                let _ = crate::launcher::launcher_toggle(app.clone());
            }
            Action::VoicePressed(mode, since) => crate::voice::voice_pressed_at(app, mode, since),
            Action::VoiceReleased => crate::voice::voice_released(app),
            Action::VoiceInterrupted => crate::voice::chord_interrupted(app),
            Action::SettleVoice(generation) => {
                let app = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(VOICE_SETTLE);
                    let actions = MONITOR.lock().unwrap().settle(generation);
                    if !actions.is_empty() {
                        let _ = app.run_on_main_thread(move || perform(actions));
                    }
                });
            }
        }
    }
}

extern "C" fn on_modifier_event(kind: i32, modifiers: u32) {
    let modifiers = (modifiers & 0xF) as u8;
    let actions = {
        let mut monitor = MONITOR.lock().unwrap();
        if kind == 0 {
            monitor.modifiers_changed(modifiers)
        } else {
            monitor.key_or_click()
        }
    };
    perform(actions);
}

fn ensure_started(app: &AppHandle) -> Result<(), String> {
    let _ = APP.set(app.clone());
    if start_monitor() {
        return Ok(());
    }
    crate::platform::request_accessibility_access();
    wait_for_access();
    Err(MONITOR_UNAVAILABLE.to_string())
}

fn anything_bound() -> bool {
    let monitor = MONITOR.lock().unwrap();
    monitor.launcher.is_some() || monitor.voice.is_some()
}

/// The tap can only be created once Samlu is allowed under Accessibility, and
/// macOS says nothing when that happens, so check now and then while a chord
/// is bound and start the tap as soon as it is. Stops once the tap runs, or
/// when nothing is bound any more; binding again starts a new watch.
fn wait_for_access() {
    if WAITING_FOR_ACCESS.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(ACCESS_POLL);
        if !anything_bound() {
            WAITING_FOR_ACCESS.store(false, Ordering::Release);
            // A bind that landed just now saw this watch still running.
            if anything_bound() && !WAITING_FOR_ACCESS.swap(true, Ordering::AcqRel) {
                continue;
            }
            return;
        }
        if crate::voice::get_voice_accessibility_status() && start_monitor() {
            log::info!("Accessibility granted; modifier-only shortcuts are now active");
            WAITING_FOR_ACCESS.store(false, Ordering::Release);
            return;
        }
    });
}

/// Records the chord even when the tap cannot start yet, so it comes alive
/// by itself once Accessibility is granted.
fn bind(
    app: &AppHandle,
    hotkey: &str,
    apply: impl FnOnce(&mut Monitor, u8) -> Result<(), String>,
) -> Result<(), String> {
    let mask = modifier_mask(hotkey).ok_or_else(|| format!("{hotkey} is not a modifier chord"))?;
    if mask.count_ones() < 2 {
        return Err("Use at least two modifier keys, or add a regular key.".to_string());
    }
    {
        let mut monitor = MONITOR.lock().unwrap();
        apply(&mut monitor, mask)?;
        monitor.reset_chords();
    }
    ensure_started(app)
}

pub fn bind_launcher(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    bind(app, hotkey, |monitor, mask| {
        if monitor.voice_masks().contains(&mask) {
            return Err("That shortcut is already used for dictation.".to_string());
        }
        monitor.launcher = Some(mask);
        Ok(())
    })
}

pub fn bind_voice(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    bind(app, hotkey, |monitor, mask| {
        if monitor
            .launcher
            .is_some_and(|launcher| [mask, mask | SHIFT, mask | COMMAND].contains(&launcher))
        {
            return Err("That shortcut is already used for the launcher.".to_string());
        }
        monitor.voice = Some(mask);
        Ok(())
    })
}

pub fn unbind_launcher() {
    let mut monitor = MONITOR.lock().unwrap();
    monitor.launcher = None;
    monitor.reset_chords();
}

pub fn unbind_voice() {
    let mut monitor = MONITOR.lock().unwrap();
    monitor.voice = None;
    monitor.reset_chords();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(launcher: Option<&str>, voice: Option<&str>) -> Monitor {
        Monitor {
            launcher: launcher.and_then(modifier_mask),
            voice: voice.and_then(modifier_mask),
            ..Monitor::default()
        }
    }

    fn names(actions: &[Action]) -> Vec<&'static str> {
        actions
            .iter()
            .map(|action| match action {
                Action::ToggleLauncher => "launcher",
                Action::VoicePressed(Mode::Normal, _) => "normal",
                Action::VoicePressed(Mode::Summarize, _) => "summary",
                Action::VoicePressed(Mode::Prompt, _) => "prompt",
                Action::VoiceReleased => "released",
                Action::VoiceInterrupted => "interrupted",
                Action::SettleVoice(_) => "settle",
            })
            .collect()
    }

    fn settle_pending(monitor: &mut Monitor) -> Vec<&'static str> {
        let generation = monitor
            .voice_pending
            .expect("a voice chord is pending")
            .generation;
        names(&monitor.settle(generation))
    }

    #[test]
    fn parses_modifier_only_chords() {
        assert_eq!(modifier_mask("Ctrl+Alt"), Some(CONTROL | OPTION));
        assert_eq!(modifier_mask("Alt+Cmd"), Some(OPTION | COMMAND));
        assert_eq!(modifier_mask("Control+Option"), Some(CONTROL | OPTION));
        assert_eq!(modifier_mask("Alt+V"), None);
        assert_eq!(modifier_mask("F5"), None);
        assert_eq!(modifier_mask(""), None);
    }

    #[test]
    fn launcher_toggles_when_a_clean_chord_is_released() {
        let mut monitor = monitor(Some("Alt+Cmd"), None);
        assert!(monitor.modifiers_changed(OPTION).is_empty());
        assert!(monitor.modifiers_changed(OPTION | COMMAND).is_empty());
        assert_eq!(names(&monitor.modifiers_changed(OPTION)), ["launcher"]);
        assert!(monitor.modifiers_changed(0).is_empty());
    }

    #[test]
    fn launcher_ignores_the_chord_inside_another_shortcut() {
        let mut monitor = monitor(Some("Alt+Cmd"), None);
        monitor.modifiers_changed(COMMAND);
        monitor.modifiers_changed(OPTION | COMMAND);
        // ⌥⌘Esc, Force Quit.
        assert!(monitor.key_or_click().is_empty());
        assert!(monitor.modifiers_changed(COMMAND).is_empty());
        assert!(monitor.modifiers_changed(0).is_empty());
        // Adding ⇧ for another shortcut does not fire either.
        monitor.modifiers_changed(OPTION | COMMAND);
        monitor.modifiers_changed(OPTION | COMMAND | SHIFT);
        assert!(monitor.modifiers_changed(OPTION | COMMAND).is_empty());
        assert!(monitor.modifiers_changed(0).is_empty());
    }

    #[test]
    fn voice_holds_and_releases_a_settled_chord() {
        let mut monitor = monitor(None, Some("Ctrl+Alt"));
        monitor.modifiers_changed(CONTROL);
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION)),
            ["settle"]
        );
        assert_eq!(settle_pending(&mut monitor), ["normal"]);
        assert_eq!(names(&monitor.modifiers_changed(CONTROL)), ["released"]);
    }

    #[test]
    fn voice_waits_for_shift_or_command_before_choosing_a_mode() {
        let mut monitor = monitor(None, Some("Ctrl+Alt"));
        monitor.modifiers_changed(CONTROL);
        monitor.modifiers_changed(CONTROL | OPTION);
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION | SHIFT)),
            ["settle"]
        );
        assert_eq!(settle_pending(&mut monitor), ["summary"]);
        // Letting go of ⇧ first ends Summary without starting Normal.
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION)),
            ["released"]
        );
        assert!(monitor.voice_pending.is_none());

        monitor.modifiers_changed(0);
        monitor.modifiers_changed(COMMAND);
        monitor.modifiers_changed(COMMAND | CONTROL);
        monitor.modifiers_changed(COMMAND | CONTROL | OPTION);
        assert_eq!(settle_pending(&mut monitor), ["prompt"]);
    }

    #[test]
    fn voice_quick_tap_still_registers() {
        let mut monitor = monitor(None, Some("Ctrl+Alt"));
        monitor.modifiers_changed(CONTROL | OPTION);
        assert_eq!(
            names(&monitor.modifiers_changed(OPTION)),
            ["normal", "released"]
        );
    }

    #[test]
    fn voice_gives_way_to_a_key_pressed_with_the_chord() {
        let mut monitor = monitor(None, Some("Ctrl+Alt"));
        monitor.modifiers_changed(CONTROL | OPTION);
        settle_pending(&mut monitor);
        assert_eq!(names(&monitor.key_or_click()), ["interrupted"]);
        assert!(monitor.modifiers_changed(CONTROL).is_empty());

        // While tainted, pressing the chord again does nothing until all
        // modifiers come up.
        assert!(monitor.modifiers_changed(CONTROL | OPTION).is_empty());
        monitor.modifiers_changed(0);
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION)),
            ["settle"]
        );
    }

    #[test]
    fn a_posted_paste_does_not_block_the_next_chord() {
        let mut monitor = monitor(Some("Alt+Cmd"), Some("Ctrl+Alt"));
        assert!(monitor.key_or_click().is_empty());
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION)),
            ["settle"]
        );
        monitor.modifiers_changed(0);
        monitor.modifiers_changed(OPTION | COMMAND);
        assert_eq!(names(&monitor.modifiers_changed(0)), ["launcher"]);
    }

    #[test]
    fn late_shift_restarts_voice_in_summary() {
        let mut monitor = monitor(None, Some("Ctrl+Alt"));
        monitor.modifiers_changed(CONTROL | OPTION);
        settle_pending(&mut monitor);
        assert_eq!(
            names(&monitor.modifiers_changed(CONTROL | OPTION | SHIFT)),
            ["interrupted", "settle"]
        );
        assert_eq!(settle_pending(&mut monitor), ["summary"]);
    }
}

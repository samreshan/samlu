//! Delivers the final transcribed/transformed text to whatever app currently
//! has focus: write it to the system clipboard, then simulate Cmd+V so it
//! lands right where the cursor is — the clipboard+simulated-paste approach
//! chosen for this feature (vs. an accessibility-API text-insertion
//! approach, which would need a per-app text-field target rather than "just
//! paste").
//!
//! Target capture, Space restoration, and Cmd+V are performed by native macOS
//! APIs from Samlu's own process. That keeps the permission identity coherent
//! and avoids System Events/AppleScript failure modes.

use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

const TARGET_RESTORE_TIMEOUT: Duration = Duration::from_millis(1_200);
const ACCESSIBILITY_ERROR: &str =
    "Copied to clipboard, but Samlu doesn't have Accessibility permission yet";

#[derive(Clone, Debug, Default)]
pub struct TargetApp {
    pub bundle_id: Option<String>,
    pub pid: Option<i32>,
}

/// Requires Accessibility permission for Samlu itself when bundled, or for
/// the terminal running it in development. The caller preserves the text on
/// the clipboard before invoking this function.
fn simulate_paste() -> Result<(), String> {
    log::info!("[voice] paste: checking accessibility trust…");
    let trusted = accessibility_trusted();
    log::info!("[voice] paste: accessibility trusted = {trusted}");
    if !trusted {
        return Err(format!(
            "{ACCESSIBILITY_ERROR} (needed to simulate the paste keystroke) — grant it in \
             System Settings > Privacy & Security > Accessibility, then paste manually with \
             Cmd+V this time."
        ));
    }

    log::info!("[voice] paste: posting native Cmd+V");
    if !crate::macos::post_paste_shortcut() {
        return Err(format!(
            "{ACCESSIBILITY_ERROR} because macOS rejected the paste shortcut — paste manually \
             with Cmd+V."
        ));
    }

    Ok(())
}

pub fn copy(app: &AppHandle, text: &str) -> Result<(), String> {
    log::info!("[voice] paste: writing clipboard…");
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|error| format!("couldn't write to clipboard: {error}"))?;
    log::info!("[voice] paste: clipboard written");
    Ok(())
}

pub fn is_accessibility_error(error: &str) -> bool {
    error.contains(ACCESSIBILITY_ERROR)
}

pub fn frontmost_target() -> TargetApp {
    let Some(application) = crate::macos::frontmost_application() else {
        return TargetApp::default();
    };
    if application.bundle_id.as_deref() == Some("com.samlu.desktop") {
        return TargetApp::default();
    }
    TargetApp {
        bundle_id: application.bundle_id,
        pid: Some(application.pid),
    }
}

pub fn inject_to_bundle(app: &AppHandle, text: &str, target: &TargetApp) -> Result<(), String> {
    // Preserve the result before interacting with external processes. Even if
    // restoring focus or simulating Cmd+V times out, the user can paste it.
    copy(app, text)?;
    restore_target(target)?;
    simulate_paste()
}

fn restore_target(target: &TargetApp) -> Result<(), String> {
    let Some(pid) = target.pid else {
        return Ok(());
    };
    if target_is_frontmost(target) {
        return Ok(());
    }
    if !crate::macos::activate_application(pid, target.bundle_id.as_deref()) {
        return Err("could not restore the app where dictation began".to_string());
    }
    let started = Instant::now();
    while started.elapsed() < TARGET_RESTORE_TIMEOUT {
        if target_is_frontmost(target) {
            std::thread::sleep(Duration::from_millis(45));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("timed out restoring the app where dictation began".to_string())
}

fn target_is_frontmost(target: &TargetApp) -> bool {
    let Some(current) = crate::macos::frontmost_application() else {
        return false;
    };
    target_matches_application(target, &current)
}

fn target_matches_application(
    target: &TargetApp,
    current: &crate::macos::FrontmostApplication,
) -> bool {
    target.pid == Some(current.pid)
        || target
            .bundle_id
            .as_deref()
            .zip(current.bundle_id.as_deref())
            .is_some_and(|(expected, actual)| expected == actual)
}

/// FFI check against macOS's own Accessibility-trust API — checked ourselves
/// rather than just trying the paste and hoping for the best, since a
/// privacy-gated operation is worth confirming explicitly rather than
/// discovering via however it happens to fail.
pub fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_identity_matches_pid_or_bundle() {
        let target = TargetApp {
            pid: Some(42),
            bundle_id: Some("dev.example.Editor".to_string()),
        };
        let same_pid = crate::macos::FrontmostApplication {
            pid: 42,
            bundle_id: Some("dev.other.Editor".to_string()),
        };
        let same_bundle = crate::macos::FrontmostApplication {
            pid: 84,
            bundle_id: Some("dev.example.Editor".to_string()),
        };
        let different = crate::macos::FrontmostApplication {
            pid: 84,
            bundle_id: Some("dev.other.Editor".to_string()),
        };
        assert!(target_matches_application(&target, &same_pid));
        assert!(target_matches_application(&target, &same_bundle));
        assert!(!target_matches_application(&target, &different));
    }

    #[test]
    fn accessibility_failures_are_classified_for_clipboard_fallback() {
        let error = format!("{ACCESSIBILITY_ERROR} — paste manually.");
        assert!(is_accessibility_error(&error));
        assert!(!is_accessibility_error("could not restore the target app"));
    }
}

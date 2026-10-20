//! Small AppKit behaviors that Tauri does not expose directly.

use std::ffi::CString;
use tauri::WebviewWindow;

extern "C" {
    fn samlu_begin_overlay_window();
    fn samlu_end_overlay_window();
    fn samlu_configure_overlay_window(window: *mut std::ffi::c_void);
    fn samlu_present_focusable_overlay_at_cursor(
        window: *mut std::ffi::c_void,
        width: f64,
        height: f64,
        top_fraction: f64,
    );
    fn samlu_focus_overlay(window: *mut std::ffi::c_void);
    fn samlu_make_overlay_key(window: *mut std::ffi::c_void);
    fn samlu_resize_overlay_keeping_top(window: *mut std::ffi::c_void, height: f64);
    fn samlu_active_display_has_notch() -> bool;
    fn samlu_active_screen_frame(out: *mut f64) -> bool;
    fn samlu_set_overlay_frame(
        window: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        present: bool,
    );
    fn samlu_center_overlay_at_top(window: *mut std::ffi::c_void);
    fn samlu_order_front_without_activating(window: *mut std::ffi::c_void);
    fn samlu_set_ignores_mouse_events(window: *mut std::ffi::c_void, ignores: bool);
    fn samlu_set_background_mode(background_mode: bool);
    fn samlu_present_application_window(window: *mut std::ffi::c_void) -> bool;
    fn samlu_microphone_authorization() -> i32;
    fn samlu_request_microphone_access();
    fn samlu_request_accessibility_access() -> bool;
    fn samlu_frontmost_application(bundle_buffer: *mut i8, bundle_capacity: usize) -> i32;
    fn samlu_activate_application(pid: i32, bundle_identifier: *const i8) -> bool;
    fn samlu_post_paste_shortcut() -> bool;
    fn samlu_is_developer_id_signed() -> bool;
}

#[derive(Clone, Debug)]
pub struct FrontmostApplication {
    pub pid: i32,
    pub bundle_id: Option<String>,
}

/// How macOS currently answers "may Samlu record audio?".
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneAccess {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

impl MicrophoneAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotDetermined => "not_determined",
            Self::Restricted => "restricted",
            Self::Denied => "denied",
            Self::Authorized => "authorized",
        }
    }
}

pub fn microphone_access() -> MicrophoneAccess {
    match unsafe { samlu_microphone_authorization() } {
        0 => MicrophoneAccess::NotDetermined,
        2 => MicrophoneAccess::Denied,
        3 => MicrophoneAccess::Authorized,
        _ => MicrophoneAccess::Restricted,
    }
}

/// Triggers the system microphone prompt. Once the user has answered, macOS
/// never shows it again, so callers must fall back to System Settings.
pub fn request_microphone_access() {
    unsafe { samlu_request_microphone_access() };
}

pub fn request_accessibility_access() -> bool {
    unsafe { samlu_request_accessibility_access() }
}

pub fn frontmost_application() -> Option<FrontmostApplication> {
    let mut bundle = [0_i8; 512];
    let pid = unsafe { samlu_frontmost_application(bundle.as_mut_ptr(), bundle.len()) };
    if pid <= 0 {
        return None;
    }
    let bundle_id = unsafe { std::ffi::CStr::from_ptr(bundle.as_ptr()) }
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(FrontmostApplication { pid, bundle_id })
}

pub fn activate_application(pid: i32, bundle_id: Option<&str>) -> bool {
    let bundle = bundle_id.and_then(|value| CString::new(value).ok());
    unsafe {
        samlu_activate_application(
            pid,
            bundle
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
        )
    }
}

/// Plays `path` at `volume` (0-100).
/// Blocks until the sound ends, so call it from a worker thread.
pub fn play_sound(path: &std::path::Path, volume: u8) {
    let volume = f32::from(volume.min(100)) / 100.0;
    let _ = std::process::Command::new("/usr/bin/afplay")
        .arg("-v")
        .arg(format!("{volume:.2}"))
        .arg(path)
        .status();
}

/// Folder holding the system alert sounds, if this platform has one.
pub fn system_sound_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("/System/Library/Sounds"))
}

/// Brings the app with `bundle_id` to the front. `false` when `open -b`
/// fails (no such app), so View can say it couldn't find the session.
pub fn activate_bundle(bundle_id: &str) -> bool {
    // `open -b` asks LaunchServices to bring the app forward, launching it if
    // needed. It runs on its own thread so a stuck LaunchServices cannot hold
    // the caller (which may be the main thread) for more than a moment; an
    // answer that has not come in 3 s is taken as success, the app is on its way.
    let bundle_id = bundle_id.to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ok = std::process::Command::new("/usr/bin/open")
            .arg("-b")
            .arg(&bundle_id)
            .status()
            .is_ok_and(|status| status.success());
        let _ = sender.send(ok);
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or(true)
}

/// Shows how many agents wait: tray title on macOS, tooltip on Windows.
pub fn set_tray_count(app: &tauri::AppHandle, count: usize) {
    // `None` leaves the old title on the status item, so clear with "".
    let title = if count > 0 {
        count.to_string()
    } else {
        String::new()
    };
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_title(Some(title));
    }
}

pub fn post_paste_shortcut() -> bool {
    unsafe { samlu_post_paste_shortcut() }
}

/// Signed with a Developer ID rather than ad hoc. See `native/signing.m`.
pub fn is_developer_id_signed() -> bool {
    unsafe { samlu_is_developer_id_signed() }
}

/// Builds an overlay window as a nonactivating panel, which is what lets it
/// take the keyboard over any app, full screen ones included, without
/// activating Samlu. `build` must create the window on the main thread.
pub fn create_overlay<T>(build: impl FnOnce() -> T) -> T {
    unsafe { samlu_begin_overlay_window() };
    let window = build();
    unsafe { samlu_end_overlay_window() };
    window
}

pub fn configure_overlay(window: &WebviewWindow) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_configure_overlay_window(pointer) };
    }
}

/// A display frame in AppKit points: bottom-left origin on the primary
/// display, y growing upward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The display holding the frontmost app's frontmost window, falling back to
/// the one under the pointer.
pub fn active_screen_frame() -> Option<ScreenFrame> {
    let mut out = [0.0f64; 4];
    if !unsafe { samlu_active_screen_frame(out.as_mut_ptr()) } {
        return None;
    }
    Some(ScreenFrame {
        x: out[0],
        y: out[1],
        width: out[2],
        height: out[3],
    })
}

/// Places an overlay's content rect in AppKit points, and with `present`
/// orders it in on the current Space without activating Samlu.
pub fn set_overlay_frame(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    present: bool,
) -> bool {
    let Ok(pointer) = window.ns_window() else {
        return false;
    };
    unsafe { samlu_set_overlay_frame(pointer, x, y, width, height, present) };
    true
}

/// Hangs an overlay from the top centre of the active display.
pub fn center_overlay_at_top(window: &WebviewWindow) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_center_overlay_at_top(pointer) };
    }
}

/// Presents and focuses a keyboard-driven overlay on the current Space, full
/// screen Spaces included, without activating Samlu.
pub fn present_focusable_overlay_at_cursor(
    window: &WebviewWindow,
    width: f64,
    height: f64,
    top_fraction: f64,
) -> bool {
    let Ok(pointer) = window.ns_window() else {
        return false;
    };
    unsafe {
        samlu_present_focusable_overlay_at_cursor(pointer, width, height, top_fraction);
    }
    true
}

pub fn focus_overlay(window: &WebviewWindow) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_focus_overlay(pointer) };
    }
}

/// Gives a visible overlay keyboard focus again without activating Samlu.
/// Tauri's `set_focus` activates the app, which switches to the Space where
/// Settings was last open.
pub fn make_overlay_key(window: &WebviewWindow) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_make_overlay_key(pointer) };
    }
}

/// Whether the display under the pointer has a camera housing, so surfaces
/// anchored to the top edge can decide whether to acknowledge it.
pub fn active_display_has_notch() -> bool {
    unsafe { samlu_active_display_has_notch() }
}

/// Resizes an overlay to `height` logical points while its top edge stays put.
/// On the main thread this has happened by the time it returns.
pub fn resize_overlay_keeping_top(window: &WebviewWindow, height: f64) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_resize_overlay_keeping_top(pointer, height) };
    }
}

/// While `true` the window is transparent to clicks, so a floating surface
/// that only reports status never swallows a click meant for the app below.
pub fn set_ignores_mouse_events(window: &WebviewWindow, ignores: bool) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_set_ignores_mouse_events(pointer, ignores) };
    }
}

pub fn order_front_without_activating(window: &WebviewWindow) {
    if let Ok(pointer) = window.ns_window() {
        unsafe { samlu_order_front_without_activating(pointer) };
    }
}

pub fn set_background_mode(background_mode: bool) {
    unsafe { samlu_set_background_mode(background_mode) };
}

/// Presents a normal, focusable application window without racing AppKit's
/// activation-policy transition against Tauri's asynchronous window commands.
pub fn present_application_window(window: &WebviewWindow) -> bool {
    match window.ns_window() {
        Ok(pointer) => unsafe { samlu_present_application_window(pointer) },
        Err(_) => false,
    }
}

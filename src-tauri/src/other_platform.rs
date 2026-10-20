//! Platforms Samlu doesn't ship on. Linux builds exist so the portable code
//! can be built and tested anywhere; every native behaviour here reports
//! itself unavailable instead.

use tauri::WebviewWindow;

#[derive(Clone, Debug)]
pub struct FrontmostApplication {
    pub pid: i32,
    pub bundle_id: Option<String>,
}

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
    MicrophoneAccess::Authorized
}

pub fn request_microphone_access() {}

pub fn request_accessibility_access() -> bool {
    false
}

pub fn frontmost_application() -> Option<FrontmostApplication> {
    None
}

pub fn activate_application(_pid: i32, _bundle_id: Option<&str>) -> bool {
    false
}

/// Plays `path` at `volume` (0-100). Wired in Task 5.
#[allow(dead_code)] // wired in Task 12
pub fn play_sound(path: &std::path::Path, volume: u8) {
    let _ = (path, volume);
}

/// Folder holding the system alert sounds, if this platform has one.
#[allow(dead_code)] // wired in Task 12
pub fn system_sound_dir() -> Option<std::path::PathBuf> {
    None
}

/// Brings the app with `bundle_id` to the front. Wired in Task 3.
#[allow(dead_code)] // wired in Task 12
pub fn activate_bundle(bundle_id: &str) -> bool {
    let _ = bundle_id;
    false
}

/// Shows how many agents wait: tray title on macOS, tooltip on Windows.
#[allow(dead_code)] // wired in Task 12
pub fn set_tray_count(app: &tauri::AppHandle, count: usize) {
    let _ = (app, count);
}

pub fn post_paste_shortcut() -> bool {
    false
}

pub fn is_developer_id_signed() -> bool {
    true
}

pub fn create_overlay<T>(build: impl FnOnce() -> T) -> T {
    build()
}

pub fn configure_overlay(_window: &WebviewWindow) {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn active_screen_frame() -> Option<ScreenFrame> {
    None
}

pub fn set_overlay_frame(
    _window: &WebviewWindow,
    _x: f64,
    _y: f64,
    _width: f64,
    _height: f64,
    _present: bool,
) -> bool {
    false
}

pub fn center_overlay_at_top(_window: &WebviewWindow) {}

pub fn present_focusable_overlay_at_cursor(
    _window: &WebviewWindow,
    _width: f64,
    _height: f64,
    _top_fraction: f64,
) -> bool {
    false
}

pub fn focus_overlay(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn make_overlay_key(window: &WebviewWindow) {
    let _ = window.set_focus();
}

pub fn active_display_has_notch() -> bool {
    false
}

pub fn resize_overlay_keeping_top(_window: &WebviewWindow, _height: f64) {}

pub fn set_ignores_mouse_events(window: &WebviewWindow, ignores: bool) {
    let _ = window.set_ignore_cursor_events(ignores);
}

pub fn order_front_without_activating(window: &WebviewWindow) {
    let _ = window.show();
}

pub fn set_background_mode(_background_mode: bool) {}

pub fn present_application_window(window: &WebviewWindow) -> bool {
    let _ = window.show();
    let _ = window.set_focus();
    true
}

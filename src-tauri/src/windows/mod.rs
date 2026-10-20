//! Windows counterparts of `macos.rs`: overlay windows, the foreground app,
//! the paste keystroke and microphone privacy, built on Win32.
//!
//! Overlay frames keep the macOS shape (logical points, y growing upward) so
//! the placement math in `voice` is shared. Every display is measured in its
//! own scale, and a frame maps back to pixels through the display holding it,
//! which keeps windows on the right screen when displays mix scales.

mod credentials;
mod hooks;

pub use credentials::{delete_secret, read_secret, write_secret};
pub use hooks::{cancel_color_pick, pick_color, start_modifier_monitor};

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use tauri::WebviewWindow;
use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ,
    RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GW_OWNER,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE,
    SW_SHOW, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// What `frontmost_application` reports for Samlu itself, matching its
/// bundle identifier on macOS so callers can skip it the same way.
const SELF_IDENTIFIER: &str = "com.samlu.desktop";

const MICROPHONE_CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

static MAIN_THREAD: OnceLock<u32> = OnceLock::new();

/// Records the UI thread. Called once from setup, which runs on it.
pub fn init() {
    let _ = MAIN_THREAD.set(unsafe { GetCurrentThreadId() });
}

fn on_main_thread() -> bool {
    MAIN_THREAD
        .get()
        .is_some_and(|thread| *thread == unsafe { GetCurrentThreadId() })
}

/// Runs `work` on the UI thread: inline when already there, otherwise queued
/// behind the event loop, as macOS dispatches to its main queue.
fn on_main(window: &WebviewWindow, work: impl FnOnce() + Send + 'static) {
    if on_main_thread() {
        work();
    } else {
        let _ = window.run_on_main_thread(work);
    }
}

/// The native handle as a plain number, so it can move between threads.
fn handle(window: &WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|hwnd| hwnd.0 as isize)
}

fn hwnd(raw: isize) -> HWND {
    raw as HWND
}

#[derive(Clone, Debug)]
pub struct FrontmostApplication {
    pub pid: i32,
    /// The executable's full path; macOS has the bundle identifier here.
    pub bundle_id: Option<String>,
}

/// How Windows privacy settings answer "may Samlu record audio?".
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

/// Desktop apps are never prompted on Windows; the answer is the privacy
/// switches in Settings. The device-wide switch is only an administrator's
/// to change, so it reads as restricted.
pub fn microphone_access() -> MicrophoneAccess {
    let denied = |root: HKEY, subkey: &str| {
        registry_string(root, subkey, "Value").as_deref() == Some("Deny")
    };
    if denied(HKEY_LOCAL_MACHINE, MICROPHONE_CONSENT) {
        return MicrophoneAccess::Restricted;
    }
    if denied(HKEY_CURRENT_USER, MICROPHONE_CONSENT)
        || denied(
            HKEY_CURRENT_USER,
            &format!(r"{MICROPHONE_CONSENT}\NonPackaged"),
        )
    {
        return MicrophoneAccess::Denied;
    }
    MicrophoneAccess::Authorized
}

/// Windows has no prompt to trigger; callers fall back to Settings.
pub fn request_microphone_access() {}

/// Simulated keystrokes and global hooks need no grant on Windows.
pub fn request_accessibility_access() -> bool {
    true
}

pub fn frontmost_application() -> Option<FrontmostApplication> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid == 0 {
        return None;
    }
    let bundle_id = if pid == unsafe { GetCurrentProcessId() } {
        Some(SELF_IDENTIFIER.to_string())
    } else {
        process_image(pid)
    };
    Some(FrontmostApplication {
        pid: pid as i32,
        bundle_id,
    })
}

/// Brings back the window the user was typing in: the topmost window of
/// `pid`, or, if that process is gone, of another running copy of the same
/// executable.
pub fn activate_application(pid: i32, bundle_id: Option<&str>) -> bool {
    let window = main_window_of(&|candidate| candidate == pid as u32).or_else(|| {
        let image = bundle_id?;
        main_window_of(&|candidate| {
            process_image(candidate).is_some_and(|path| path.eq_ignore_ascii_case(image))
        })
    });
    match window {
        Some(window) => bring_to_front(window),
        None => false,
    }
}

/// Plays `path` at `volume` (0-100).
pub fn play_sound(path: &std::path::Path, volume: u8) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Media::Audio::{
        waveOutSetVolume, PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT,
    };

    // waveOut takes the left channel in the low word and the right in the high.
    let v = u32::from(volume.min(100)) * 0xFFFF / 100;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: the default device is requested with a null handle, and `wide`
    // is NUL-terminated and outlives the call. SND_ASYNC returns at once.
    unsafe {
        waveOutSetVolume(std::ptr::null_mut(), v | (v << 16));
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// Folder holding the system alert sounds, if this platform has one.
pub fn system_sound_dir() -> Option<std::path::PathBuf> {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    Some(std::path::PathBuf::from(windir).join("Media"))
}

/// Brings the app with `bundle_id` to the front.
pub fn activate_bundle(bundle_id: &str) -> bool {
    let _ = bundle_id;
    false
}

/// Shows how many agents wait: tray title on macOS, tooltip on Windows.
pub fn set_tray_count(app: &tauri::AppHandle, count: usize) {
    let tooltip = match count {
        0 => "Samlu".to_string(),
        1 => "Samlu · 1 agent waiting".to_string(),
        n => format!("Samlu · {n} agents waiting"),
    };
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

/// Posts Ctrl+V from Samlu's own process.
pub fn post_paste_shortcut() -> bool {
    const V: u16 = 0x56;
    let press = [key(VK_CONTROL, false), key(V, false)];
    let release = [key(V, true), key(VK_CONTROL, true)];
    let sent = send_keys(&press);
    // The same gap macOS leaves between the halves of the keystroke.
    std::thread::sleep(std::time::Duration::from_millis(12));
    sent && send_keys(&release)
}

fn key(code: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: code,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_keys(inputs: &[INPUT]) -> bool {
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    sent as usize == inputs.len()
}

/// Updates keep the app's identity on Windows whatever signs them, so the
/// macOS warning about permissions resetting after an update never applies.
pub fn is_developer_id_signed() -> bool {
    true
}

pub fn create_overlay<T>(build: impl FnOnce() -> T) -> T {
    build()
}

/// Keeps an overlay above other windows and out of the taskbar and Alt+Tab.
pub fn configure_overlay(window: &WebviewWindow) {
    let _ = window.set_always_on_top(true);
    let _ = window.set_skip_taskbar(true);
    if let Some(raw) = handle(window) {
        on_main(window, move || unsafe {
            let style = GetWindowLongPtrW(hwnd(raw), GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd(raw),
                GWL_EXSTYLE,
                (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize),
            );
        });
    }
}

/// A display frame in logical points with y growing upward, as on macOS.
/// It covers the display's work area, so nothing lands under the taskbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One display: its work area in pixels and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Display {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    scale: f64,
}

impl Display {
    fn frame(&self) -> ScreenFrame {
        let scale = self.scale;
        ScreenFrame {
            x: f64::from(self.left) / scale,
            y: -f64::from(self.bottom) / scale,
            width: f64::from(self.right - self.left) / scale,
            height: f64::from(self.bottom - self.top) / scale,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let frame = self.frame();
        x >= frame.x && x < frame.x + frame.width && y >= frame.y && y < frame.y + frame.height
    }

    /// Pixels (left, top, width, height) for a rect given in this display's
    /// points.
    fn pixels_for(&self, x: f64, y: f64, width: f64, height: f64) -> (i32, i32, i32, i32) {
        let scale = self.scale;
        let frame = self.frame();
        let left = f64::from(self.left) + (x - frame.x) * scale;
        let top = f64::from(self.top) + (frame.y + frame.height - (y + height)) * scale;
        (
            left.round() as i32,
            top.round() as i32,
            (width * scale).round() as i32,
            (height * scale).round() as i32,
        )
    }
}

fn display_info(monitor: HMONITOR) -> Option<Display> {
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let work = info.rcWork;
    Some(Display {
        left: work.left,
        top: work.top,
        right: work.right,
        bottom: work.bottom,
        scale: if dpi_x == 0 {
            1.0
        } else {
            f64::from(dpi_x) / 96.0
        },
    })
}

fn displays() -> Vec<Display> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let list = &mut *(data as *mut Vec<Display>);
        if let Some(display) = display_info(monitor) {
            list.push(display);
        }
        1
    }
    let mut list: Vec<Display> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut list as *mut Vec<Display> as LPARAM,
        )
    };
    list
}

/// The display holding the foreground window, falling back to the one under
/// the pointer.
fn active_display() -> Option<Display> {
    let foreground = unsafe { GetForegroundWindow() };
    let mut monitor = if foreground.is_null() {
        null_mut()
    } else {
        unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL) }
    };
    if monitor.is_null() {
        let mut point = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut point) };
        monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    }
    display_info(monitor)
}

/// The display a rect in points belongs to: the one holding its centre, or
/// the nearest.
fn display_for(x: f64, y: f64, width: f64, height: f64) -> Option<Display> {
    let (centre_x, centre_y) = (x + width / 2.0, y + height / 2.0);
    let all = displays();
    all.iter()
        .find(|display| display.contains(centre_x, centre_y))
        .or_else(|| {
            all.iter().min_by(|a, b| {
                distance(a, centre_x, centre_y).total_cmp(&distance(b, centre_x, centre_y))
            })
        })
        .copied()
}

fn distance(display: &Display, x: f64, y: f64) -> f64 {
    let frame = display.frame();
    let dx = (frame.x - x).max(0.0).max(x - (frame.x + frame.width));
    let dy = (frame.y - y).max(0.0).max(y - (frame.y + frame.height));
    dx.hypot(dy)
}

pub fn active_screen_frame() -> Option<ScreenFrame> {
    active_display().map(|display| display.frame())
}

/// Places an overlay's content rect, given in points, and with `present`
/// shows it without taking focus.
pub fn set_overlay_frame(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    present: bool,
) -> bool {
    let (Some(raw), Some(display)) = (handle(window), display_for(x, y, width, height)) else {
        return false;
    };
    let (left, top, cx, cy) = display.pixels_for(x, y, width, height);
    on_main(window, move || unsafe {
        let show = if present { SWP_SHOWWINDOW } else { 0 };
        SetWindowPos(
            hwnd(raw),
            HWND_TOPMOST,
            left,
            top,
            cx,
            cy,
            SWP_NOACTIVATE | show,
        );
    });
    true
}

/// Hangs an overlay from the top centre of the active display, keeping its
/// current size. Used by the Samlu Island.
pub fn center_overlay_at_top(window: &WebviewWindow) {
    let Some(raw) = handle(window) else {
        return;
    };
    on_main(window, move || unsafe {
        let Some(display) = active_display() else {
            return;
        };
        let mut rect: RECT = zeroed();
        GetWindowRect(hwnd(raw), &mut rect);
        let width = rect.right - rect.left;
        let left = display.left + (display.right - display.left - width) / 2;
        SetWindowPos(
            hwnd(raw),
            HWND_TOPMOST,
            left,
            display.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    });
}

/// Shows a keyboard-driven overlay on the active display, `top_fraction` of
/// the way down, and gives it focus.
pub fn present_focusable_overlay_at_cursor(
    window: &WebviewWindow,
    width: f64,
    height: f64,
    top_fraction: f64,
) -> bool {
    let Some(raw) = handle(window) else {
        return false;
    };
    on_main(window, move || unsafe {
        if let Some(display) = active_display() {
            let scale = display.scale;
            let (cx, cy) = (
                (width * scale).round() as i32,
                (height * scale).round() as i32,
            );
            let left = display.left + (display.right - display.left - cx) / 2;
            let top = display.top + (f64::from(display.bottom - display.top) * top_fraction) as i32;
            SetWindowPos(hwnd(raw), HWND_TOPMOST, left, top, cx, cy, SWP_SHOWWINDOW);
        } else {
            ShowWindow(hwnd(raw), SW_SHOW);
        }
        bring_to_front(raw);
    });
    true
}

pub fn focus_overlay(window: &WebviewWindow) {
    if let Some(raw) = handle(window) {
        on_main(window, move || unsafe {
            ShowWindow(hwnd(raw), SW_SHOW);
            bring_to_front(raw);
        });
    }
}

/// Gives a visible overlay keyboard focus again. Activating the app has no
/// Space to switch to here, so this is Tauri's own focus.
pub fn make_overlay_key(window: &WebviewWindow) {
    let _ = window.set_focus();
}

/// Windows displays have no camera housing to hang the Island from.
pub fn active_display_has_notch() -> bool {
    false
}

/// Resizes an overlay to `height` logical points while its top edge stays put.
/// On the UI thread this has happened by the time it returns.
pub fn resize_overlay_keeping_top(window: &WebviewWindow, height: f64) {
    let Some(raw) = handle(window) else {
        return;
    };
    on_main(window, move || unsafe {
        let mut frame: RECT = zeroed();
        let mut client: RECT = zeroed();
        if GetWindowRect(hwnd(raw), &mut frame) == 0 || GetClientRect(hwnd(raw), &mut client) == 0 {
            return;
        }
        let dpi = GetDpiForWindow(hwnd(raw));
        let scale = if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 };
        let chrome = (frame.bottom - frame.top) - (client.bottom - client.top);
        let target = (height * scale).round() as i32 + chrome;
        if target == frame.bottom - frame.top {
            return;
        }
        SetWindowPos(
            hwnd(raw),
            null_mut(),
            0,
            0,
            frame.right - frame.left,
            target,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    });
}

/// While `true` the window is transparent to clicks, so a floating surface
/// that only reports status never swallows a click meant for the app below.
pub fn set_ignores_mouse_events(window: &WebviewWindow, ignores: bool) {
    let _ = window.set_ignore_cursor_events(ignores);
}

pub fn order_front_without_activating(window: &WebviewWindow) {
    if let Some(raw) = handle(window) {
        on_main(window, move || unsafe {
            ShowWindow(hwnd(raw), SW_SHOWNOACTIVATE);
            SetWindowPos(
                hwnd(raw),
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        });
    }
}

/// Windows has no activation policy to switch: with Settings hidden, only
/// the tray icon is left, which is the background mode already.
pub fn set_background_mode(_background_mode: bool) {}

/// Brings the Settings window forward and focuses it.
pub fn present_application_window(window: &WebviewWindow) -> bool {
    let _ = window.unminimize();
    let _ = window.show();
    if let Some(raw) = handle(window) {
        on_main(window, move || {
            bring_to_front(raw);
        });
    }
    let _ = window.set_focus();
    true
}

/// Takes the foreground for `raw`. Windows only lets the process the user
/// last interacted with do that, so Samlu briefly shares input state with
/// the current foreground window, the standard way around the lock.
fn bring_to_front(raw: isize) -> bool {
    let window = hwnd(raw);
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        let foreground = GetForegroundWindow();
        let ours = GetCurrentThreadId();
        let theirs = if foreground.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, null_mut())
        };
        let attached = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, 1) != 0;
        BringWindowToTop(window);
        let raised = SetForegroundWindow(window) != 0;
        if attached {
            AttachThreadInput(ours, theirs, 0);
        }
        raised
    }
}

/// The front-most visible, unowned top-level window whose process matches.
fn main_window_of(matches: &dyn Fn(u32) -> bool) -> Option<isize> {
    struct Search<'a> {
        matches: &'a dyn Fn(u32) -> bool,
        found: Option<isize>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        let search = &mut *(data as *mut Search);
        if IsWindowVisible(window) == 0 || !GetWindow(window, GW_OWNER).is_null() {
            return 1;
        }
        if GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_TOOLWINDOW as isize != 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, &mut pid);
        if (search.matches)(pid) {
            search.found = Some(window as isize);
            return 0;
        }
        1
    }
    let mut search = Search {
        matches,
        found: None,
    };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    search.found
}

/// The full path of a process's executable.
fn process_image(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        );
        CloseHandle(process);
        (ok != 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

/// An app's icon as the shell shows it (the large system size), as RGBA
/// rows.
pub fn file_icon_rgba(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    unsafe {
        // The shell needs COM for some icon handlers.
        let initialized = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let pixels = icon_pixels(path);
        if initialized {
            CoUninitialize();
        }
        pixels
    }
}

unsafe fn icon_pixels(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::DeleteObject;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    let path = wide(path);
    let mut info: SHFILEINFOW = zeroed();
    let found = SHGetFileInfoW(
        path.as_ptr(),
        0,
        &mut info,
        size_of::<SHFILEINFOW>() as u32,
        SHGFI_ICON | SHGFI_LARGEICON,
    );
    if found == 0 || info.hIcon.is_null() {
        return None;
    }
    let mut parts: ICONINFO = zeroed();
    let pixels = if GetIconInfo(info.hIcon, &mut parts) != 0 {
        let pixels = bitmap_rgba(parts.hbmColor, parts.hbmMask);
        for bitmap in [parts.hbmColor, parts.hbmMask] {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
        }
        pixels
    } else {
        None
    };
    DestroyIcon(info.hIcon);
    pixels
}

unsafe fn bitmap_rgba(
    color: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    mask: windows_sys::Win32::Graphics::Gdi::HBITMAP,
) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{GetObjectW, BITMAP};
    if color.is_null() {
        return None;
    }
    let mut bitmap: BITMAP = zeroed();
    if GetObjectW(
        color,
        size_of::<BITMAP>() as i32,
        (&mut bitmap as *mut BITMAP).cast(),
    ) == 0
    {
        return None;
    }
    let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
    let mut pixels = bitmap_bits(color, width, height)?;
    for pixel in pixels.chunks_mut(4) {
        pixel.swap(0, 2);
    }
    // Icons from before alpha channels say what is transparent in the mask.
    if pixels.chunks(4).all(|pixel| pixel[3] == 0) {
        let mask = bitmap_bits(mask, width, height);
        for (index, pixel) in pixels.chunks_mut(4).enumerate() {
            let transparent = mask.as_ref().is_some_and(|mask| mask[index * 4] != 0);
            pixel[3] = if transparent { 0 } else { 255 };
        }
    }
    Some((width as u32, height as u32, pixels))
}

/// A bitmap's pixels as top-down 32-bit BGRA rows.
unsafe fn bitmap_bits(
    bitmap: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    width: i32,
    height: i32,
) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    if bitmap.is_null() || width <= 0 || height <= 0 {
        return None;
    }
    let context = CreateCompatibleDC(null_mut());
    let mut format: BITMAPINFO = zeroed();
    format.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    format.bmiHeader.biWidth = width;
    format.bmiHeader.biHeight = -height;
    format.bmiHeader.biPlanes = 1;
    format.bmiHeader.biBitCount = 32;
    format.bmiHeader.biCompression = BI_RGB;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = GetDIBits(
        context,
        bitmap,
        0,
        height as u32,
        pixels.as_mut_ptr().cast(),
        &mut format,
        DIB_RGB_COLORS,
    );
    DeleteDC(context);
    (lines == height).then_some(pixels)
}

pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A string value from the registry, with `%VARIABLES%` left as written.
pub fn registry_string(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let (subkey, value) = (wide(subkey), wide(value));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            null_mut(),
            null_mut(),
            &mut size,
        )
    };
    if status != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let length = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// `PATH` as a new process would get it, read from the user and system
/// environment in the registry, so tools installed after Samlu started are
/// found too.
pub fn registry_path() -> Option<String> {
    const SYSTEM: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    let parts: Vec<String> = [
        registry_string(HKEY_LOCAL_MACHINE, SYSTEM, "Path"),
        registry_string(HKEY_CURRENT_USER, "Environment", "Path"),
    ]
    .into_iter()
    .flatten()
    .map(|part| expand_variables(&part))
    .filter(|part| !part.trim().is_empty())
    .collect();
    (!parts.is_empty()).then(|| parts.join(";"))
}

/// Expands `%NAME%` from Samlu's environment; unknown names stay as written.
pub fn expand_variables(value: &str) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(found) if !name.is_empty() => expanded.push_str(&found),
                    _ => {
                        expanded.push('%');
                        expanded.push_str(name);
                        expanded.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                expanded.push('%');
                rest = after;
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(left: i32, top: i32, right: i32, bottom: i32, scale: f64) -> Display {
        Display {
            left,
            top,
            right,
            bottom,
            scale,
        }
    }

    #[test]
    fn frames_grow_upward_like_appkit() {
        let primary = display(0, 0, 1920, 1040, 1.0);
        let frame = primary.frame();
        assert_eq!((frame.x, frame.width, frame.height), (0.0, 1920.0, 1040.0));
        // The top edge sits at y + height, the bottom at y.
        assert_eq!(frame.y + frame.height, 0.0);
        assert_eq!(frame.y, -1040.0);
    }

    #[test]
    fn a_rect_at_the_bottom_centre_lands_in_pixels() {
        let primary = display(0, 0, 1920, 1040, 1.0);
        let frame = primary.frame();
        let (width, height) = (300.0, 60.0);
        let x = frame.x + (frame.width - width) / 2.0;
        let y = frame.y + 20.0;
        assert_eq!(primary.pixels_for(x, y, width, height), (810, 960, 300, 60));
    }

    #[test]
    fn scaled_displays_convert_through_their_own_scale() {
        // A 150% display to the right of the primary one.
        let right = display(1920, 0, 4800, 1600, 1.5);
        let frame = right.frame();
        assert_eq!((frame.x, frame.width), (1280.0, 1920.0));
        let (left, top, width, height) =
            right.pixels_for(frame.x + 100.0, frame.y + frame.height - 50.0, 200.0, 50.0);
        assert_eq!((left, top, width, height), (2070, 0, 300, 75));
        assert!(right.contains(frame.x + 1.0, frame.y + 1.0));
        assert!(!display(0, 0, 1920, 1040, 1.0).contains(frame.x + 1.0, frame.y + 1.0));
    }

    #[test]
    fn variables_expand_and_unknown_ones_stay() {
        std::env::set_var("SAMLU_TEST_ROOT", r"C:\Tools");
        assert_eq!(
            expand_variables(r"%SAMLU_TEST_ROOT%\bin;%SAMLU_NOT_SET%\x;100%"),
            r"C:\Tools\bin;%SAMLU_NOT_SET%\x;100%"
        );
    }

    #[test]
    fn wide_strings_end_in_nul() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
    }
}

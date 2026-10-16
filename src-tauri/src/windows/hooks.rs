//! Low-level keyboard and mouse hooks: the monitor behind modifier-only
//! shortcuts, and the screen color picker. Each hook runs on its own thread
//! with a message loop, as Windows requires, and returns quickly so Windows
//! never removes it for being slow.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC, CLR_INVALID};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostQuitMessage, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_XBUTTONDOWN,
};

/* ------------------------------------------------------ modifier monitor */

/// Bits `modifier_shortcuts` compares against, the same as on macOS:
/// 1 = Control, 2 = Alt (Option), 4 = Shift, 8 = Windows (Command).
const CONTROL: u32 = 1;
const ALT: u32 = 2;
const SHIFT: u32 = 4;
const WINDOWS: u32 = 8;

/// Each modifier key, left and right, with the bit it reports as.
const MODIFIER_KEYS: [(u16, u32); 8] = [
    (VK_LCONTROL, CONTROL),
    (VK_RCONTROL, CONTROL),
    (VK_LMENU, ALT),
    (VK_RMENU, ALT),
    (VK_LSHIFT, SHIFT),
    (VK_RSHIFT, SHIFT),
    (VK_LWIN, WINDOWS),
    (VK_RWIN, WINDOWS),
];

/// 0 = the held modifiers changed, 1 = a key or click landed.
type ModifierHandler = extern "C" fn(i32, u32);

static MODIFIER_EVENTS: Mutex<Option<mpsc::Sender<(i32, u32)>>> = Mutex::new(None);
/// The modifier bits last reported, so only real changes are sent.
static HELD: AtomicU32 = AtomicU32::new(0);

/// Starts listening, once. The hooks only listen; they never change or
/// swallow an event. Returns whether they are installed.
pub fn start_modifier_monitor(handler: ModifierHandler) -> bool {
    let mut events = MODIFIER_EVENTS.lock().unwrap();
    if events.is_some() {
        return true;
    }
    // Handled off the hook thread, in order, so the hooks return at once.
    let (sender, received) = mpsc::channel::<(i32, u32)>();
    let (ready, installed) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("samlu-modifier-hooks".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(null());
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(modifier_keyboard), module, 0);
            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(modifier_mouse), module, 0);
            let ok = !keyboard.is_null() && !mouse.is_null();
            let _ = ready.send(ok);
            if ok {
                pump_messages();
            }
            unhook(keyboard);
            unhook(mouse);
        });
    if spawned.is_err() || !installed.recv().unwrap_or(false) {
        return false;
    }
    std::thread::spawn(move || {
        for (kind, modifiers) in received {
            handler(kind, modifiers);
        }
    });
    *events = Some(sender);
    true
}

fn report(kind: i32, modifiers: u32) {
    if let Some(events) = MODIFIER_EVENTS.lock().unwrap().as_ref() {
        let _ = events.send((kind, modifiers));
    }
}

fn modifier_bit(code: u32) -> Option<u32> {
    let code = code as u16;
    MODIFIER_KEYS
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, bit)| *bit)
        .or(match code {
            VK_CONTROL => Some(CONTROL),
            VK_MENU => Some(ALT),
            VK_SHIFT => Some(SHIFT),
            _ => None,
        })
}

/// The modifiers down now. A low-level hook sees an event before the key
/// state is updated, so the key in hand comes from the event itself and the
/// rest from the keyboard; a key-up lost to a secure desktop can never leave
/// a modifier stuck.
fn held_modifiers(code: u16, down: bool) -> u32 {
    let mut bits = 0;
    for (key, bit) in MODIFIER_KEYS {
        let pressed = if key == code {
            down
        } else {
            (unsafe { GetAsyncKeyState(i32::from(key)) } as u16) & 0x8000 != 0
        };
        if pressed {
            bits |= bit;
        }
    }
    bits
}

unsafe extern "system" fn modifier_keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = &*(lparam as *const KBDLLHOOKSTRUCT);
        // Samlu's own paste keystroke is not the user's chord.
        if event.flags & LLKHF_INJECTED == 0 {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            match modifier_bit(event.vkCode) {
                Some(_) => {
                    let held = held_modifiers(event.vkCode as u16, down);
                    if HELD.swap(held, Ordering::AcqRel) != held {
                        report(0, held);
                    }
                }
                None if down => report(1, HELD.load(Ordering::Acquire)),
                None => {}
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn modifier_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = &*(lparam as *const MSLLHOOKSTRUCT);
        let button_down = matches!(
            wparam as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        );
        if button_down && event.flags & LLMHF_INJECTED == 0 {
            report(1, HELD.load(Ordering::Acquire));
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/* ----------------------------------------------------------- color picker */

/// The thread running a pick, or 0. One pick at a time.
static PICK_THREAD: AtomicU32 = AtomicU32::new(0);
static PICKED: Mutex<Option<String>> = Mutex::new(None);

/// Waits for the next click anywhere on screen and hands `done` the color of
/// that pixel as `#RRGGBB`. Escape or a right click cancels with `None`. The
/// click that picks is swallowed, so it never reaches the window beneath.
pub fn pick_color(done: impl FnOnce(Option<String>) + Send + 'static) {
    std::thread::spawn(move || done(run_pick()));
}

/// Ends a pick that is still waiting, as a cancellation.
pub fn cancel_color_pick() {
    let thread = PICK_THREAD.load(Ordering::Acquire);
    if thread != 0 {
        unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
    }
}

fn run_pick() -> Option<String> {
    *PICKED.lock().unwrap() = None;
    let thread = unsafe { GetCurrentThreadId() };
    if PICK_THREAD
        .compare_exchange(0, thread, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    unsafe {
        let module = GetModuleHandleW(null());
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(pick_mouse), module, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(pick_keyboard), module, 0);
        if !mouse.is_null() && !keyboard.is_null() {
            pump_messages();
        }
        unhook(mouse);
        unhook(keyboard);
    }
    PICK_THREAD.store(0, Ordering::Release);
    PICKED.lock().unwrap().take()
}

unsafe extern "system" fn pick_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = &*(lparam as *const MSLLHOOKSTRUCT);
        match wparam as u32 {
            WM_LBUTTONDOWN => {
                *PICKED.lock().unwrap() = pixel_at(event.pt);
                PostQuitMessage(0);
                return 1;
            }
            WM_RBUTTONDOWN => {
                PostQuitMessage(0);
                return 1;
            }
            _ => {}
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn pick_keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = &*(lparam as *const KBDLLHOOKSTRUCT);
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if down && event.vkCode == u32::from(VK_ESCAPE) {
            PostQuitMessage(0);
            return 1;
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// The screen pixel at `point` (physical pixels) as `#RRGGBB`.
fn pixel_at(point: POINT) -> Option<String> {
    unsafe {
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return None;
        }
        let color = GetPixel(screen, point.x, point.y);
        ReleaseDC(null_mut(), screen);
        (color != CLR_INVALID).then(|| hex(color))
    }
}

/// A `COLORREF` (0x00BBGGRR) as `#RRGGBB`.
fn hex(color: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        color & 0xFF,
        (color >> 8) & 0xFF,
        (color >> 16) & 0xFF
    )
}

/* ---------------------------------------------------------------- shared */

unsafe fn pump_messages() {
    let mut message: MSG = std::mem::zeroed();
    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
}

unsafe fn unhook(hook: HHOOK) {
    if !hook.is_null() {
        UnhookWindowsHookEx(hook);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorrefs_read_as_rgb_hex() {
        assert_eq!(hex(0x0000_00FF), "#FF0000");
        assert_eq!(hex(0x0033_2211), "#112233");
        assert_eq!(hex(0x00EF_D01D), "#1DD0EF");
    }

    #[test]
    fn both_sides_of_a_modifier_share_its_bit() {
        assert_eq!(modifier_bit(u32::from(VK_LCONTROL)), Some(CONTROL));
        assert_eq!(modifier_bit(u32::from(VK_RMENU)), Some(ALT));
        assert_eq!(modifier_bit(u32::from(VK_SHIFT)), Some(SHIFT));
        assert_eq!(modifier_bit(u32::from(VK_RWIN)), Some(WINDOWS));
        assert_eq!(modifier_bit(0x56), None);
    }
}

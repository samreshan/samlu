use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

type Sender = mpsc::Sender<Option<String>>;

fn pending_sender() -> &'static Mutex<Option<Sender>> {
    static SENDER: OnceLock<Mutex<Option<Sender>>> = OnceLock::new();
    SENDER.get_or_init(|| Mutex::new(None))
}

fn deliver(result: Option<String>) {
    if let Some(sender) = pending_sender().lock().unwrap().take() {
        let _ = sender.send(result);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    extern "C" fn color_selected(value: *const c_char) {
        let result = if value.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(value) }
                    .to_string_lossy()
                    .to_string(),
            )
        };
        super::deliver(result);
    }

    extern "C" {
        fn samlu_pick_color(callback: extern "C" fn(*const c_char));
    }

    /// AppKit's own color sampler.
    pub fn start() {
        unsafe { samlu_pick_color(color_selected) };
    }

    pub fn cancel() {}
}

#[cfg(windows)]
mod native {
    /// The next click anywhere picks; Escape or a right click cancels.
    pub fn start() {
        crate::platform::pick_color(super::deliver);
    }

    pub fn cancel() {
        crate::platform::cancel_color_pick();
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod native {
    pub fn start() {
        super::deliver(None);
    }

    pub fn cancel() {}
}

pub fn pick() -> Result<String, String> {
    let (sender, receiver) = mpsc::channel();
    {
        let mut pending = pending_sender().lock().unwrap();
        if pending.is_some() {
            return Err("A color picker is already open.".to_string());
        }
        *pending = Some(sender);
    }
    native::start();
    match receiver.recv_timeout(Duration::from_secs(120)) {
        Ok(Some(color)) => Ok(color),
        Ok(None) => Err("Color picking was cancelled.".to_string()),
        Err(_) => {
            pending_sender().lock().unwrap().take();
            native::cancel();
            Err("Color picker timed out.".to_string())
        }
    }
}

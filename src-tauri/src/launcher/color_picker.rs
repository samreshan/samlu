use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

type Sender = mpsc::Sender<Option<String>>;

fn pending_sender() -> &'static Mutex<Option<Sender>> {
    static SENDER: OnceLock<Mutex<Option<Sender>>> = OnceLock::new();
    SENDER.get_or_init(|| Mutex::new(None))
}

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
    if let Some(sender) = pending_sender().lock().unwrap().take() {
        let _ = sender.send(result);
    }
}

extern "C" {
    fn samlu_pick_color(callback: extern "C" fn(*const c_char));
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
    unsafe { samlu_pick_color(color_selected) };
    match receiver.recv_timeout(Duration::from_secs(120)) {
        Ok(Some(color)) => Ok(color),
        Ok(None) => Err("Color picking was cancelled.".to_string()),
        Err(_) => {
            pending_sender().lock().unwrap().take();
            Err("Color picker timed out.".to_string())
        }
    }
}

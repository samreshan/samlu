//! On-device Underdog Bark 3.0 (ConwayResearch/Underdog-Bark-3.0), an
//! English-only Parakeet TDT model. Core ML runs the encoder through the
//! Swift bridge in `native/SamluBark.swift`; the features and the decoder run
//! here. Core ML's first load of the encoder on a Mac takes minutes, so the
//! model is prepared in the background after download and at launch, and is
//! then kept resident.

mod decoder;
mod frontend;

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::ProviderError;
use crate::voice::models::MODEL_NOT_FOUND;
use decoder::Decoder;
use frontend::{Frontend, HOP, MELS, SAMPLE_RATE};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The encoder's fixed window. The package also has an 8 second function,
/// but it is no faster and would double the first load.
const WINDOW_SECONDS: i32 = 15;
/// Silence added in front of each piece, the setting the model was scored with.
const LEADING_SILENCE: usize = SAMPLE_RATE / 4;
const MIN_SEGMENT_SECONDS: f32 = 5.0;
/// Core ML compute units: 0 Neural Engine, 1 GPU, 2 CPU.
const COMPUTE_UNITS: i32 = 0;

#[cfg(target_os = "macos")]
extern "C" {
    fn samlu_bark_available() -> bool;
    fn samlu_bark_load(
        dir: *const c_char,
        units: i32,
        windows: *const i32,
        window_count: i32,
        out: *mut *mut c_char,
    ) -> *mut c_void;
    fn samlu_bark_release(handle: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn samlu_bark_encode(
        handle: *mut c_void,
        window: i32,
        input: *const f32,
        frames: i32,
        valid: i32,
        output: *mut f32,
        capacity: usize,
        out_frames: *mut i32,
        out_width: *mut i32,
        out: *mut *mut c_char,
    ) -> i32;
    fn samlu_bark_free(pointer: *mut c_char);
}

/// Core ML is macOS only. Elsewhere these stand in for the Swift bridge and
/// report it unavailable, so everything below is shared.
#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
mod unavailable {
    use std::ffi::{c_char, c_void, CString};

    const MESSAGE: &str = "Underdog Bark is only available on macOS.";

    pub unsafe fn samlu_bark_available() -> bool {
        false
    }

    pub unsafe fn samlu_bark_load(
        _dir: *const c_char,
        _units: i32,
        _windows: *const i32,
        _window_count: i32,
        out: *mut *mut c_char,
    ) -> *mut c_void {
        *out = CString::new(MESSAGE).unwrap_or_default().into_raw();
        std::ptr::null_mut()
    }

    pub unsafe fn samlu_bark_release(_handle: *mut c_void) {}

    pub unsafe fn samlu_bark_encode(
        _handle: *mut c_void,
        _window: i32,
        _input: *const f32,
        _frames: i32,
        _valid: i32,
        _output: *mut f32,
        _capacity: usize,
        _out_frames: *mut i32,
        _out_width: *mut i32,
        out: *mut *mut c_char,
    ) -> i32 {
        *out = CString::new(MESSAGE).unwrap_or_default().into_raw();
        1
    }

    pub unsafe fn samlu_bark_free(pointer: *mut c_char) {
        if !pointer.is_null() {
            drop(CString::from_raw(pointer));
        }
    }
}

#[cfg(not(target_os = "macos"))]
use unavailable::*;

/// Whether this Mac can run Bark: Core ML multifunction models need macOS 15.
pub fn available() -> bool {
    // SAFETY: takes no arguments.
    unsafe { samlu_bark_available() }
}

/// Takes ownership of a string the bridge allocated.
unsafe fn take(out: *mut c_char) -> String {
    if out.is_null() {
        return String::new();
    }
    let text = CStr::from_ptr(out).to_string_lossy().into_owned();
    samlu_bark_free(out);
    text
}

/// A loaded Core ML encoder. Calls are serialized by `Model`'s mutex.
struct Encoder(*mut c_void);

// SAFETY: the handle is a retained Swift object that is only used behind a
// mutex, so one thread at a time calls into it.
unsafe impl Send for Encoder {}

impl Encoder {
    fn load(dir: &Path) -> Result<Self, String> {
        let dir = CString::new(dir.to_string_lossy().as_bytes())
            .map_err(|_| MODEL_NOT_FOUND.to_string())?;
        let windows = [WINDOW_SECONDS];
        let mut out = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated path and window list; `out` receives a
        // malloc'd message on failure.
        let handle = unsafe {
            samlu_bark_load(
                dir.as_ptr(),
                COMPUTE_UNITS,
                windows.as_ptr(),
                windows.len() as i32,
                &mut out,
            )
        };
        let message = unsafe { take(out) };
        if handle.is_null() {
            Err(message)
        } else {
            Ok(Self(handle))
        }
    }

    /// `features` is `frames × MELS`; returns `rows × width` floats.
    fn encode(&self, features: &[f32], valid: usize, width: usize) -> Result<Vec<f32>, String> {
        let frames = features.len() / MELS;
        let mut output = vec![0.0_f32; frames * width];
        let (mut rows, mut columns) = (0_i32, 0_i32);
        let mut out = std::ptr::null_mut();
        // SAFETY: the handle is live; buffers match the lengths passed.
        let status = unsafe {
            samlu_bark_encode(
                self.0,
                WINDOW_SECONDS,
                features.as_ptr(),
                frames as i32,
                valid as i32,
                output.as_mut_ptr(),
                output.len(),
                &mut rows,
                &mut columns,
                &mut out,
            )
        };
        let message = unsafe { take(out) };
        if status != 0 {
            return Err(message);
        }
        if columns as usize != width {
            return Err("Underdog Bark's encoder and decoder do not match.".to_string());
        }
        output.truncate(rows as usize * width);
        Ok(output)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: releases the reference `samlu_bark_load` retained.
        unsafe { samlu_bark_release(self.0) }
    }
}

struct Model {
    encoder: Mutex<Encoder>,
    decoder: Decoder,
    frontend: Frontend,
}

impl Model {
    fn load(dir: &Path) -> Result<Self, String> {
        let decoder = Decoder::load(dir)?;
        let frontend = Frontend::new(decoder.mel_filters.clone())?;
        let encoder = Encoder::load(dir)?;
        Ok(Self {
            encoder: Mutex::new(encoder),
            decoder,
            frontend,
        })
    }

    fn piece(&self, samples: &[f32]) -> Result<Vec<usize>, String> {
        let mut padded = vec![0.0_f32; LEADING_SILENCE];
        padded.extend_from_slice(samples);
        let features = self.frontend.features(&padded);
        let frames = WINDOW_SECONDS as usize * (SAMPLE_RATE / HOP) + 1;
        let mut window = vec![0.0_f32; frames * MELS];
        let used = features.frames.min(frames);
        window[..used * MELS].copy_from_slice(&features.data[..used * MELS]);
        let valid = features.valid.min(frames);
        let width = self.decoder.encoder_width();
        let encoded = self.encoder.lock().unwrap().encode(&window, valid, width)?;
        let rows = decoder::encoded_frames(valid).min(encoded.len() / width);
        let encoded = &encoded[..rows * width];
        if !encoded.iter().all(|value| value.is_finite()) {
            return Err("Underdog Bark produced invalid output.".to_string());
        }
        Ok(self.decoder.decode(encoded, rows))
    }

    fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        let max_seconds = WINDOW_SECONDS as f32 - LEADING_SILENCE as f32 / SAMPLE_RATE as f32;
        let mut parts = Vec::new();
        for piece in decoder::segments(samples, max_seconds, MIN_SEGMENT_SECONDS) {
            let text = self.decoder.detokenize(&self.piece(piece)?);
            if !text.is_empty() {
                parts.push(text);
            }
        }
        Ok(parts.join(" "))
    }
}

/// Where the resident model is. Core ML's first load of the encoder on a Mac
/// takes minutes, so loads run on their own thread and nothing waits on one
/// for long; a loaded model stays resident until another is chosen.
enum Slot {
    Empty,
    Loading(PathBuf),
    Ready(PathBuf, Arc<Model>),
    Failed(PathBuf, String),
}

static SLOT: Mutex<Slot> = Mutex::new(Slot::Empty);
static CHANGED: Condvar = Condvar::new();
/// How long a dictation waits for a load in progress before it is held for
/// a retry instead.
const DICTATION_WAIT: Duration = Duration::from_secs(30);
pub const PREPARING: &str =
    "Underdog Bark is getting ready. The first time takes a few minutes; try again shortly.";

/// Starts loading `path` unless it is loaded or loading already. A failed
/// load is tried again.
fn ensure_loading(path: &Path) {
    let mut slot = SLOT.lock().unwrap();
    match &*slot {
        Slot::Ready(loaded, _) | Slot::Loading(loaded) if loaded == path => return,
        _ => {}
    }
    *slot = Slot::Loading(path.to_path_buf());
    drop(slot);
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = load(&path);
        match &result {
            Ok(_) => log::info!(
                "[voice] loaded Underdog Bark in {:.1}s",
                started.elapsed().as_secs_f32()
            ),
            Err(error) => log::warn!("[voice] Underdog Bark failed to load: {error}"),
        }
        let mut slot = SLOT.lock().unwrap();
        // Only the load still wanted is kept; an unload or another choice
        // during it wins.
        if matches!(&*slot, Slot::Loading(wanted) if *wanted == path) {
            *slot = match result {
                Ok(model) => Slot::Ready(path, Arc::new(model)),
                Err(error) => Slot::Failed(path, error),
            };
            CHANGED.notify_all();
        }
    });
}

fn load(path: &Path) -> Result<Model, String> {
    if !available() {
        return Err("Underdog Bark needs macOS 15 or later.".to_string());
    }
    if !crate::voice::models::is_bark_dir(path) {
        return Err(MODEL_NOT_FOUND.to_string());
    }
    Model::load(path)
}

/// The loaded model at `path`, waiting up to `wait` for a load to finish.
fn model(path: &Path, wait: Option<Duration>) -> Result<Arc<Model>, String> {
    ensure_loading(path);
    let deadline = wait.map(|wait| Instant::now() + wait);
    let mut slot = SLOT.lock().unwrap();
    loop {
        match &*slot {
            Slot::Ready(loaded, model) if loaded == path => return Ok(model.clone()),
            Slot::Failed(loaded, error) if loaded == path => return Err(error.clone()),
            Slot::Loading(loaded) if loaded == path => {}
            // Unloaded or switched while waiting.
            _ => return Err(MODEL_NOT_FOUND.to_string()),
        }
        slot = match deadline {
            None => CHANGED.wait(slot).unwrap(),
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(PREPARING.to_string());
                }
                CHANGED.wait_timeout(slot, left).unwrap().0
            }
        };
    }
}

/// Blocking. Loads the model, however long the first load takes.
pub fn prepare(path: &Path) -> Result<(), String> {
    model(path, None).map(|_| ())
}

/// Starts loading in the background so the first dictation does not wait.
pub fn preload(path: PathBuf) {
    ensure_loading(&path);
}

/// The model being loaded right now, for Settings to show.
pub fn loading() -> Option<PathBuf> {
    match &*SLOT.lock().unwrap() {
        Slot::Loading(path) => Some(path.clone()),
        _ => None,
    }
}

pub fn unload() {
    *SLOT.lock().unwrap() = Slot::Empty;
    CHANGED.notify_all();
}

/// Drops the resident or loading model when `drop` says so.
fn release_if(drop: impl FnOnce(&Path) -> bool) {
    let mut slot = SLOT.lock().unwrap();
    let path = match &*slot {
        Slot::Empty => return,
        Slot::Loading(path) | Slot::Ready(path, _) | Slot::Failed(path, _) => path,
    };
    if drop(path) {
        *slot = Slot::Empty;
        CHANGED.notify_all();
    }
}

/// Keeps the model only if it is `keep`, so choosing Bark while it is still
/// loading does not throw the load away.
pub fn keep_only(keep: &Path) {
    release_if(|path| path != keep);
}

/// Drops `path` if it is resident or loading.
pub fn release(path: &Path) {
    release_if(|loaded| loaded == path);
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
    let path = PathBuf::from(opts.settings.model.trim());
    let samples = audio
        .local_samples()
        .map_err(ProviderError::permanent)?
        .to_vec();
    tauri::async_runtime::spawn_blocking(move || {
        model(&path, Some(DICTATION_WAIT))?.transcribe(&samples)
    })
    .await
    .map_err(|error| ProviderError::permanent(format!("Underdog Bark task failed: {error}")))?
    .map_err(ProviderError::permanent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Tests that touch the one model slot run one at a time.
    static SLOT_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn a_folder_that_is_not_bark_fails_without_waiting() {
        let _guard = SLOT_TEST.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("samlu-bark-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let started = Instant::now();
        let error = model(&dir, Some(Duration::from_secs(20)))
            .err()
            .expect("not a model");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_ne!(error, PREPARING);
        unload();
        assert!(loading().is_none());
    }

    #[test]
    fn only_the_model_in_use_is_kept() {
        let _guard = SLOT_TEST.lock().unwrap();
        let bark = PathBuf::from("/models/Underdog-Bark-3.0");
        *SLOT.lock().unwrap() = Slot::Loading(bark.clone());
        keep_only(&bark);
        assert_eq!(loading().as_deref(), Some(bark.as_path()));
        release(Path::new("/models/other"));
        assert_eq!(loading().as_deref(), Some(bark.as_path()));
        keep_only(Path::new("/models/ggml-base.bin"));
        assert!(loading().is_none());
        *SLOT.lock().unwrap() = Slot::Loading(bark.clone());
        release(&bark);
        assert!(loading().is_none());
    }

    #[test]
    #[ignore = "needs Underdog Bark downloaded to SAMLU_BARK_DIR and macOS `say`"]
    fn transcribes_a_spoken_sentence() {
        let dir = PathBuf::from(std::env::var("SAMLU_BARK_DIR").expect("set SAMLU_BARK_DIR"));
        let wav = std::env::temp_dir().join("samlu-bark-fox.wav");
        let status = Command::new("say")
            .args(["--file-format=WAVE", "--data-format=LEI16@48000", "-o"])
            .arg(&wav)
            .arg("The quick brown fox jumps over the lazy dog.")
            .status()
            .unwrap();
        assert!(status.success());
        let samples = crate::voice::audio::to_local_samples(&std::fs::read(&wav).unwrap()).unwrap();
        let _guard = SLOT_TEST.lock().unwrap();
        let started = Instant::now();
        // Through the background loader, as dictation and Settings use it.
        preload(dir.clone());
        assert_eq!(loading().as_deref(), Some(dir.as_path()));
        let model = model(&dir, None).unwrap();
        assert!(loading().is_none());
        eprintln!("load {:?}", started.elapsed());
        let started = Instant::now();
        let text = model.transcribe(&samples).unwrap();
        eprintln!("transcribe {:?}: {text}", started.elapsed());
        assert!(text.to_lowercase().contains("brown fox"), "{text}");
        // Past one window, so the audio is cut and both halves are read.
        let mut long = samples.clone();
        long.extend(vec![0.0; SAMPLE_RATE]);
        long.extend_from_slice(&samples);
        long.extend(vec![0.0; SAMPLE_RATE]);
        long.extend_from_slice(&samples);
        long.extend(vec![0.0; SAMPLE_RATE * 6]);
        let text = model.transcribe(&long).unwrap().to_lowercase();
        eprintln!("long: {text}");
        assert_eq!(text.matches("brown fox").count(), 3, "{text}");
    }
}

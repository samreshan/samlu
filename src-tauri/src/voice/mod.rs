//! Global voice dictation for macOS. A short tap starts toggle recording; a
//! press held for at least 350 ms records until release.

mod audio;
mod cloud;
pub mod config;
mod download;
mod engines;
pub mod history;
pub mod model_commands;
mod models;
mod paste;
mod recorder;
mod vocabulary;

pub use download::Downloads;

use config::{DeliveryBehavior, VoiceConfig};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{
    utils::config::Color, AppHandle, Emitter, LogicalSize, Manager, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const HOLD_THRESHOLD: Duration = Duration::from_millis(350);
const MAX_RECORDING_DURATION: Duration = Duration::from_secs(120);
const PREVIEW_LABEL: &str = "voice-preview";
/// Window sizes are the visible surface plus the 8pt inset the webview keeps
/// on every side for the float shadow (BODY_INSET in preview.js).
const SURFACE_INSET: f64 = 16.0;
/// Listening is a 44pt pill sized to its content: the breath bead, the voice
/// beads and the timer, plus a mode chip for Summary and Prompt, plus Cancel
/// and Stop once a tap has made it a toggle recording.
const LISTENING_WIDTH: f64 = 160.0;
const LISTENING_CHIP_WIDTH: f64 = 64.0;
const LISTENING_CONTROLS_WIDTH: f64 = 60.0;
const PILL_HEIGHT: f64 = 44.0;
const CAPSULE_WIDTH: f64 = 236.0;
const LANDED_WIDTH: f64 = 120.0;
/// Room for a qualifier after the result, e.g. "Inserted · cleanup skipped".
const LANDED_WIDE_WIDTH: f64 = 212.0;
const LANDED_HEIGHT: f64 = 36.0;
const COPIED_WIDTH: f64 = 248.0;
const COPIED_HEIGHT: f64 = 52.0;
const NOTICE_WIDTH: f64 = 340.0;
const NOTICE_HEIGHT: f64 = 90.0;
const RECOVERY_WIDTH: f64 = 360.0;
const RECOVERY_HEIGHT: f64 = 118.0;
const EDITOR_WIDTH: f64 = 524.0;
const EDITOR_HEIGHT: f64 = 252.0;
const BOTTOM_MARGIN: f64 = 28.0;
/// Long enough for the capsule's exit to finish. Keep in step with
/// --duration-exit in presence.css.
const PREVIEW_EXIT_MS: u64 = 170;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Summarize,
    Prompt,
}

#[derive(Clone, Debug)]
struct TargetContext {
    app: paste::TargetApp,
    /// The display you were working on when the shortcut fired. The capsule
    /// stays on it for the whole session.
    display: Option<crate::macos::ScreenFrame>,
}

enum Runtime {
    Idle,
    Starting {
        mode: Mode,
        pressed_at: Instant,
        released_at: Option<Instant>,
        target: TargetContext,
    },
    Recording {
        id: u64,
        recording: recorder::Recording,
        mode: Mode,
        pressed_at: Instant,
        target: TargetContext,
    },
    Processing,
    Previewing {
        target: TargetContext,
        text: String,
        mode: Mode,
    },
    Recovering {
        target: TargetContext,
        text: String,
        mode: Mode,
    },
    /// Processing failed after audio was captured. The capture is held until
    /// the user retries or discards it, so a provider failure never costs
    /// them what they said.
    Failed {
        target: TargetContext,
        capture: Capture,
        mode: Mode,
    },
}

/// One dictation's audio, kept until it has produced text.
struct Capture {
    audio: engines::PreparedAudio,
    /// Set once transcription succeeds, so a retry after a failed transform
    /// does not pay for (or risk) transcribing again.
    transcript: Option<String>,
}

pub struct VoiceState {
    runtime: Mutex<Runtime>,
    shortcut_down: AtomicBool,
    next_recording_id: AtomicU64,
    presentation_generation: AtomicU64,
    presentation_ready: std::sync::atomic::AtomicBool,
    pending_presentation: Mutex<Option<PreviewPayload>>,
    /// Physical-pixel offset the user has dragged the capsule to, relative to
    /// its default bottom-center anchor. Reset per session so a one-off nudge
    /// never becomes a permanent, forgotten position.
    drag_offset: Mutex<(f64, f64)>,
    /// Where the capsule currently sits: its display and window size, so a
    /// drag can move it without asking AppKit where it is.
    placement: Mutex<Option<(crate::macos::ScreenFrame, f64, f64)>>,
    /// The current recording was started by a tap rather than a hold.
    toggle_recording: AtomicBool,
    last_history_id: Mutex<Option<String>>,
}

impl VoiceState {
    pub fn new() -> Self {
        Self {
            runtime: Mutex::new(Runtime::Idle),
            shortcut_down: AtomicBool::new(false),
            next_recording_id: AtomicU64::new(1),
            presentation_generation: AtomicU64::new(1),
            presentation_ready: std::sync::atomic::AtomicBool::new(false),
            pending_presentation: Mutex::new(None),
            drag_offset: Mutex::new((0.0, 0.0)),
            placement: Mutex::new(None),
            toggle_recording: AtomicBool::new(false),
            last_history_id: Mutex::new(None),
        }
    }

    fn present_or_queue(&self, app: &AppHandle, payload: PreviewPayload) {
        if self
            .presentation_ready
            .load(std::sync::atomic::Ordering::Acquire)
        {
            emit_preview(app, payload);
        } else {
            *self.pending_presentation.lock().unwrap() = Some(payload);
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewPayload {
    state: String,
    mode: String,
    title: String,
    detail: String,
    text: String,
    editable: bool,
    interface_sounds: bool,
    /// A tap started this recording, so it needs visible Cancel and Stop. A
    /// held shortcut ends on release and keeps the capsule bare.
    controls: bool,
    /// Logical size of the window this state occupies. The webview animates
    /// its single glass surface to match, so the presentation reads as one
    /// panel growing rather than three windows swapping.
    width: f64,
    height: f64,
}

pub fn voice_preview_init(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PREVIEW_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        PREVIEW_LABEL,
        WebviewUrl::App("voice-preview/preview.html".into()),
    )
    .title("Samlu Voice")
    .inner_size(CAPSULE_WIDTH + SURFACE_INSET, PILL_HEIGHT + SURFACE_INSET)
    .decorations(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .focused(false)
    .visible(false)
    .build()
    .map(|window| {
        crate::macos::configure_overlay(&window);
    })
    .map_err(|error| error.to_string())
}

pub fn engines_init() {
    engines::whisper::init();
}

/// Releases the resident whisper model before process exit; ggml-metal
/// asserts if a context is still alive during its static teardown.
pub fn engines_shutdown() {
    engines::whisper::unload();
}

pub fn apple_available() -> bool {
    engines::apple::available()
}

pub fn recommended_model() -> &'static str {
    models::recommended_id()
}

fn derive_shortcuts(base: &str) -> (String, String, String) {
    let mut parts: Vec<&str> = base
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let key = parts.pop().unwrap_or("V").to_string();
    let modifiers = parts.join("+");
    let with_extra = |extra: &str| {
        if modifiers.is_empty() {
            format!("{extra}+{key}")
        } else {
            format!("{modifiers}+{extra}+{key}")
        }
    };
    (base.to_string(), with_extra("Shift"), with_extra("Cmd"))
}

fn normalize_endpoint_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(value).map_err(|_| {
        "Enter a valid provider URL using HTTPS (or localhost for development).".to_string()
    })?;
    let is_https = url.scheme() == "https";
    let is_local_http = url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
        );
    if !is_https && !is_local_http {
        return Err(
            "Custom provider URL must use HTTPS (or localhost for development).".to_string(),
        );
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Provider URLs cannot contain embedded credentials.".to_string());
    }
    Ok(value.to_string())
}

fn register_one(app: &AppHandle, shortcut: &str, mode: Mode) -> Result<(), String> {
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| match event.state() {
            ShortcutState::Pressed => voice_pressed(app, mode),
            ShortcutState::Released => voice_released(app),
        })
        .map_err(|error| error.to_string())
}

pub fn register_hotkeys(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    // A rebind can happen with the old chord still physically held down; the
    // release for it will never arrive at the new registration.
    if let Some(state) = app.try_state::<Arc<VoiceState>>() {
        state.shortcut_down.store(false, Ordering::Release);
    }
    let (normal, summarize, prompt) = derive_shortcuts(hotkey);
    register_one(app, &normal, Mode::Normal)?;
    if let Err(error) = register_one(app, &summarize, Mode::Summarize) {
        unregister_hotkeys(app, hotkey);
        return Err(error);
    }
    if let Err(error) = register_one(app, &prompt, Mode::Prompt) {
        unregister_hotkeys(app, hotkey);
        return Err(error);
    }
    Ok(())
}

pub fn unregister_hotkeys(app: &AppHandle, hotkey: &str) {
    let (normal, summarize, prompt) = derive_shortcuts(hotkey);
    let _ = app.global_shortcut().unregister(normal.as_str());
    let _ = app.global_shortcut().unregister(summarize.as_str());
    let _ = app.global_shortcut().unregister(prompt.as_str());
}

const OPENAI_STT_MODEL: &str = "gpt-4o-transcribe";

/// The single picker value the settings UI uses for engine + preset.
fn stt_choice(stt: &config::SttSettings) -> &'static str {
    use config::SttEngine::*;
    match stt.engine {
        OpenaiCompat => match stt.preset.as_deref() {
            Some("groq") => "groq",
            Some("openai") => "openai",
            _ => "custom",
        },
        Deepgram => "deepgram",
        Elevenlabs => "elevenlabs",
        WhisperCpp => "whisper_cpp",
        Apple => "apple",
    }
}

/// Builds settings for a picker choice. An empty `model` falls back to the
/// choice's default; the language carries over from `current`.
fn stt_from_choice(
    choice: &str,
    base_url: &str,
    model: &str,
    current: &config::SttSettings,
) -> Result<config::SttSettings, String> {
    use config::SttEngine::*;
    let model = model.trim();
    let pick = |default: &str| {
        if model.is_empty() {
            default.to_string()
        } else {
            model.to_string()
        }
    };
    let (engine, preset, base_url, model) = match choice {
        "groq" => (
            OpenaiCompat,
            Some("groq"),
            Some(config::DEFAULT_BASE_URL.to_string()),
            pick(config::DEFAULT_STT_MODEL),
        ),
        "openai" => (
            OpenaiCompat,
            Some("openai"),
            Some(config::OPENAI_BASE_URL.to_string()),
            pick(OPENAI_STT_MODEL),
        ),
        "custom" => (
            OpenaiCompat,
            Some("custom"),
            Some(normalize_endpoint_url(base_url)?),
            pick(config::DEFAULT_STT_MODEL),
        ),
        "deepgram" => (Deepgram, None, None, pick(config::DEEPGRAM_DEFAULT_MODEL)),
        "elevenlabs" => (
            Elevenlabs,
            None,
            None,
            pick(config::ELEVENLABS_DEFAULT_MODEL),
        ),
        "whisper_cpp" => (WhisperCpp, None, None, model.to_string()),
        "apple" => (Apple, None, None, String::new()),
        _ => return Err("Choose a supported speech engine.".to_string()),
    };
    Ok(config::SttSettings {
        engine,
        preset: preset.map(str::to_string),
        model,
        base_url,
        language: current.language.clone(),
    })
}

#[tauri::command]
pub fn get_voice_settings(config: tauri::State<'_, Arc<VoiceConfig>>) -> serde_json::Value {
    let stt = config.stt();
    let transformation = config.transformation_config();
    serde_json::json!({
        "stt": {
            "choice": stt_choice(&stt),
            "engine": stt.engine,
            "preset": stt.preset,
            "baseUrl": stt.base_url,
            "model": stt.model,
            "language": stt.language,
            "needsApiKey": stt.engine.needs_api_key(),
            "hasApiKey": stt.engine.needs_api_key() && config.has_api_key("transcription"),
        },
        "appleAvailable": engines::apple::available(),
        "transformation": {
            "provider": transformation.provider,
            "baseUrl": transformation.base_url,
            "model": transformation.model,
            "needsApiKey": config::transform_needs_key(&transformation.provider),
            "hasApiKey": config.has_api_key("transformation"),
        },
        "cleanupDictation": config.cleanup_dictation(),
        "vocabulary": config.vocabulary(),
        "keepHistory": config.keep_history(),
        "hotkey": config.hotkey(),
        "delivery": {
            "dictation": config.delivery(Mode::Normal),
            "summary": config.delivery(Mode::Summarize),
            "prompt": config.delivery(Mode::Prompt),
        },
        "interfaceSounds": config.interface_sounds(),
    })
}

#[tauri::command]
pub fn set_voice_api_key(
    role: String,
    key: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    validate_role(&role)?;
    config.set_api_key(&role, &key)
}

#[tauri::command]
pub fn set_voice_stt(
    choice: String,
    base_url: String,
    model: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    let current = config.stt();
    let next = stt_from_choice(&choice, &base_url, &model, &current)?;
    if current.engine == config::SttEngine::WhisperCpp && current.model != next.model {
        engines::whisper::unload();
    }
    config.set_stt(next);
    Ok(())
}

#[tauri::command]
pub fn set_voice_language(language: String, config: tauri::State<'_, Arc<VoiceConfig>>) {
    let mut stt = config.stt();
    stt.language = match language.trim() {
        "" => "auto".to_string(),
        other => other.to_string(),
    };
    config.set_stt(stt);
}

#[tauri::command]
pub fn set_voice_transformation(
    provider: String,
    base_url: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    let base_url = match config::transform_preset_base_url(&provider) {
        Some(preset) => preset.to_string(),
        None if provider == "custom" => normalize_endpoint_url(&base_url)?,
        None => return Err("Choose a supported text provider.".to_string()),
    };
    let provider_changed = config.transformation_config().provider != provider;
    let default_model = if provider_changed {
        config::transform_default_model(&provider)
    } else {
        None
    };
    config.set_transform_endpoint(provider, base_url);
    if let Some(model) = default_model {
        config.set_transform_model(model.to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn set_voice_cleanup(enabled: bool, config: tauri::State<'_, Arc<VoiceConfig>>) {
    config.set_cleanup_dictation(enabled);
}

/// One term per line; returns the normalized list so the UI can show what
/// was kept.
#[tauri::command]
pub fn set_voice_vocabulary(
    text: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Vec<String> {
    let terms = vocabulary::normalize(text.lines().map(str::to_string));
    config.set_vocabulary(terms.clone());
    terms
}

#[tauri::command]
pub fn set_voice_transform_model(
    model: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Transform model cannot be empty.".to_string());
    }
    config.set_transform_model(model.to_string());
    Ok(())
}

#[tauri::command]
pub fn set_voice_delivery(
    mode: String,
    delivery: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    config.set_delivery(&mode, DeliveryBehavior::parse(&delivery)?)
}

#[tauri::command]
pub fn set_voice_interface_sounds(enabled: bool, config: tauri::State<'_, Arc<VoiceConfig>>) {
    config.set_interface_sounds(enabled);
}

#[tauri::command]
pub async fn voice_apple_install(language: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || engines::apple::install(&language))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_voice_microphone_status() -> &'static str {
    crate::macos::microphone_access().as_str()
}

#[tauri::command]
pub fn request_voice_microphone() -> &'static str {
    let status = crate::macos::microphone_access();
    if status == crate::macos::MicrophoneAccess::NotDetermined {
        crate::macos::request_microphone_access();
    }
    status.as_str()
}

#[tauri::command]
pub fn open_voice_microphone_settings() -> Result<(), String> {
    crate::open_path("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
}

#[tauri::command]
pub fn get_voice_accessibility_status() -> bool {
    paste::accessibility_trusted()
}

#[tauri::command]
pub fn request_voice_accessibility() -> bool {
    crate::macos::request_accessibility_access()
}

#[tauri::command]
pub fn open_voice_accessibility_settings() -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open Accessibility settings: {error}"))
}

#[tauri::command]
pub fn set_voice_hotkey(
    app: AppHandle,
    hotkey: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    let hotkey = hotkey.trim().to_string();
    if hotkey.is_empty() {
        return Err("Hotkey cannot be empty.".to_string());
    }
    let reserved_modifier = hotkey.split('+').map(str::trim).any(|part| {
        part.eq_ignore_ascii_case("shift")
            || part.eq_ignore_ascii_case("cmd")
            || part.eq_ignore_ascii_case("command")
            || part.eq_ignore_ascii_case("super")
            || part.eq_ignore_ascii_case("meta")
    });
    if reserved_modifier {
        return Err(
            "Use Option and/or Control for the base shortcut. Shift and Command are reserved for Summary and Prompt."
                .to_string(),
        );
    }
    let previous = config.hotkey();
    app.state::<Arc<VoiceState>>()
        .shortcut_down
        .store(false, Ordering::Release);
    unregister_hotkeys(&app, &previous);
    if let Err(error) = register_hotkeys(&app, &hotkey) {
        let _ = register_hotkeys(&app, &previous);
        return Err(error);
    }
    config.set_hotkey(hotkey);
    Ok(())
}

fn voice_pressed(app: &AppHandle, mode: Mode) {
    let state = app.state::<Arc<VoiceState>>();
    if state.shortcut_down.swap(true, Ordering::AcqRel) {
        log::debug!("[voice] ignored repeated shortcut key-down");
        return;
    }
    match crate::macos::microphone_access() {
        crate::macos::MicrophoneAccess::Authorized => {}
        crate::macos::MicrophoneAccess::NotDetermined => {
            crate::macos::request_microphone_access();
            fail(
                app,
                "Allow microphone access in the macOS prompt, then use the shortcut again.",
            );
            return;
        }
        crate::macos::MicrophoneAccess::Denied => {
            fail(
                app,
                "Microphone access is off. Enable Samlu in System Settings > Privacy & Security > Microphone.",
            );
            return;
        }
        crate::macos::MicrophoneAccess::Restricted => {
            fail(
                app,
                "Microphone access is restricted by this Mac’s privacy policy.",
            );
            return;
        }
    }
    let mut runtime = state.runtime.lock().unwrap();
    match &*runtime {
        // A new dictation replaces a failed one: blocking the shortcut until
        // the failure is dismissed would read as voice being broken.
        Runtime::Idle | Runtime::Failed { .. } => {
            if matches!(&*runtime, Runtime::Failed { .. }) {
                log::info!("[voice] discarding failed dictation for a new recording");
            }
            let pressed_at = Instant::now();
            let target = capture_target(app);
            state.toggle_recording.store(false, Ordering::Relaxed);
            *runtime = Runtime::Starting {
                mode,
                pressed_at,
                released_at: None,
                target: target.clone(),
            };
            drop(runtime);
            let stt = app.state::<Arc<VoiceConfig>>().stt();
            if stt.engine == config::SttEngine::WhisperCpp {
                engines::whisper::preload(std::path::PathBuf::from(stt.model));
            }
            show_status(
                app,
                &target,
                "listening",
                mode_label(mode),
                "Listening",
                "",
                false,
            );
            let app = app.clone();
            std::thread::spawn(move || begin_recording(app));
        }
        Runtime::Recording { .. } => {
            let previous = std::mem::replace(&mut *runtime, Runtime::Processing);
            drop(runtime);
            if let Runtime::Recording {
                recording,
                mode,
                target,
                ..
            } = previous
            {
                spawn_processing(app.clone(), recording, mode, target);
            }
        }
        Runtime::Starting { .. }
        | Runtime::Processing
        | Runtime::Previewing { .. }
        | Runtime::Recovering { .. } => {}
    }
}

fn voice_released(app: &AppHandle) {
    let state = app.state::<Arc<VoiceState>>();
    state.shortcut_down.store(false, Ordering::Release);
    let mut runtime = state.runtime.lock().unwrap();
    match &mut *runtime {
        Runtime::Starting { released_at, .. } => {
            *released_at = Some(Instant::now());
        }
        Runtime::Recording { pressed_at, .. } if pressed_at.elapsed() >= HOLD_THRESHOLD => {
            let previous = std::mem::replace(&mut *runtime, Runtime::Processing);
            drop(runtime);
            if let Runtime::Recording {
                recording,
                mode,
                target,
                ..
            } = previous
            {
                spawn_processing(app.clone(), recording, mode, target);
            }
        }
        Runtime::Recording { mode, target, .. } => {
            let (mode, target) = (*mode, target.clone());
            drop(runtime);
            show_toggle_controls(app, &target, mode);
        }
        _ => {}
    }
}

/// A tap, not a hold: the recording now waits for a second press, so the
/// capsule widens to offer Cancel and Stop.
fn show_toggle_controls(app: &AppHandle, target: &TargetContext, mode: Mode) {
    let state = app.state::<Arc<VoiceState>>();
    if state.toggle_recording.swap(true, Ordering::Relaxed) {
        return;
    }
    show_status(
        app,
        target,
        "listening",
        mode_label(mode),
        "Listening",
        "",
        false,
    );
}

fn begin_recording(app: AppHandle) {
    // Each session starts back at the default anchor: a nudge to get the
    // capsule off something you were reading should not silently become where
    // it lives forever.
    if let Some(state) = app.try_state::<Arc<VoiceState>>() {
        *state.drag_offset.lock().unwrap() = (0.0, 0.0);
    }
    let level_app = app.clone();
    match recorder::Recording::start(move |level| {
        let _ = level_app.emit_to(PREVIEW_LABEL, "voice://level", level);
    }) {
        Ok(recording) => {
            let state = app.state::<Arc<VoiceState>>();
            let previous = {
                let mut runtime = state.runtime.lock().unwrap();
                std::mem::replace(&mut *runtime, Runtime::Idle)
            };
            let Runtime::Starting {
                mode,
                pressed_at,
                released_at,
                target,
            } = previous
            else {
                return;
            };

            if released_at
                .is_some_and(|released| released.duration_since(pressed_at) >= HOLD_THRESHOLD)
            {
                *state.runtime.lock().unwrap() = Runtime::Processing;
                spawn_processing(app, recording, mode, target);
                return;
            }

            let id = state.next_recording_id.fetch_add(1, Ordering::Relaxed);
            let tapped = released_at.is_some();
            *state.runtime.lock().unwrap() = Runtime::Recording {
                id,
                recording,
                mode,
                pressed_at,
                target: target.clone(),
            };
            if tapped {
                show_toggle_controls(&app, &target, mode);
            }
            schedule_recording_limit(app, id);
        }
        Err(error) => fail(&app, &error),
    }
}

fn schedule_recording_limit(app: AppHandle, recording_id: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(MAX_RECORDING_DURATION);
        let state = app.state::<Arc<VoiceState>>();
        let mut runtime = state.runtime.lock().unwrap();
        let should_stop = matches!(&*runtime, Runtime::Recording { id, .. } if *id == recording_id);
        if !should_stop {
            return;
        }
        let previous = std::mem::replace(&mut *runtime, Runtime::Processing);
        drop(runtime);
        if let Runtime::Recording {
            recording,
            mode,
            target,
            ..
        } = previous
        {
            spawn_processing(app, recording, mode, target);
        }
    });
}

fn spawn_processing(
    app: AppHandle,
    recording: recorder::Recording,
    mode: Mode,
    target: TargetContext,
) {
    show_status(
        &app,
        &target,
        "processing",
        mode_label(mode),
        "Transcribing",
        "",
        false,
    );
    tauri::async_runtime::spawn(async move {
        match finish_recording(recording).await {
            Ok(wav) => {
                let capture = Capture {
                    audio: engines::PreparedAudio::new(wav),
                    transcript: None,
                };
                process_capture(app, capture, mode, target).await;
            }
            Err(error) => {
                let _ = app.emit_to(crate::onboarding::LABEL, "voice://error", error.clone());
                fail(&app, &error);
            }
        }
    });
}

async fn finish_recording(recording: recorder::Recording) -> Result<Vec<u8>, String> {
    let wav = tauri::async_runtime::spawn_blocking(move || recording.stop())
        .await
        .map_err(|error| format!("recording task failed: {error}"))??;
    if wav.len() < 2_048 {
        return Err("Recording was too short. Hold the shortcut a little longer.".to_string());
    }
    if wav.len() > 24 * 1024 * 1024 {
        return Err("Recording is too large for the configured provider.".to_string());
    }
    Ok(wav)
}

/// Turns a capture into text and delivers it. Any failure from here on keeps
/// the capture so the user can retry it.
async fn process_capture(app: AppHandle, mut capture: Capture, mode: Mode, target: TargetContext) {
    match produce_text(&app, &mut capture, mode, &target).await {
        Ok(produced) if produced.text.trim().is_empty() => {
            let error = "No speech was recognized. Try again a little closer to the microphone."
                .to_string();
            let _ = app.emit_to(crate::onboarding::LABEL, "voice://error", error.clone());
            fail(&app, &error);
        }
        Ok(produced) => deliver(app, produced, mode, target).await,
        Err(error) => {
            let _ = app.emit_to(crate::onboarding::LABEL, "voice://error", error.clone());
            show_failed(&app, target, capture, mode, &error);
        }
    }
}

/// Text ready for delivery, plus what history needs to know about it.
struct Produced {
    text: String,
    raw: Option<String>,
    cleanup_skipped: bool,
}

async fn produce_text(
    app: &AppHandle,
    capture: &mut Capture,
    mode: Mode,
    target: &TargetContext,
) -> Result<Produced, String> {
    let config = app.state::<Arc<VoiceConfig>>();
    let show_retry = |attempt: u32| {
        show_status(
            app,
            target,
            "processing",
            mode_label(mode),
            &format!("Retrying ({attempt}/{})", cloud::MAX_ATTEMPTS),
            "",
            false,
        );
    };
    let transcript = match &capture.transcript {
        Some(transcript) => transcript.clone(),
        None => {
            let settings = config.stt();
            let api_key = if settings.engine.needs_api_key() {
                Some(config.api_key("transcription")?)
            } else {
                None
            };
            let options = engines::SttOptions {
                settings,
                vocabulary: config.vocabulary(),
                api_key,
            };
            let audio = &capture.audio;
            let transcript =
                cloud::with_retries(|| engines::transcribe(audio, &options), show_retry).await?;
            capture.transcript = Some(transcript.clone());
            transcript
        }
    };
    if transcript.trim().is_empty() || (mode == Mode::Normal && !config.cleanup_dictation()) {
        return Ok(Produced {
            text: transcript,
            raw: None,
            cleanup_skipped: false,
        });
    }
    show_status(
        app,
        target,
        "processing",
        mode_label(mode),
        if mode == Mode::Normal {
            "Cleaning up"
        } else {
            "Structuring"
        },
        "",
        false,
    );
    let transformation = config.transformation_config();
    let vocabulary = config.vocabulary();
    let transformed = match config.api_key("transformation") {
        Ok(key) => {
            cloud::with_retries(
                || cloud::transform(&transcript, mode, &vocabulary, &transformation, &key),
                show_retry,
            )
            .await
        }
        Err(error) => Err(error),
    };
    match transformed {
        Ok(text) => Ok(Produced {
            raw: (text != transcript).then(|| transcript.clone()),
            text,
            cleanup_skipped: false,
        }),
        // Cleanup is polish: losing it must never cost the user their words.
        Err(error) if mode == Mode::Normal => {
            log::warn!("[voice] cleanup skipped: {error}");
            Ok(Produced {
                text: transcript,
                raw: None,
                cleanup_skipped: true,
            })
        }
        Err(error) => Err(error),
    }
}

async fn deliver(app: AppHandle, produced: Produced, mode: Mode, target: TargetContext) {
    let text = produced.text.clone();
    let cleanup_skipped = produced.cleanup_skipped;
    let success = |base: &'static str| -> String {
        if cleanup_skipped {
            format!("{base} · cleanup skipped")
        } else {
            base.to_string()
        }
    };
    // The setup guide shows the transcript itself rather than relying on
    // insertion landing in its own window.
    let _ = app.emit_to(crate::onboarding::LABEL, "voice://result", text.clone());
    let delivery = app.state::<Arc<VoiceConfig>>().delivery(mode);
    record_history(&app, &produced, mode, &target, delivery);
    match delivery {
        DeliveryBehavior::EditablePreview => {
            let state = app.state::<Arc<VoiceState>>();
            *state.runtime.lock().unwrap() = Runtime::Previewing {
                target: target.clone(),
                text: text.clone(),
                mode,
            };
            show_status(
                &app,
                &target,
                "preview",
                mode_output_label(mode),
                "Review before inserting",
                &text,
                true,
            );
        }
        DeliveryBehavior::CopyOnly => match paste::copy(&app, &text) {
            Ok(()) => {
                *app.state::<Arc<VoiceState>>().runtime.lock().unwrap() = Runtime::Idle;
                show_success(&app, &target, mode, &success("Copied"));
            }
            Err(error) => show_recovery(&app, target, text, error, mode),
        },
        DeliveryBehavior::InstantInsert => {
            // Avoid an animation and app switch that cannot succeed. Preserve
            // the result, explain the fallback, then immediately release the
            // runtime for another dictation.
            if !paste::accessibility_trusted() {
                match paste::copy(&app, &text) {
                    Ok(()) => show_clipboard_fallback(&app, &target, mode),
                    Err(error) => show_recovery(&app, target, text, error, mode),
                }
                return;
            }
            match inject_without_blocking(&app, &text, &target.app).await {
                Ok(()) => {
                    *app.state::<Arc<VoiceState>>().runtime.lock().unwrap() = Runtime::Idle;
                    show_success(&app, &target, mode, &success("Inserted"));
                }
                Err(error) => handle_delivery_error(&app, target, text, error, mode),
            }
        }
    }
}

#[tauri::command]
pub async fn voice_preview_accept(
    app: AppHandle,
    text: String,
    state: tauri::State<'_, Arc<VoiceState>>,
) -> Result<(), String> {
    let previous = {
        let mut runtime = state.runtime.lock().unwrap();
        std::mem::replace(&mut *runtime, Runtime::Processing)
    };
    let (target, original_text, mode) = match previous {
        Runtime::Previewing { target, text, mode } => (target, text, mode),
        other => {
            *state.runtime.lock().unwrap() = other;
            return Err("No voice preview is active.".to_string());
        }
    };
    let final_text = if text.trim().is_empty() {
        original_text
    } else {
        text.trim().to_string()
    };
    match inject_without_blocking(&app, &final_text, &target.app).await {
        Ok(()) => {
            *state.runtime.lock().unwrap() = Runtime::Idle;
            update_last_history(&app, history::Outcome::Inserted);
            show_success(&app, &target, mode, "Inserted");
            Ok(())
        }
        Err(error) => {
            handle_delivery_error(&app, target, final_text, error.clone(), mode);
            Err(error)
        }
    }
}

#[tauri::command]
pub fn voice_preview_cancel(app: AppHandle, state: tauri::State<'_, Arc<VoiceState>>) {
    let previous = {
        let mut runtime = state.runtime.lock().unwrap();
        std::mem::replace(&mut *runtime, Runtime::Idle)
    };
    if let Runtime::Previewing { text, .. } = previous {
        update_last_history(&app, history::Outcome::PreviewCancelled);
        let _ = paste::copy(&app, &text);
    }
    hide_preview(&app);
}

#[tauri::command]
pub fn voice_stop(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<VoiceState>>();
    let previous = {
        let mut runtime = state.runtime.lock().unwrap();
        match &*runtime {
            Runtime::Recording { .. } => std::mem::replace(&mut *runtime, Runtime::Processing),
            _ => return Err("No recording is active.".to_string()),
        }
    };
    if let Runtime::Recording {
        recording,
        mode,
        target,
        ..
    } = previous
    {
        spawn_processing(app, recording, mode, target);
    }
    Ok(())
}

#[tauri::command]
pub fn voice_cancel(app: AppHandle) {
    let state = app.state::<Arc<VoiceState>>();
    let previous = {
        let mut runtime = state.runtime.lock().unwrap();
        std::mem::replace(&mut *runtime, Runtime::Idle)
    };
    match previous {
        Runtime::Recording { recording, .. } => {
            std::thread::spawn(move || recording.cancel());
        }
        Runtime::Previewing { text, .. } => {
            let _ = paste::copy(&app, &text);
            update_last_history(&app, history::Outcome::PreviewCancelled);
        }
        Runtime::Recovering { text, .. } => {
            let _ = paste::copy(&app, &text);
        }
        Runtime::Processing => {
            *state.runtime.lock().unwrap() = Runtime::Processing;
            return;
        }
        Runtime::Idle | Runtime::Starting { .. } | Runtime::Failed { .. } => {}
    }
    hide_preview(&app);
}

/// Takes the text a recovery action can work with: an undelivered result, or
/// the transcript of a dictation whose transform failed.
fn take_recoverable_text(
    state: &VoiceState,
    next: Runtime,
) -> Result<(TargetContext, String, Mode), String> {
    let mut runtime = state.runtime.lock().unwrap();
    match std::mem::replace(&mut *runtime, next) {
        Runtime::Recovering { target, text, mode } => Ok((target, text, mode)),
        Runtime::Failed {
            target,
            capture:
                Capture {
                    transcript: Some(text),
                    ..
                },
            mode,
        } => Ok((target, text, mode)),
        other => {
            *runtime = other;
            Err("No voice result is waiting for delivery.".to_string())
        }
    }
}

#[tauri::command]
pub async fn voice_recovery_retry(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<VoiceState>>();
    let previous = {
        let mut runtime = state.runtime.lock().unwrap();
        std::mem::replace(&mut *runtime, Runtime::Processing)
    };
    let (target, text, mode) = match previous {
        Runtime::Recovering { target, text, mode } => (target, text, mode),
        Runtime::Failed {
            target,
            capture,
            mode,
        } => {
            let detail = if capture.transcript.is_some() {
                "Structuring"
            } else {
                "Transcribing"
            };
            show_status(
                &app,
                &target,
                "processing",
                mode_label(mode),
                detail,
                "",
                false,
            );
            process_capture(app.clone(), capture, mode, target).await;
            return Ok(());
        }
        other => {
            *state.runtime.lock().unwrap() = other;
            return Err("No voice result is waiting for delivery.".to_string());
        }
    };

    show_status(
        &app,
        &target,
        "processing",
        "Voice result",
        "Trying insertion again",
        "",
        false,
    );
    match inject_without_blocking(&app, &text, &target.app).await {
        Ok(()) => {
            *state.runtime.lock().unwrap() = Runtime::Idle;
            show_success(&app, &target, mode, "Inserted");
            Ok(())
        }
        Err(error) => {
            handle_delivery_error(&app, target, text, error.clone(), mode);
            Err(error)
        }
    }
}

#[tauri::command]
pub fn voice_recovery_copy(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<VoiceState>>();
    let (target, text, mode) = take_recoverable_text(&state, Runtime::Idle)?;
    paste::copy(&app, &text)?;
    show_success(&app, &target, mode, "Copied");
    Ok(())
}

#[tauri::command]
pub fn voice_recovery_preview(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<VoiceState>>();
    let (target, text, mode) = take_recoverable_text(&state, Runtime::Processing)?;
    *state.runtime.lock().unwrap() = Runtime::Previewing {
        target: target.clone(),
        text: text.clone(),
        mode,
    };
    show_status(
        &app,
        &target,
        "preview",
        "Voice result",
        "Edit before inserting",
        &text,
        true,
    );
    Ok(())
}

fn show_status(
    app: &AppHandle,
    target: &TargetContext,
    state_name: &str,
    title: &str,
    detail: &str,
    text: &str,
    editable: bool,
) -> u64 {
    let config = app.try_state::<Arc<VoiceConfig>>();
    let interface_sounds = config
        .as_ref()
        .map(|config| config.interface_sounds())
        .unwrap_or(false);
    let controls = state_name == "listening"
        && app
            .state::<Arc<VoiceState>>()
            .toggle_recording
            .load(Ordering::Relaxed);
    let chip = state_name == "listening" && title != mode_label(Mode::Normal);
    let (width, height) = state_size(state_name, detail, chip, controls);
    let payload = PreviewPayload {
        state: state_name.to_string(),
        mode: title.to_string(),
        title: title.to_string(),
        detail: detail.to_string(),
        text: text.to_string(),
        editable,
        interface_sounds,
        controls,
        width,
        height,
    };
    let _ = app.emit_to(crate::onboarding::LABEL, "voice://status", state_name);
    let generation = app
        .state::<Arc<VoiceState>>()
        .presentation_generation
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    if let Some(window) = app.get_webview_window(PREVIEW_LABEL) {
        let already_visible = window.is_visible().unwrap_or(false);
        let interactive =
            editable || matches!(state_name, "recovery" | "failed") || state_name == "listening";
        crate::macos::set_ignores_mouse_events(&window, !interactive);
        // Re-presenting orders the window out and back in, which reads as a
        // flicker mid-session. Only do it for a genuine arrival; while the
        // capsule is already up, resizing in place lets the webview animate
        // one surface from one state into the next.
        place_window(&window, target, width, height, !already_visible);
        if editable {
            crate::macos::focus_overlay(&window);
        }
    }
    app.state::<Arc<VoiceState>>()
        .present_or_queue(app, payload);
    generation
}

fn emit_preview(app: &AppHandle, payload: PreviewPayload) {
    if let Err(error) = app.emit_to(PREVIEW_LABEL, "voice://state", payload) {
        log::warn!("[voice] failed to update preview: {error}");
    }
}

async fn inject_without_blocking(
    app: &AppHandle,
    text: &str,
    target: &paste::TargetApp,
) -> Result<(), String> {
    let delivery_app = app.clone();
    let delivery_text = text.to_string();
    let delivery_target = target.clone();
    tauri::async_runtime::spawn_blocking(move || {
        paste::inject_to_bundle(&delivery_app, &delivery_text, &delivery_target)
    })
    .await
    .map_err(|error| format!("voice delivery task failed: {error}"))?
}

#[tauri::command]
pub fn voice_preview_ready(app: AppHandle, state: tauri::State<'_, Arc<VoiceState>>) {
    state
        .presentation_ready
        .store(true, std::sync::atomic::Ordering::Release);
    if let Some(payload) = state.pending_presentation.lock().unwrap().take() {
        emit_preview(&app, payload);
    }
}

fn hide_preview(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<VoiceState>>() else {
        if let Some(window) = app.get_webview_window(PREVIEW_LABEL) {
            let _ = window.hide();
        }
        return;
    };
    let generation = state
        .presentation_generation
        .fetch_add(1, Ordering::Relaxed)
        + 1;

    // Let the surface retrace its entrance before the window goes away.
    // Hiding first would make every dismissal a hard cut.
    let _ = app.emit_to(PREVIEW_LABEL, "voice://dismiss", ());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(PREVIEW_EXIT_MS));
        let Some(state) = app.try_state::<Arc<VoiceState>>() else {
            return;
        };
        // A new presentation during the exit means this window is wanted
        // again; leave it on screen.
        if state.presentation_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        if let Some(window) = app.get_webview_window(PREVIEW_LABEL) {
            let _ = window.hide();
        }
    });
}

fn hide_preview_if_current(app: &AppHandle, generation: u64) {
    let Some(state) = app.try_state::<Arc<VoiceState>>() else {
        return;
    };
    if state.presentation_generation.load(Ordering::Relaxed) != generation {
        return;
    }
    hide_preview(app);
}

fn show_success(app: &AppHandle, target: &TargetContext, mode: Mode, detail: &str) {
    let generation = show_status(app, target, "success", mode_label(mode), detail, "", false);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(850));
        hide_preview_if_current(&app, generation);
    });
}

fn handle_delivery_error(
    app: &AppHandle,
    target: TargetContext,
    text: String,
    error: String,
    mode: Mode,
) {
    if paste::is_accessibility_error(&error) {
        show_clipboard_fallback(app, &target, mode);
    } else {
        show_recovery(app, target, text, error, mode);
    }
}

fn show_clipboard_fallback(app: &AppHandle, target: &TargetContext, mode: Mode) {
    log::info!("[voice] insertion unavailable; result copied for manual paste");
    *app.state::<Arc<VoiceState>>().runtime.lock().unwrap() = Runtime::Idle;
    let generation = show_status(
        app,
        target,
        "copied",
        mode_label(mode),
        "Couldn’t insert\nCopied — paste with ⌘V",
        "",
        false,
    );
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2_600));
        hide_preview_if_current(&app, generation);
    });
}

fn show_recovery(app: &AppHandle, target: TargetContext, text: String, error: String, mode: Mode) {
    log::warn!("[voice] delivery needs recovery: {error}");
    let detail = if paste::is_accessibility_error(&error) {
        "Turn on Accessibility to insert automatically. Your words are on the clipboard."
    } else {
        "Samlu couldn’t reach the original field. Your words are right here."
    };
    *app.state::<Arc<VoiceState>>().runtime.lock().unwrap() = Runtime::Recovering {
        target: target.clone(),
        text,
        mode,
    };
    show_status(
        app,
        &target,
        "recovery",
        "Result is safe",
        detail,
        "",
        false,
    );
}

/// Unlike `fail`, this stays up until the user acts: auto-dismissing would
/// silently drop the recording it is holding.
fn show_failed(app: &AppHandle, target: TargetContext, capture: Capture, mode: Mode, error: &str) {
    log::warn!("[voice] processing failed; keeping the recording for retry: {error}");
    let title = if capture.transcript.is_some() {
        "Transcript is safe"
    } else {
        "Recording is safe"
    };
    // The transcript tells the presentation whether Copy and Preview apply.
    let transcript = capture.transcript.clone().unwrap_or_default();
    *app.state::<Arc<VoiceState>>().runtime.lock().unwrap() = Runtime::Failed {
        target: target.clone(),
        capture,
        mode,
    };
    show_status(app, &target, "failed", title, error, &transcript, false);
}

fn fail(app: &AppHandle, error: &str) {
    log::error!("[voice] {error}");
    if let Some(state) = app.try_state::<Arc<VoiceState>>() {
        *state.runtime.lock().unwrap() = Runtime::Idle;
    }
    let target = capture_target(app);
    let generation = show_status(app, &target, "error", "Voice unavailable", error, "", false);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        hide_preview_if_current(&app, generation);
    });
}

fn capture_target(_app: &AppHandle) -> TargetContext {
    TargetContext {
        // NSWorkspace target capture is permission-independent and preserves
        // the original editor even when Accessibility is granted mid-session.
        app: paste::frontmost_target(),
        display: crate::macos::active_screen_frame(),
    }
}

/// Logical window size for a presentation state. Also handed to the webview so
/// it can animate its surface to the same bounds.
fn state_size(state_name: &str, detail: &str, chip: bool, controls: bool) -> (f64, f64) {
    let (width, height) = match state_name {
        "listening" => {
            let mut width = LISTENING_WIDTH;
            if chip {
                width += LISTENING_CHIP_WIDTH;
            }
            if controls {
                width += LISTENING_CONTROLS_WIDTH;
            }
            (width, PILL_HEIGHT)
        }
        "success" if detail.contains('·') => (LANDED_WIDE_WIDTH, LANDED_HEIGHT),
        "success" => (LANDED_WIDTH, LANDED_HEIGHT),
        "copied" => (COPIED_WIDTH, COPIED_HEIGHT),
        "error" => (NOTICE_WIDTH, NOTICE_HEIGHT),
        "preview" => (EDITOR_WIDTH, EDITOR_HEIGHT),
        "recovery" | "failed" => (RECOVERY_WIDTH, RECOVERY_HEIGHT),
        _ => (CAPSULE_WIDTH, PILL_HEIGHT),
    };
    (width + SURFACE_INSET, height + SURFACE_INSET)
}

/// Puts the capsule at the bottom centre of the session's display, plus any
/// drag offset, in AppKit points. Everything stays in points: converting
/// through physical pixels uses the scale of whichever display the window is
/// on now, which sent it back to the wrong screen on mixed-DPI setups.
fn place_window(
    window: &tauri::WebviewWindow,
    target: &TargetContext,
    width: f64,
    height: f64,
    present: bool,
) {
    let Some(display) = target.display.or_else(crate::macos::active_screen_frame) else {
        let _ = window.set_size(LogicalSize::new(width, height));
        if present {
            let _ = window.show();
        }
        return;
    };
    let Some(state) = window.app_handle().try_state::<Arc<VoiceState>>() else {
        return;
    };
    let mut offset = state.drag_offset.lock().unwrap();
    let (x, y) = capsule_origin(&display, width, height, &mut offset);
    *state.placement.lock().unwrap() = Some((display, width, height));
    drop(offset);
    if !crate::macos::set_overlay_frame(window, x, y, width, height, present) {
        let _ = window.set_size(LogicalSize::new(width, height));
        if present {
            let _ = window.show();
        }
    }
}

/// Bottom-centre origin for a capsule of this size, moved by the user's drag
/// offset (points, y down) and kept on the display. The offset is rewritten
/// to what was actually applied, so dragging into an edge never banks travel
/// that has to be dragged back out.
fn capsule_origin(
    display: &crate::macos::ScreenFrame,
    width: f64,
    height: f64,
    offset: &mut (f64, f64),
) -> (f64, f64) {
    let base_x = display.x + (display.width - width) / 2.0;
    let base_y = display.y + BOTTOM_MARGIN;
    let (x, y) = clamp_to_display(base_x + offset.0, base_y - offset.1, width, height, display);
    *offset = (x - base_x, base_y - y);
    (x, y)
}

/// Keeps the capsule wholly on its display, so a drag can never strand it
/// half off an edge where its buttons are unreachable.
fn clamp_to_display(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    display: &crate::macos::ScreenFrame,
) -> (f64, f64) {
    let max_x = display.x + (display.width - width).max(0.0);
    let max_y = display.y + (display.height - height).max(0.0);
    (x.clamp(display.x, max_x), y.clamp(display.y, max_y))
}

/// Moves the capsule by a pointer delta in logical pixels. The webview tracks
/// the pointer 1:1 and hands the release velocity back through the same call,
/// so the window keeps moving with the gesture instead of jumping to a
/// nearest slot.
#[tauri::command]
pub fn voice_preview_drag(app: AppHandle, dx: f64, dy: f64) {
    let Some(window) = app.get_webview_window(PREVIEW_LABEL) else {
        return;
    };
    let Some(state) = app.try_state::<Arc<VoiceState>>() else {
        return;
    };
    let Some((display, width, height)) = *state.placement.lock().unwrap() else {
        return;
    };
    let mut offset = state.drag_offset.lock().unwrap();
    offset.0 += dx;
    offset.1 += dy;
    let (x, y) = capsule_origin(&display, width, height, &mut offset);
    drop(offset);
    crate::macos::set_overlay_frame(&window, x, y, width, height, false);
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "Dictation",
        Mode::Summarize => "Summarize",
        Mode::Prompt => "Prompt mode",
    }
}

fn mode_output_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "Transcript",
        Mode::Summarize => "Summary",
        Mode::Prompt => "Detailed prompt",
    }
}

fn history_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "dictation",
        Mode::Summarize => "summary",
        Mode::Prompt => "prompt",
    }
}

fn record_history(
    app: &AppHandle,
    produced: &Produced,
    mode: Mode,
    target: &TargetContext,
    delivery: DeliveryBehavior,
) {
    let config = app.state::<Arc<VoiceConfig>>();
    if !config.keep_history() {
        return;
    }
    let outcome = if produced.cleanup_skipped {
        history::Outcome::CleanupSkipped
    } else {
        match delivery {
            DeliveryBehavior::InstantInsert => history::Outcome::Inserted,
            DeliveryBehavior::CopyOnly => history::Outcome::Copied,
            DeliveryBehavior::EditablePreview => history::Outcome::Previewing,
        }
    };
    let entry = history::HistoryEntry::new(
        history_mode(mode),
        produced.text.clone(),
        produced.raw.clone(),
        config.engine_label(),
        target.app.bundle_id.clone().unwrap_or_default(),
        outcome,
    );
    *app.state::<Arc<VoiceState>>()
        .last_history_id
        .lock()
        .unwrap() = Some(entry.id.clone());
    app.state::<Arc<history::VoiceHistory>>().record(entry);
}

fn update_last_history(app: &AppHandle, outcome: history::Outcome) {
    let id = app
        .state::<Arc<VoiceState>>()
        .last_history_id
        .lock()
        .unwrap()
        .take();
    if let Some(id) = id {
        app.state::<Arc<history::VoiceHistory>>()
            .set_outcome(&id, outcome);
    }
}

#[tauri::command]
pub fn get_voice_history(
    query: String,
    history: tauri::State<'_, Arc<history::VoiceHistory>>,
) -> Vec<history::HistoryEntry> {
    history.search(&query, history::CAPACITY)
}

#[tauri::command]
pub fn clear_voice_history(history: tauri::State<'_, Arc<history::VoiceHistory>>) {
    history.clear();
}

#[tauri::command]
pub fn set_voice_keep_history(
    enabled: bool,
    config: tauri::State<'_, Arc<VoiceConfig>>,
    history: tauri::State<'_, Arc<history::VoiceHistory>>,
) {
    config.set_keep_history(enabled);
    if !enabled {
        history.clear();
    }
}

fn validate_role(role: &str) -> Result<(), String> {
    if matches!(role, "transcription" | "transformation") {
        Ok(())
    } else {
        Err("Voice endpoint role must be transcription or transformation.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{SttEngine, SttSettings};

    fn display() -> crate::macos::ScreenFrame {
        // A secondary display to the left of the primary one, as AppKit
        // reports it: negative x, its own bottom edge.
        crate::macos::ScreenFrame {
            x: -1920.0,
            y: 120.0,
            width: 1920.0,
            height: 1080.0,
        }
    }

    #[test]
    fn capsule_sits_bottom_centre_of_its_display() {
        let mut offset = (0.0, 0.0);
        let (x, y) = capsule_origin(&display(), 200.0, 60.0, &mut offset);
        assert_eq!(x, -1920.0 + (1920.0 - 200.0) / 2.0);
        assert_eq!(y, 120.0 + BOTTOM_MARGIN);
        assert_eq!(offset, (0.0, 0.0));
    }

    #[test]
    fn dragging_moves_in_points_with_y_down() {
        let mut offset = (40.0, -100.0);
        let (x, y) = capsule_origin(&display(), 200.0, 60.0, &mut offset);
        assert_eq!(x, -1920.0 + 860.0 + 40.0);
        assert_eq!(y, 120.0 + BOTTOM_MARGIN + 100.0);
    }

    #[test]
    fn dragging_past_an_edge_does_not_bank_travel() {
        let mut offset = (5000.0, 5000.0);
        let (x, y) = capsule_origin(&display(), 200.0, 60.0, &mut offset);
        assert_eq!(x, -1920.0 + 1920.0 - 200.0);
        assert_eq!(y, 120.0);
        // The offset now records only what was applied, so dragging back
        // moves the capsule immediately.
        assert_eq!(offset, (860.0, BOTTOM_MARGIN));
    }

    #[test]
    fn listening_grows_for_the_mode_chip_and_tap_controls() {
        let (bare, _) = state_size("listening", "", false, false);
        let (chip, _) = state_size("listening", "", true, false);
        let (both, height) = state_size("listening", "", true, true);
        assert!(bare < chip && chip < both);
        assert_eq!(height, PILL_HEIGHT + SURFACE_INSET);
    }

    #[test]
    fn choices_map_to_engine_settings_and_back() {
        let current = SttSettings::default();
        for (choice, engine) in [
            ("groq", SttEngine::OpenaiCompat),
            ("openai", SttEngine::OpenaiCompat),
            ("deepgram", SttEngine::Deepgram),
            ("elevenlabs", SttEngine::Elevenlabs),
            ("whisper_cpp", SttEngine::WhisperCpp),
            ("apple", SttEngine::Apple),
        ] {
            let settings = stt_from_choice(choice, "", "", &current).unwrap();
            assert_eq!(settings.engine, engine, "{choice}");
            assert_eq!(stt_choice(&settings), choice);
            assert_eq!(settings.language, "auto");
        }
        let custom = stt_from_choice("custom", "https://stt.example/v1/", "m", &current).unwrap();
        assert_eq!(custom.base_url.as_deref(), Some("https://stt.example/v1"));
        assert_eq!(stt_choice(&custom), "custom");
        assert!(stt_from_choice("custom", "ftp://x", "m", &current).is_err());
        assert!(stt_from_choice("nope", "", "", &current).is_err());
    }

    #[test]
    fn switching_cloud_presets_uses_their_default_model() {
        let current = SttSettings::default();
        assert_eq!(
            stt_from_choice("openai", "", "", &current).unwrap().model,
            "gpt-4o-transcribe"
        );
        assert_eq!(
            stt_from_choice("deepgram", "", "", &current).unwrap().model,
            "nova-3"
        );
        assert_eq!(
            stt_from_choice("groq", "", "my-model", &current)
                .unwrap()
                .model,
            "my-model"
        );
    }

    #[test]
    fn derives_mode_shortcuts_from_a_simple_base() {
        assert_eq!(
            derive_shortcuts("Alt+V"),
            (
                "Alt+V".to_string(),
                "Alt+Shift+V".to_string(),
                "Alt+Cmd+V".to_string()
            )
        );
    }

    #[test]
    fn provider_url_requires_https_or_an_actual_loopback_host() {
        assert_eq!(
            normalize_endpoint_url(" https://speech.example/v1/ ").unwrap(),
            "https://speech.example/v1"
        );
        assert!(normalize_endpoint_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(normalize_endpoint_url("http://localhost:8080/v1").is_ok());
        assert!(normalize_endpoint_url("http://[::1]:8080/v1").is_ok());
        assert!(normalize_endpoint_url("http://127.0.0.1.example/v1").is_err());
        assert!(normalize_endpoint_url("https://token@speech.example/v1").is_err());
        assert!(normalize_endpoint_url("http://speech.example/v1").is_err());
    }
}

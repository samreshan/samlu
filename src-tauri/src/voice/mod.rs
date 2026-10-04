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
    utils::config::Color, AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const HOLD_THRESHOLD: Duration = Duration::from_millis(350);
const MAX_RECORDING_DURATION: Duration = Duration::from_secs(120);
const PREVIEW_LABEL: &str = "voice-preview";
const CAPSULE_WIDTH: f64 = 260.0;
const CAPSULE_HEIGHT: f64 = 72.0;
/// The pet capsule is a square: just Samlu reacting to your voice, plus one
/// line of status. No waveform, no elapsed timer.
const PET_CAPSULE_WIDTH: f64 = 108.0;
const PET_CAPSULE_HEIGHT: f64 = 88.0;
const PET_FALLBACK_WIDTH: f64 = 180.0;
const PET_FALLBACK_HEIGHT: f64 = 106.0;
const RECOVERY_WIDTH: f64 = 440.0;
const RECOVERY_HEIGHT: f64 = 142.0;
const EDITOR_WIDTH: f64 = 540.0;
const EDITOR_HEIGHT: f64 = 268.0;
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
struct DisplayAnchor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

#[derive(Clone, Debug)]
struct TargetContext {
    app: paste::TargetApp,
    display: Option<DisplayAnchor>,
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
    pet: bool,
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
    .inner_size(CAPSULE_WIDTH, CAPSULE_HEIGHT)
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

#[tauri::command]
pub fn get_voice_settings(config: tauri::State<'_, Arc<VoiceConfig>>) -> serde_json::Value {
    let transcription = config.transcription_config();
    let transformation = config.transformation_config();
    serde_json::json!({
        "separateProviders": config.separate_providers(),
        "transcription": {
            "provider": transcription.provider,
            "baseUrl": transcription.base_url,
            "model": transcription.model,
            "hasApiKey": config.has_api_key("transcription"),
        },
        "transformation": {
            "provider": transformation.provider,
            "baseUrl": transformation.base_url,
            "model": transformation.model,
            "hasApiKey": config.has_api_key("transformation"),
        },
        "hotkey": config.hotkey(),
        "delivery": {
            "dictation": config.delivery(Mode::Normal),
            "summary": config.delivery(Mode::Summarize),
            "prompt": config.delivery(Mode::Prompt),
        },
        "interfaceSounds": config.interface_sounds(),
        "petCapsule": config.pet_capsule(),
        "keepHistory": config.keep_history(),
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
pub fn set_voice_endpoint(
    role: String,
    provider: String,
    base_url: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    validate_role(&role)?;
    if provider != "groq" && provider != "custom" {
        return Err("Provider must be Groq or OpenAI-compatible.".to_string());
    }
    let base_url = if provider == "groq" {
        config::DEFAULT_BASE_URL.to_string()
    } else {
        normalize_endpoint_url(&base_url)?
    };
    config.set_endpoint(&role, provider, base_url);
    Ok(())
}

#[tauri::command]
pub fn set_voice_separate_providers(enabled: bool, config: tauri::State<'_, Arc<VoiceConfig>>) {
    config.set_separate_providers(enabled);
}

#[tauri::command]
pub fn set_voice_stt_model(
    model: String,
    config: tauri::State<'_, Arc<VoiceConfig>>,
) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Speech model cannot be empty.".to_string());
    }
    config.set_stt_model(model.to_string());
    Ok(())
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
pub fn set_voice_pet_capsule(enabled: bool, config: tauri::State<'_, Arc<VoiceConfig>>) {
    config.set_pet_capsule(enabled);
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
        _ => {}
    }
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
            *state.runtime.lock().unwrap() = Runtime::Recording {
                id,
                recording,
                mode,
                pressed_at,
                target,
            };
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
            let error = "The provider returned an empty transcript.".to_string();
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
            // Samlu ferries the result to the pointer before it lands. Short
            // by design — this delay sits between releasing the hotkey and
            // seeing the text.
            if app.state::<Arc<VoiceConfig>>().pet_capsule() {
                show_status(
                    &app,
                    &target,
                    "delivering",
                    mode_label(mode),
                    "Inserting",
                    "",
                    false,
                );
                if crate::pet::carry(&app) {
                    // The travelling pet is now the presentation; avoid
                    // leaving a duplicate capsule behind.
                    hide_preview(&app);
                    let _ = tauri::async_runtime::spawn_blocking(|| {
                        std::thread::sleep(Duration::from_millis(crate::pet::CARRY_DURATION_MS));
                    })
                    .await;
                }
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
        Runtime::Previewing { text, .. } | Runtime::Recovering { text, .. } => {
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
    let pet = config
        .as_ref()
        .map(|config| config.pet_capsule())
        .unwrap_or(true);
    let (width, height) = state_size(state_name, pet);
    let payload = PreviewPayload {
        state: state_name.to_string(),
        mode: title.to_string(),
        title: title.to_string(),
        detail: detail.to_string(),
        text: text.to_string(),
        editable,
        interface_sounds,
        pet,
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
        position_window(&window, target, width, height);
        let interactive = editable
            || matches!(state_name, "recovery" | "failed")
            || (state_name == "listening" && !pet);
        crate::macos::set_ignores_mouse_events(&window, !interactive);
        // Re-presenting orders the window out and back in, which reads as a
        // flicker mid-session. Only do it for a genuine arrival; while the
        // capsule is already up, resizing in place lets the webview animate
        // one surface from one state into the next.
        if !already_visible {
            let presented =
                crate::macos::present_overlay_at_cursor(&window, width, height, BOTTOM_MARGIN);
            if !presented {
                let _ = window.show();
            }
        }
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
        "Enable Accessibility for automatic insertion. Your result is already on the clipboard."
    } else {
        "Samlu could not restore the original field. Your result is safe."
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

fn capture_target(app: &AppHandle) -> TargetContext {
    let display = app.get_webview_window(PREVIEW_LABEL).and_then(|window| {
        let position = window.cursor_position().ok()?;
        let monitor = window
            .monitor_from_point(position.x, position.y)
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten())?;
        Some(DisplayAnchor {
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
            scale: monitor.scale_factor(),
        })
    });
    TargetContext {
        // NSWorkspace target capture is permission-independent and preserves
        // the original editor even when Accessibility is granted mid-session.
        app: paste::frontmost_target(),
        display,
    }
}

/// Logical window size for a presentation state. Also handed to the webview so
/// it can animate its surface to the same bounds.
fn state_size(state_name: &str, pet: bool) -> (f64, f64) {
    match state_name {
        "preview" => (EDITOR_WIDTH, EDITOR_HEIGHT),
        "recovery" | "failed" => (RECOVERY_WIDTH, RECOVERY_HEIGHT),
        "copied" if pet => (PET_FALLBACK_WIDTH, PET_FALLBACK_HEIGHT),
        "listening" | "processing" | "delivering" if pet => (PET_CAPSULE_WIDTH, PET_CAPSULE_HEIGHT),
        _ => (CAPSULE_WIDTH, CAPSULE_HEIGHT),
    }
}

fn position_window(window: &tauri::WebviewWindow, target: &TargetContext, width: f64, height: f64) {
    if let Err(error) = window.set_size(LogicalSize::new(width, height)) {
        log::warn!("[voice] could not resize preview window: {error}");
    }
    let Some(display) = target.display.as_ref() else {
        return;
    };
    let physical_width = width * display.scale;
    let physical_height = height * display.scale;
    let (offset_x, offset_y) = window
        .app_handle()
        .try_state::<Arc<VoiceState>>()
        .map(|state| *state.drag_offset.lock().unwrap())
        .unwrap_or((0.0, 0.0));
    let x = display.x as f64 + (display.width as f64 - physical_width) / 2.0 + offset_x;
    let y =
        display.y as f64 + display.height as f64 - physical_height - BOTTOM_MARGIN * display.scale
            + offset_y;
    let (x, y) = clamp_to_display(
        x,
        y,
        physical_width,
        physical_height,
        display.x as f64,
        display.y as f64,
        display.width as f64,
        display.height as f64,
    );
    if let Err(error) = window.set_position(PhysicalPosition::new(x, y)) {
        log::warn!("[voice] could not position preview window: {error}");
    }
}

/// Keeps the capsule wholly on its display, so a drag can never strand it
/// half off an edge where its buttons are unreachable.
#[allow(clippy::too_many_arguments)]
fn clamp_to_display(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    display_x: f64,
    display_y: f64,
    display_width: f64,
    display_height: f64,
) -> (f64, f64) {
    let max_x = display_x + (display_width - width).max(0.0);
    let max_y = display_y + (display_height - height).max(0.0);
    (
        x.clamp(display_x, max_x.max(display_x)),
        y.clamp(display_y, max_y.max(display_y)),
    )
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
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let mut x = position.x as f64 + dx * scale;
    let mut y = position.y as f64 + dy * scale;

    if let Ok(Some(monitor)) = window.current_monitor() {
        let origin = monitor.position();
        let bounds = monitor.size();
        (x, y) = clamp_to_display(
            x,
            y,
            size.width as f64,
            size.height as f64,
            origin.x as f64,
            origin.y as f64,
            bounds.width as f64,
            bounds.height as f64,
        );
    }

    if let Some(state) = app.try_state::<Arc<VoiceState>>() {
        let mut offset = state.drag_offset.lock().unwrap();
        offset.0 += x - position.x as f64;
        offset.1 += y - position.y as f64;
    }
    let _ = window.set_position(PhysicalPosition::new(x, y));
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
    use super::{derive_shortcuts, normalize_endpoint_url};

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

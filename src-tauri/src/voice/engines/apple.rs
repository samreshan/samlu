//! Apple SpeechAnalyzer through the Swift bridge in `native/SamluSpeech.swift`.

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::ProviderError;
use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};

pub const NOT_INSTALLED: &str =
    "Apple speech for this language isn't installed yet. Install it in Voice settings.";

extern "C" {
    fn samlu_apple_speech_available() -> bool;
    fn samlu_apple_install(locale: *const c_char, out: *mut *mut c_char) -> i32;
    fn samlu_apple_transcribe(
        path: *const c_char,
        locale: *const c_char,
        terms: *const c_char,
        out: *mut *mut c_char,
    ) -> i32;
    fn samlu_apple_free(pointer: *mut c_char);
}

pub fn available() -> bool {
    // SAFETY: takes no arguments; returns false before macOS 26.
    unsafe { samlu_apple_speech_available() }
}

/// Apple has no automatic language detection; an empty identifier makes the
/// bridge use the system locale.
fn bridge_locale(language: &str) -> &str {
    match language.trim() {
        "auto" => "",
        other => other,
    }
}

fn status_message(status: i32, detail: &str) -> String {
    match status {
        2 => NOT_INSTALLED.to_string(),
        3 => {
            let language = if detail.is_empty() {
                "your system language"
            } else {
                detail
            };
            format!(
                "Apple speech doesn't support {language}. Choose another language in Voice settings."
            )
        }
        4 => detail.to_string(),
        _ => format!("Apple speech failed: {detail}"),
    }
}

fn c_string(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "text contains a NUL byte".to_string())
}

/// Takes ownership of a string the bridge allocated.
unsafe fn take(out: *mut c_char) -> String {
    if out.is_null() {
        return String::new();
    }
    let text = CStr::from_ptr(out).to_string_lossy().into_owned();
    samlu_apple_free(out);
    text
}

/// Blocking. Installs the speech assets for `language` and returns the
/// resolved BCP-47 locale.
pub fn install(language: &str) -> Result<String, String> {
    let locale = c_string(bridge_locale(language))?;
    let mut out = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated input; `out` receives a malloc'd string.
    let status = unsafe { samlu_apple_install(locale.as_ptr(), &mut out) };
    let message = unsafe { take(out) };
    if status == 0 {
        Ok(message)
    } else {
        Err(status_message(status, &message))
    }
}

/// Blocking. Transcribes a WAV file.
fn run_bridge(wav: &Path, language: &str, vocabulary: &[String]) -> Result<String, String> {
    let path = c_string(&wav.to_string_lossy())?;
    let locale = c_string(bridge_locale(language))?;
    let terms = c_string(&vocabulary.join("\n"))?;
    let mut out = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated inputs; `out` receives a malloc'd string.
    let status =
        unsafe { samlu_apple_transcribe(path.as_ptr(), locale.as_ptr(), terms.as_ptr(), &mut out) };
    let message = unsafe { take(out) };
    if status == 0 {
        Ok(message)
    } else {
        Err(status_message(status, &message))
    }
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
    if !available() {
        return Err(ProviderError::permanent(
            "Apple on-device speech requires macOS 26 or later.".to_string(),
        ));
    }
    let wav: PathBuf = std::env::temp_dir().join(format!(
        "samlu-voice-{}-{}.wav",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::write(&wav, &audio.wav).map_err(|error| {
        ProviderError::permanent(format!("could not stage the recording: {error}"))
    })?;
    let language = opts.settings.language.clone();
    let vocabulary = opts.vocabulary.clone();
    let staged = wav.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || run_bridge(&staged, &language, &vocabulary))
            .await;
    let _ = std::fs::remove_file(&wav);
    result
        .map_err(|error| ProviderError::permanent(format!("Apple speech task failed: {error}")))?
        .map_err(ProviderError::permanent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn bridge_statuses_map_to_actionable_messages() {
        assert_eq!(status_message(2, "en-US"), NOT_INSTALLED);
        assert!(status_message(3, "xx").contains("doesn't support xx"));
        assert!(status_message(3, "").contains("your system language"));
        assert_eq!(status_message(1, "boom"), "Apple speech failed: boom");
        assert_eq!(status_message(4, "needs macOS 26"), "needs macOS 26");
    }

    #[test]
    fn auto_language_uses_the_system_locale() {
        assert_eq!(bridge_locale("auto"), "");
        assert_eq!(bridge_locale(""), "");
        assert_eq!(bridge_locale("de"), "de");
    }

    #[test]
    #[ignore = "requires macOS 26 and installs English speech assets"]
    fn transcribes_a_spoken_sentence() {
        assert!(available());
        install("en-US").unwrap();
        let wav = std::env::temp_dir().join("samlu-apple-fox.wav");
        let status = Command::new("say")
            .args(["--file-format=WAVE", "--data-format=LEI16@48000", "-o"])
            .arg(&wav)
            .arg("The quick brown fox jumps over the lazy dog.")
            .status()
            .unwrap();
        assert!(status.success());
        let text = run_bridge(&wav, "en-US", &[]).unwrap();
        assert!(text.to_lowercase().contains("brown fox"), "{text}");
    }
}

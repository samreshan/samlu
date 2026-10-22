//! Speech-to-text engines behind one entry point. Every engine returns the
//! same `ProviderError`, so `cloud::with_retries` and the recovery flow treat
//! them identically. Local engines only produce permanent errors.

pub mod apple;
pub mod bark;
mod deepgram;
mod elevenlabs;
mod openai_compat;
pub mod whisper;

use super::cloud::ProviderError;
use super::config::{SttEngine, SttSettings};
use std::path::Path;
use std::sync::OnceLock;

/// One capture's audio: the WAV cloud engines upload, and the 16 kHz samples
/// local engines need, decoded at most once so a retry reuses them.
pub struct PreparedAudio {
    pub wav: Vec<u8>,
    local: OnceLock<Result<Vec<f32>, String>>,
}

impl PreparedAudio {
    pub fn new(wav: Vec<u8>) -> Self {
        Self {
            wav,
            local: OnceLock::new(),
        }
    }

    pub fn local_samples(&self) -> Result<&[f32], String> {
        self.local
            .get_or_init(|| super::audio::to_local_samples(&self.wav))
            .as_deref()
            .map_err(Clone::clone)
    }
}

pub struct SttOptions {
    pub settings: SttSettings,
    pub vocabulary: Vec<String>,
    pub api_key: Option<String>,
}

impl SttOptions {
    pub fn language(&self) -> Option<String> {
        effective_language(&self.settings.language, &self.settings.model)
    }

    fn api_key(&self) -> Result<&str, ProviderError> {
        self.api_key.as_deref().ok_or_else(|| {
            ProviderError::permanent("No API key saved for this endpoint.".to_string())
        })
    }
}

/// Underdog Bark understands only English. whisper.cpp English-only models
/// are named `*.en.bin` or `*.en-<quant>.bin`.
pub fn is_english_only(model: &str) -> bool {
    if super::models::is_bark_dir(Path::new(model)) {
        return true;
    }
    let name = Path::new(model)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(model)
        .to_ascii_lowercase();
    name.contains(".en.") || name.contains(".en-")
}

/// `None` asks the engine to detect the language.
pub fn effective_language(language: &str, model: &str) -> Option<String> {
    if is_english_only(model) {
        return Some("en".to_string());
    }
    let language = language.trim();
    (!language.is_empty() && language != "auto").then(|| language.to_string())
}

/// The local engine runs whisper.cpp files and Underdog Bark folders.
fn is_bark(model: &str) -> bool {
    super::models::is_bark_dir(Path::new(model.trim()))
}

/// Loads the selected local model in the background.
pub fn preload_local(model: &str) {
    let path = std::path::PathBuf::from(model.trim());
    if is_bark(model) {
        bark::preload(path);
    } else {
        whisper::preload(path);
    }
}

/// Releases whichever local model is resident.
pub fn unload_local() {
    whisper::unload();
    bark::unload();
}

/// Releases local models other than `model`, the one now in use. A Bark
/// load already under way for `model` carries on.
pub fn unload_local_except(model: &str) {
    whisper::unload();
    bark::keep_only(Path::new(model.trim()));
}

pub async fn transcribe(audio: &PreparedAudio, opts: &SttOptions) -> Result<String, ProviderError> {
    match opts.settings.engine {
        SttEngine::OpenaiCompat => openai_compat::transcribe(audio, opts).await,
        SttEngine::Deepgram => deepgram::transcribe(audio, opts).await,
        SttEngine::Elevenlabs => elevenlabs::transcribe(audio, opts).await,
        SttEngine::WhisperCpp if is_bark(&opts.settings.model) => {
            bark::transcribe(audio, opts).await
        }
        SttEngine::WhisperCpp => whisper::transcribe(audio, opts).await,
        SttEngine::Apple => apple::transcribe(audio, opts).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_only_models_are_detected_by_file_name() {
        assert!(is_english_only("ggml-base.en.bin"));
        assert!(is_english_only("/models/ggml-small.en-q5_1.bin"));
        assert!(!is_english_only("/models/ggml-large-v3-turbo.bin"));
        assert!(!is_english_only("whisper-large-v3-turbo"));
        assert!(!is_english_only("/Users/en.person/ggml-base.bin"));
    }

    #[test]
    fn bark_folders_are_english_only() {
        let dir = std::env::temp_dir().join(format!("samlu-bark-en-{}", std::process::id()));
        for file in [
            "BarkEncoder.mlpackage/Manifest.json",
            "decoder.npz",
            "vocab.json",
        ] {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }
        let model = dir.to_string_lossy().into_owned();
        assert!(is_english_only(&model));
        assert!(is_bark(&model));
        assert_eq!(effective_language("de", &model).as_deref(), Some("en"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn language_auto_means_none_and_english_models_force_en() {
        assert_eq!(effective_language("auto", "whisper-large-v3"), None);
        assert_eq!(effective_language("", "whisper-large-v3"), None);
        assert_eq!(
            effective_language("de", "whisper-large-v3").as_deref(),
            Some("de")
        );
        assert_eq!(
            effective_language("de", "/m/ggml-base.en.bin").as_deref(),
            Some("en")
        );
    }

    #[test]
    fn local_samples_are_prepared_once() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 32_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for _ in 0..3_200 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
        let audio = PreparedAudio::new(cursor.into_inner());
        let first = audio.local_samples().unwrap().as_ptr();
        assert_eq!(audio.local_samples().unwrap().len(), 1_600);
        assert_eq!(audio.local_samples().unwrap().as_ptr(), first);
    }
}

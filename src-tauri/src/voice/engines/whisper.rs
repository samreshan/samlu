//! On-device whisper.cpp transcription. The selected model loads when the
//! hotkey is pressed (hidden behind the user speaking), stays resident, and
//! is released after ten idle minutes or when another model is chosen.

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::ProviderError;
use crate::voice::models::{self, MODEL_NOT_FOUND};
use crate::voice::vocabulary;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
const IDLE_CHECK: Duration = Duration::from_secs(60);

struct Loaded {
    path: PathBuf,
    context: Arc<WhisperContext>,
    last_used: Instant,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
/// Serializes loads so a preload and a transcription never load twice.
static LOADING: Mutex<()> = Mutex::new(());

pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        whisper_rs::install_logging_hooks();
        std::thread::spawn(|| loop {
            std::thread::sleep(IDLE_CHECK);
            let mut loaded = LOADED.lock().unwrap();
            if loaded
                .as_ref()
                .is_some_and(|model| model.last_used.elapsed() >= IDLE_UNLOAD)
            {
                *loaded = None;
                log::info!("[voice] unloaded idle whisper model");
            }
        });
    });
}

fn context(path: &Path) -> Result<Arc<WhisperContext>, String> {
    let _loading = LOADING.lock().unwrap();
    if let Some(model) = LOADED.lock().unwrap().as_mut() {
        if model.path == path {
            model.last_used = Instant::now();
            return Ok(model.context.clone());
        }
    }
    models::inspect(path)?;
    let path_str = path.to_str().ok_or_else(|| MODEL_NOT_FOUND.to_string())?;
    let context = Arc::new(
        WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
            .map_err(|error| format!("Could not load the whisper model: {error}"))?,
    );
    *LOADED.lock().unwrap() = Some(Loaded {
        path: path.to_path_buf(),
        context: context.clone(),
        last_used: Instant::now(),
    });
    Ok(context)
}

pub fn preload(path: PathBuf) {
    std::thread::spawn(move || {
        if let Err(error) = context(&path) {
            log::warn!("[voice] whisper preload failed: {error}");
        }
    });
}

pub fn unload() {
    LOADED.lock().unwrap().take();
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(8) as i32)
        .unwrap_or(4)
}

fn run(
    context: &WhisperContext,
    samples: &[f32],
    language: Option<&str>,
    prompt: Option<&str>,
) -> Result<String, String> {
    let mut state = context
        .create_state()
        .map_err(|error| format!("whisper could not start: {error}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads());
    params.set_translate(false);
    params.set_no_context(true);
    params.set_language(Some(language.unwrap_or("auto")));
    if let Some(prompt) = prompt {
        params.set_initial_prompt(prompt);
    }
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, samples)
        .map_err(|error| format!("whisper transcription failed: {error}"))?;
    let mut text = String::new();
    for segment in state.as_iter() {
        let segment = segment
            .to_str_lossy()
            .map_err(|error| format!("whisper returned unreadable text: {error}"))?;
        text.push_str(&segment);
    }
    Ok(text.trim().to_string())
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
    let model = opts.settings.model.trim();
    let path = PathBuf::from(model);
    if model.is_empty() || !path.is_file() {
        return Err(ProviderError::permanent(MODEL_NOT_FOUND.to_string()));
    }
    let samples = audio
        .local_samples()
        .map_err(ProviderError::permanent)?
        .to_vec();
    let language = opts.language();
    let prompt = vocabulary::whisper_prompt(&opts.vocabulary);
    tauri::async_runtime::spawn_blocking(move || {
        let context = context(&path)?;
        run(&context, &samples, language.as_deref(), prompt.as_deref())
    })
    .await
    .map_err(|error| ProviderError::permanent(format!("whisper task failed: {error}")))?
    .map_err(ProviderError::permanent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::config::{SttEngine, SttSettings};
    use std::process::Command;

    fn options(model: &str) -> SttOptions {
        SttOptions {
            settings: SttSettings {
                engine: SttEngine::WhisperCpp,
                preset: None,
                model: model.to_string(),
                base_url: None,
                language: "auto".to_string(),
            },
            vocabulary: Vec::new(),
            api_key: None,
        }
    }

    #[test]
    fn missing_model_fails_permanently_with_guidance() {
        let audio = PreparedAudio::new(Vec::new());
        for model in ["", "/nonexistent/ggml-base.bin"] {
            let opts = options(model);
            let mut attempts = 0;
            let result = tauri::async_runtime::block_on(crate::voice::cloud::with_retries(
                || {
                    attempts += 1;
                    transcribe(&audio, &opts)
                },
                |_| {},
            ));
            assert_eq!(result.unwrap_err(), MODEL_NOT_FOUND);
            assert_eq!(attempts, 1, "local failures must not be retried");
        }
    }

    #[test]
    #[ignore = "downloads ggml-tiny.en (75 MB) and uses macOS `say`"]
    fn transcribes_a_spoken_sentence() {
        let dir = std::env::temp_dir().join("samlu-whisper-test");
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("ggml-tiny.en.bin");
        if !model.exists() {
            let status = Command::new("curl")
                .args(["-sL", "-o"])
                .arg(&model)
                .arg("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin")
                .status()
                .unwrap();
            assert!(status.success());
        }
        let wav = dir.join("fox.wav");
        let status = Command::new("say")
            .args(["--file-format=WAVE", "--data-format=LEI16@48000", "-o"])
            .arg(&wav)
            .arg("The quick brown fox jumps over the lazy dog.")
            .status()
            .unwrap();
        assert!(status.success());
        let audio = PreparedAudio::new(std::fs::read(&wav).unwrap());
        let text =
            tauri::async_runtime::block_on(transcribe(&audio, &options(model.to_str().unwrap())))
                .map_err(|error| format!("{error:?}"))
                .unwrap();
        unload();
        assert!(text.to_lowercase().contains("brown fox"), "{text}");
    }
}

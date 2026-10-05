//! Persisted non-secret voice preferences. Provider credentials live in the
//! macOS Keychain and never enter this JSON file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub const DEFAULT_HOTKEY: &str = "Alt+V";
pub const DEFAULT_STT_MODEL: &str = "whisper-large-v3-turbo";
pub const DEFAULT_TRANSFORM_MODEL: &str = "openai/gpt-oss-120b";
pub const DEFAULT_PROVIDER: &str = "groq";
pub const DEFAULT_BASE_URL: &str = "https://api.groq.com/openai/v1";
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
pub const LMSTUDIO_BASE_URL: &str = "http://localhost:1234/v1";
pub const DEEPGRAM_DEFAULT_MODEL: &str = "nova-3";
pub const ELEVENLABS_DEFAULT_MODEL: &str = "scribe_v2";
const DEEPGRAM_KEYCHAIN_SERVICE: &str = "com.samlu.desktop.voice.deepgram";
const ELEVENLABS_KEYCHAIN_SERVICE: &str = "com.samlu.desktop.voice.elevenlabs";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryBehavior {
    InstantInsert,
    EditablePreview,
    CopyOnly,
}

impl DeliveryBehavior {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "instant_insert" => Ok(Self::InstantInsert),
            "editable_preview" => Ok(Self::EditablePreview),
            "copy_only" => Ok(Self::CopyOnly),
            _ => {
                Err("Delivery must be Instant insert, Editable preview, or Copy only.".to_string())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    OpenaiCompat,
    Deepgram,
    Elevenlabs,
    WhisperCpp,
    Apple,
}

impl SttEngine {
    pub fn needs_api_key(self) -> bool {
        matches!(self, Self::OpenaiCompat | Self::Deepgram | Self::Elevenlabs)
    }
}

/// The selected speech engine. `model` is a provider model name for cloud
/// engines and an absolute file path for whisper.cpp; Apple ignores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttSettings {
    pub engine: SttEngine,
    /// `groq`, `openai`, or `custom` for OpenAI-compatible endpoints.
    pub preset: Option<String>,
    pub model: String,
    pub base_url: Option<String>,
    /// `auto` or an ISO 639-1 code.
    pub language: String,
}

impl Default for SttSettings {
    fn default() -> Self {
        Self {
            engine: SttEngine::OpenaiCompat,
            preset: Some(DEFAULT_PROVIDER.to_string()),
            model: DEFAULT_STT_MODEL.to_string(),
            base_url: Some(DEFAULT_BASE_URL.to_string()),
            language: "auto".to_string(),
        }
    }
}

pub fn transform_preset_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "groq" => Some(DEFAULT_BASE_URL),
        "openai" => Some(OPENAI_BASE_URL),
        "ollama" => Some(OLLAMA_BASE_URL),
        "lmstudio" => Some(LMSTUDIO_BASE_URL),
        _ => None,
    }
}

pub fn transform_default_model(provider: &str) -> Option<&'static str> {
    match provider {
        "groq" => Some(DEFAULT_TRANSFORM_MODEL),
        "openai" => Some("gpt-4.1-mini"),
        "ollama" => Some("llama3.2"),
        _ => None,
    }
}

/// Local model servers accept requests without a key.
pub fn transform_needs_key(provider: &str) -> bool {
    !matches!(provider, "ollama" | "lmstudio")
}

/// `None` for engines that have no key.
fn stt_keychain_service(stt: &SttSettings) -> Option<String> {
    match stt.engine {
        SttEngine::OpenaiCompat => Some(keychain_service(
            stt.preset.as_deref().unwrap_or("custom"),
            stt.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL),
        )),
        SttEngine::Deepgram => Some(DEEPGRAM_KEYCHAIN_SERVICE.to_string()),
        SttEngine::Elevenlabs => Some(ELEVENLABS_KEYCHAIN_SERVICE.to_string()),
        SttEngine::WhisperCpp | SttEngine::Apple => None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    /// `None` only while reading a pre-module config; `migrate` fills it.
    #[serde(default)]
    stt: Option<SttSettings>,
    // Pre-module fields: read once for migration, never written back.
    #[serde(default, skip_serializing)]
    provider: Option<String>,
    #[serde(default, skip_serializing)]
    base_url: Option<String>,
    #[serde(default, skip_serializing)]
    stt_model: Option<String>,
    #[serde(default, skip_serializing)]
    separate_transform_provider: Option<bool>,
    transform_provider: String,
    transform_base_url: String,
    transform_model: String,
    hotkey: String,
    dictation_delivery: DeliveryBehavior,
    summary_delivery: DeliveryBehavior,
    prompt_delivery: DeliveryBehavior,
    interface_sounds: bool,
    cleanup_dictation: bool,
    vocabulary: Vec<String>,
    keep_history: bool,
    /// Model files the user picked by hand; downloaded and discovered models
    /// are found by scanning instead.
    added_models: Vec<String>,
    #[serde(skip_serializing)]
    preview_enabled: bool,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            stt: Some(SttSettings::default()),
            provider: None,
            base_url: None,
            stt_model: None,
            separate_transform_provider: None,
            transform_provider: DEFAULT_PROVIDER.to_string(),
            transform_base_url: DEFAULT_BASE_URL.to_string(),
            transform_model: DEFAULT_TRANSFORM_MODEL.to_string(),
            hotkey: DEFAULT_HOTKEY.to_string(),
            dictation_delivery: DeliveryBehavior::InstantInsert,
            summary_delivery: DeliveryBehavior::InstantInsert,
            prompt_delivery: DeliveryBehavior::EditablePreview,
            interface_sounds: true,
            cleanup_dictation: false,
            vocabulary: Vec::new(),
            keep_history: true,
            added_models: Vec::new(),
            preview_enabled: false,
        }
    }
}

impl Data {
    /// Converts a pre-module config (flat provider fields, optionally shared
    /// with transformation) to the `stt` object. Returns whether anything
    /// changed so the caller can persist the new shape.
    fn migrate(&mut self) -> bool {
        if self.stt.is_some() {
            return false;
        }
        let provider = self
            .provider
            .take()
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
        let base_url = self
            .base_url
            .take()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = self
            .stt_model
            .take()
            .unwrap_or_else(|| DEFAULT_STT_MODEL.to_string());
        // Transformation used to share the speech endpoint unless split.
        if !self.separate_transform_provider.take().unwrap_or(false) {
            self.transform_provider = provider.clone();
            self.transform_base_url = base_url.clone();
        }
        let preset = if provider == DEFAULT_PROVIDER {
            DEFAULT_PROVIDER
        } else {
            "custom"
        };
        self.stt = Some(SttSettings {
            engine: SttEngine::OpenaiCompat,
            preset: Some(preset.to_string()),
            model,
            base_url: Some(base_url),
            language: "auto".to_string(),
        });
        true
    }
}

#[derive(Clone)]
pub struct EndpointConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
}

pub struct VoiceConfig {
    path: PathBuf,
    data: Mutex<Data>,
}

impl VoiceConfig {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("voice-config.json");
        let mut data: Data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        let migrated = data.migrate();
        let config = Self {
            path,
            data: Mutex::new(data),
        };
        if migrated {
            config.save();
        }
        config
    }

    pub fn stt(&self) -> SttSettings {
        self.data.lock().unwrap().stt.clone().unwrap_or_default()
    }

    pub fn set_stt(&self, stt: SttSettings) {
        self.data.lock().unwrap().stt = Some(stt);
        self.save();
    }

    /// Whether dictation can run without further setup.
    pub fn stt_ready(&self) -> bool {
        let stt = self.stt();
        match stt.engine {
            SttEngine::WhisperCpp => {
                super::models::inspect(std::path::Path::new(&stt.model)).is_ok()
            }
            SttEngine::Apple => super::engines::apple::available(),
            _ => self.has_api_key("transcription"),
        }
    }

    pub fn transformation_config(&self) -> EndpointConfig {
        let data = self.data.lock().unwrap();
        EndpointConfig {
            provider: data.transform_provider.clone(),
            base_url: data.transform_base_url.clone(),
            model: data.transform_model.clone(),
        }
    }

    pub fn set_transform_endpoint(&self, provider: String, base_url: String) {
        {
            let mut data = self.data.lock().unwrap();
            data.transform_provider = provider;
            data.transform_base_url = base_url.trim_end_matches('/').to_string();
        }
        self.save();
    }

    pub fn cleanup_dictation(&self) -> bool {
        self.data.lock().unwrap().cleanup_dictation
    }

    pub fn set_cleanup_dictation(&self, enabled: bool) {
        self.data.lock().unwrap().cleanup_dictation = enabled;
        self.save();
    }

    pub fn vocabulary(&self) -> Vec<String> {
        self.data.lock().unwrap().vocabulary.clone()
    }

    pub fn set_vocabulary(&self, terms: Vec<String>) {
        self.data.lock().unwrap().vocabulary = terms;
        self.save();
    }

    pub fn keep_history(&self) -> bool {
        self.data.lock().unwrap().keep_history
    }

    pub fn set_keep_history(&self, enabled: bool) {
        self.data.lock().unwrap().keep_history = enabled;
        self.save();
    }

    /// Short description of the speech engine for history entries.
    pub fn engine_label(&self) -> String {
        let stt = self.stt();
        let model_name = std::path::Path::new(&stt.model)
            .file_stem()
            .map(|stem| {
                stem.to_string_lossy()
                    .trim_start_matches("ggml-")
                    .to_string()
            })
            .unwrap_or_default();
        match stt.engine {
            SttEngine::OpenaiCompat => {
                let provider = match stt.preset.as_deref() {
                    Some("groq") => "Groq",
                    Some("openai") => "OpenAI",
                    _ => "Custom",
                };
                format!("{provider} · {}", stt.model)
            }
            SttEngine::Deepgram => format!("Deepgram · {}", stt.model),
            SttEngine::Elevenlabs => format!("ElevenLabs · {}", stt.model),
            SttEngine::WhisperCpp => format!("whisper · {model_name}"),
            SttEngine::Apple => "Apple · on-device".to_string(),
        }
    }

    pub fn added_models(&self) -> Vec<String> {
        self.data.lock().unwrap().added_models.clone()
    }

    pub fn add_model(&self, path: String) {
        {
            let mut data = self.data.lock().unwrap();
            if data.added_models.contains(&path) {
                return;
            }
            data.added_models.push(path);
        }
        self.save();
    }

    pub fn remove_model(&self, path: &str) {
        self.data
            .lock()
            .unwrap()
            .added_models
            .retain(|item| item != path);
        self.save();
    }

    pub fn set_transform_model(&self, model: String) {
        self.data.lock().unwrap().transform_model = model;
        self.save();
    }

    pub fn hotkey(&self) -> String {
        self.data.lock().unwrap().hotkey.clone()
    }

    pub fn set_hotkey(&self, hotkey: String) {
        self.data.lock().unwrap().hotkey = hotkey;
        self.save();
    }

    pub fn delivery(&self, mode: super::Mode) -> DeliveryBehavior {
        let data = self.data.lock().unwrap();
        match mode {
            super::Mode::Normal => data.dictation_delivery,
            super::Mode::Summarize => data.summary_delivery,
            super::Mode::Prompt => data.prompt_delivery,
        }
    }

    pub fn set_delivery(&self, mode: &str, delivery: DeliveryBehavior) -> Result<(), String> {
        {
            let mut data = self.data.lock().unwrap();
            match mode {
                "dictation" => data.dictation_delivery = delivery,
                "summary" => data.summary_delivery = delivery,
                "prompt" => data.prompt_delivery = delivery,
                _ => return Err("Voice mode must be Dictation, Summary, or Prompt.".to_string()),
            }
        }
        self.save();
        Ok(())
    }

    pub fn interface_sounds(&self) -> bool {
        self.data.lock().unwrap().interface_sounds
    }

    pub fn set_interface_sounds(&self, enabled: bool) {
        self.data.lock().unwrap().interface_sounds = enabled;
        self.save();
    }

    fn keychain_service_for_role(&self, role: &str) -> Option<String> {
        if role == "transformation" {
            let endpoint = self.transformation_config();
            return Some(keychain_service(&endpoint.provider, &endpoint.base_url));
        }
        stt_keychain_service(&self.stt())
    }

    pub fn api_key(&self, role: &str) -> Result<String, String> {
        if role == "transformation" && !transform_needs_key(&self.transformation_config().provider)
        {
            return Ok(String::new());
        }
        let service = self
            .keychain_service_for_role(role)
            .ok_or_else(|| "This speech engine does not use an API key.".to_string())?;
        match read_keychain(&service) {
            Ok(key) => Ok(key),
            Err(error) if service.starts_with("com.samlu.desktop.voice.custom.") => {
                read_keychain("com.samlu.desktop.voice.custom").map_err(|_| error)
            }
            Err(error) => Err(error),
        }
    }

    pub fn has_api_key(&self, role: &str) -> bool {
        self.api_key(role).is_ok()
    }

    pub fn set_api_key(&self, role: &str, key: &str) -> Result<(), String> {
        let service = self
            .keychain_service_for_role(role)
            .ok_or_else(|| "This speech engine does not use an API key.".to_string())?;
        if key.trim().is_empty() {
            let _ = Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-a", "samlu", "-s", &service])
                .status();
            return Ok(());
        }
        let status = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-a",
                "samlu",
                "-s",
                &service,
                "-w",
                key.trim(),
                "-U",
            ])
            .status()
            .map_err(|error| format!("could not write macOS Keychain: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("macOS Keychain returned {status}"))
        }
    }

    fn save(&self) {
        let data = self.data.lock().unwrap();
        let Ok(json) = serde_json::to_string_pretty(&*data) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.path, json);
    }
}

fn read_keychain(service: &str) -> Result<String, String> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-a", "samlu", "-s", service, "-w"])
        .output()
        .map_err(|error| format!("could not read macOS Keychain: {error}"))?;
    if !output.status.success() {
        return Err("No API key saved for this endpoint.".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn keychain_service(provider: &str, base_url: &str) -> String {
    if provider == DEFAULT_PROVIDER {
        return "com.samlu.desktop.voice.groq".to_string();
    }
    let endpoint: String = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(72)
        .collect();
    format!("com.samlu.desktop.voice.custom.{endpoint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated(json: &str) -> Data {
        let mut data: Data = serde_json::from_str(json).unwrap();
        assert!(data.migrate());
        data
    }

    #[test]
    fn legacy_groq_config_migrates_to_openai_compat_preset() {
        let data = migrated(
            r#"{"provider":"groq","base_url":"https://api.groq.com/openai/v1","stt_model":"whisper-large-v3","transform_model":"llm","hotkey":"Alt+V"}"#,
        );
        let stt = data.stt.unwrap();
        assert_eq!(stt.engine, SttEngine::OpenaiCompat);
        assert_eq!(stt.preset.as_deref(), Some("groq"));
        assert_eq!(stt.model, "whisper-large-v3");
        assert_eq!(stt.base_url.as_deref(), Some(DEFAULT_BASE_URL));
        assert_eq!(stt.language, "auto");
        assert!(data.keep_history);
        assert!(!data.cleanup_dictation);
    }

    #[test]
    fn legacy_shared_custom_endpoint_is_copied_to_transformation() {
        let data = migrated(
            r#"{"provider":"custom","base_url":"https://speech.example/v1","stt_model":"stt","separate_transform_provider":false,"transform_provider":"groq","transform_base_url":"https://api.groq.com/openai/v1","transform_model":"llm"}"#,
        );
        assert_eq!(data.stt.as_ref().unwrap().preset.as_deref(), Some("custom"));
        assert_eq!(data.transform_provider, "custom");
        assert_eq!(data.transform_base_url, "https://speech.example/v1");
        assert_eq!(data.transform_model, "llm");
    }

    #[test]
    fn legacy_separate_transformation_endpoint_is_kept() {
        let data = migrated(
            r#"{"provider":"groq","base_url":"https://api.groq.com/openai/v1","separate_transform_provider":true,"transform_provider":"custom","transform_base_url":"https://llm.example/v1"}"#,
        );
        assert_eq!(data.transform_provider, "custom");
        assert_eq!(data.transform_base_url, "https://llm.example/v1");
    }

    #[test]
    fn migrated_config_does_not_write_legacy_fields() {
        let data = migrated(r#"{"provider":"groq","base_url":"https://api.groq.com/openai/v1"}"#);
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"stt\""));
        assert!(!json.contains("\"provider\""));
        assert!(!json.contains("\"stt_model\""));
        assert!(!json.contains("separate_transform_provider"));
    }

    #[test]
    fn current_config_is_not_migrated_again() {
        let mut data: Data = serde_json::from_str(
            r#"{"stt":{"engine":"whisper_cpp","model":"/m/ggml-base.bin","language":"de"}}"#,
        )
        .unwrap();
        assert!(!data.migrate());
        let stt = data.stt.unwrap();
        assert_eq!(stt.engine, SttEngine::WhisperCpp);
        assert_eq!(stt.language, "de");
    }

    #[test]
    fn keychain_services_are_unchanged_for_migrated_endpoints() {
        let groq = SttSettings::default();
        assert_eq!(
            stt_keychain_service(&groq).as_deref(),
            Some("com.samlu.desktop.voice.groq")
        );
        let custom = SttSettings {
            preset: Some("custom".into()),
            base_url: Some("https://API.Example.com/openai/v1".into()),
            ..SttSettings::default()
        };
        assert_eq!(
            stt_keychain_service(&custom).as_deref(),
            Some("com.samlu.desktop.voice.custom.api_example_com_openai_v1")
        );
        let local = SttSettings {
            engine: SttEngine::WhisperCpp,
            ..SttSettings::default()
        };
        assert_eq!(stt_keychain_service(&local), None);
        let deepgram = SttSettings {
            engine: SttEngine::Deepgram,
            ..SttSettings::default()
        };
        assert_eq!(
            stt_keychain_service(&deepgram).as_deref(),
            Some("com.samlu.desktop.voice.deepgram")
        );
    }

    #[test]
    fn local_transformation_presets_need_no_key() {
        assert!(!transform_needs_key("ollama"));
        assert!(!transform_needs_key("lmstudio"));
        assert!(transform_needs_key("groq"));
        assert!(transform_needs_key("custom"));
        assert_eq!(transform_preset_base_url("ollama"), Some(OLLAMA_BASE_URL));
        assert_eq!(transform_preset_base_url("custom"), None);
    }

    #[test]
    fn transform_default_model_covers_all_providers() {
        assert_eq!(
            transform_default_model("groq"),
            Some(DEFAULT_TRANSFORM_MODEL)
        );
        assert_eq!(transform_default_model("openai"), Some("gpt-4.1-mini"));
        assert_eq!(transform_default_model("ollama"), Some("llama3.2"));
        assert_eq!(transform_default_model("lmstudio"), None);
        assert_eq!(transform_default_model("custom"), None);
        assert_eq!(transform_default_model("unknown"), None);
        assert_eq!(transform_default_model(""), None);
    }

    #[test]
    fn custom_keychain_service_is_stable_per_endpoint() {
        assert_eq!(
            keychain_service("custom", "https://API.Example.com/openai/v1"),
            "com.samlu.desktop.voice.custom.api_example_com_openai_v1"
        );
    }

    #[test]
    fn engine_labels_name_the_engine_and_model() {
        let dir = std::env::temp_dir().join(format!("samlu-label-{}", std::process::id()));
        let config = VoiceConfig::load(&dir);
        config.set_stt(SttSettings {
            engine: SttEngine::WhisperCpp,
            model: "/m/ggml-large-v3-turbo-q5_0.bin".into(),
            ..SttSettings::default()
        });
        assert_eq!(config.engine_label(), "whisper · large-v3-turbo-q5_0");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_whisper_is_ready_only_with_a_valid_model_file() {
        let dir = std::env::temp_dir().join(format!("samlu-ready-{}", std::process::id()));
        let config = VoiceConfig::load(&dir);
        config.set_stt(SttSettings {
            engine: SttEngine::WhisperCpp,
            model: dir.join("missing.bin").to_string_lossy().into_owned(),
            ..SttSettings::default()
        });
        assert!(!config.stt_ready());
        let _ = std::fs::remove_dir_all(dir);
    }
}

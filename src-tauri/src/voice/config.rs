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

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    provider: String,
    base_url: String,
    separate_transform_provider: bool,
    transform_provider: String,
    transform_base_url: String,
    stt_model: String,
    transform_model: String,
    hotkey: String,
    dictation_delivery: DeliveryBehavior,
    summary_delivery: DeliveryBehavior,
    prompt_delivery: DeliveryBehavior,
    interface_sounds: bool,
    /// Show Samlu itself in the recording capsule instead of the waveform.
    pet_capsule: bool,
    #[serde(skip_serializing)]
    preview_enabled: bool,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            provider: DEFAULT_PROVIDER.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            separate_transform_provider: false,
            transform_provider: DEFAULT_PROVIDER.to_string(),
            transform_base_url: DEFAULT_BASE_URL.to_string(),
            stt_model: DEFAULT_STT_MODEL.to_string(),
            transform_model: DEFAULT_TRANSFORM_MODEL.to_string(),
            hotkey: DEFAULT_HOTKEY.to_string(),
            dictation_delivery: DeliveryBehavior::InstantInsert,
            summary_delivery: DeliveryBehavior::InstantInsert,
            prompt_delivery: DeliveryBehavior::EditablePreview,
            interface_sounds: true,
            pet_capsule: true,
            preview_enabled: false,
        }
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
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    pub fn transcription_config(&self) -> EndpointConfig {
        let data = self.data.lock().unwrap();
        EndpointConfig {
            provider: data.provider.clone(),
            base_url: data.base_url.clone(),
            model: data.stt_model.clone(),
        }
    }

    pub fn transformation_config(&self) -> EndpointConfig {
        let data = self.data.lock().unwrap();
        let (provider, base_url) = if data.separate_transform_provider {
            (
                data.transform_provider.clone(),
                data.transform_base_url.clone(),
            )
        } else {
            (data.provider.clone(), data.base_url.clone())
        };
        EndpointConfig {
            provider,
            base_url,
            model: data.transform_model.clone(),
        }
    }

    pub fn separate_providers(&self) -> bool {
        self.data.lock().unwrap().separate_transform_provider
    }

    pub fn set_separate_providers(&self, enabled: bool) {
        self.data.lock().unwrap().separate_transform_provider = enabled;
        self.save();
    }

    pub fn set_endpoint(&self, role: &str, provider: String, base_url: String) {
        {
            let mut data = self.data.lock().unwrap();
            if role == "transformation" {
                data.transform_provider = provider;
                data.transform_base_url = base_url.trim_end_matches('/').to_string();
            } else {
                data.provider = provider;
                data.base_url = base_url.trim_end_matches('/').to_string();
            }
        }
        self.save();
    }

    pub fn set_stt_model(&self, model: String) {
        self.data.lock().unwrap().stt_model = model;
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

    pub fn pet_capsule(&self) -> bool {
        self.data.lock().unwrap().pet_capsule
    }

    pub fn set_pet_capsule(&self, enabled: bool) {
        self.data.lock().unwrap().pet_capsule = enabled;
        self.save();
    }

    pub fn set_interface_sounds(&self, enabled: bool) {
        self.data.lock().unwrap().interface_sounds = enabled;
        self.save();
    }

    pub fn api_key(&self, role: &str) -> Result<String, String> {
        let endpoint = self.endpoint_for_role(role);
        let service = keychain_service(&endpoint.provider, &endpoint.base_url);
        match read_keychain(&service) {
            Ok(key) => Ok(key),
            Err(error) if endpoint.provider == "custom" => {
                read_keychain("com.samlu.desktop.voice.custom").map_err(|_| error)
            }
            Err(error) => Err(error),
        }
    }

    pub fn has_api_key(&self, role: &str) -> bool {
        self.api_key(role).is_ok()
    }

    pub fn set_api_key(&self, role: &str, key: &str) -> Result<(), String> {
        let endpoint = self.endpoint_for_role(role);
        let service = keychain_service(&endpoint.provider, &endpoint.base_url);
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

    fn endpoint_for_role(&self, role: &str) -> EndpointConfig {
        if role == "transformation" {
            self.transformation_config()
        } else {
            self.transcription_config()
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

    #[test]
    fn old_config_uses_shared_provider_defaults() {
        let data: Data = serde_json::from_str(
            r#"{"provider":"custom","base_url":"https://speech.example/v1","stt_model":"stt","transform_model":"llm","hotkey":"Alt+V","preview_enabled":true}"#,
        )
        .unwrap();

        assert!(!data.separate_transform_provider);
        assert_eq!(data.transform_provider, DEFAULT_PROVIDER);
    }

    #[test]
    fn custom_keychain_service_is_stable_per_endpoint() {
        assert_eq!(
            keychain_service("custom", "https://API.Example.com/openai/v1"),
            "com.samlu.desktop.voice.custom.api_example_com_openai_v1"
        );
    }
}

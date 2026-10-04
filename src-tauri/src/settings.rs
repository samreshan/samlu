//! Persisted application preferences shared by agent notifications and
//! launcher utilities. Secrets are deliberately excluded from this file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    auth_token: String,
    appearance: String,
    debounce_secs: u64,
    notify_needs_input: bool,
    notify_task_completed: bool,
    notify_turn_finished: bool,
    include_agent_summary: bool,
    show_agent_island: bool,
    /// `pet` | `island` | `system`. Absent in configs written before delivery
    /// became a three-way choice, and derived from `show_agent_island` then.
    delivery: Option<String>,
    clipboard_history_enabled: bool,
    onboarding_completed: bool,
    /// `background` keeps shortcuts and agent notifications active after
    /// Settings closes; `quit` exits the process instead.
    close_behavior: String,
}

pub const DELIVERY_PET: &str = "pet";
pub const DELIVERY_ISLAND: &str = "island";
pub const DELIVERY_SYSTEM: &str = "system";

fn normalize_delivery(value: &str) -> Option<&'static str> {
    match value {
        DELIVERY_PET => Some(DELIVERY_PET),
        DELIVERY_ISLAND => Some(DELIVERY_ISLAND),
        DELIVERY_SYSTEM => Some(DELIVERY_SYSTEM),
        _ => None,
    }
}

impl Default for Data {
    fn default() -> Self {
        Self {
            auth_token: String::new(),
            appearance: "system".to_string(),
            debounce_secs: 20,
            notify_needs_input: true,
            notify_task_completed: true,
            notify_turn_finished: false,
            include_agent_summary: true,
            show_agent_island: true,
            delivery: None,
            clipboard_history_enabled: false,
            onboarding_completed: false,
            close_behavior: "background".to_string(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPreferences {
    pub debounce_secs: u64,
    pub notify_needs_input: bool,
    pub notify_task_completed: bool,
    pub notify_turn_finished: bool,
    pub include_agent_summary: bool,
    pub delivery: String,
    pub clipboard_history_enabled: bool,
    pub close_behavior: String,
}

pub struct AppConfig {
    path: PathBuf,
    data: Mutex<Data>,
}

impl AppConfig {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("samlu-config.json");
        let mut data: Data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        if data.auth_token.is_empty() {
            data.auth_token = crate::server::auth::generate_token();
        }
        // Existing installs carried a boolean; keep whichever style they had.
        if data
            .delivery
            .as_deref()
            .and_then(normalize_delivery)
            .is_none()
        {
            data.delivery = Some(
                if data.show_agent_island {
                    DELIVERY_ISLAND
                } else {
                    DELIVERY_SYSTEM
                }
                .to_string(),
            );
        }
        let config = Self {
            path,
            data: Mutex::new(data),
        };
        config.save();
        config
    }

    pub fn auth_token(&self) -> String {
        self.data.lock().unwrap().auth_token.clone()
    }

    pub fn appearance(&self) -> String {
        self.data.lock().unwrap().appearance.clone()
    }

    pub fn set_appearance(&self, appearance: String) -> Result<(), String> {
        if !matches!(appearance.as_str(), "system" | "light" | "dark") {
            return Err("Appearance must be System, Light, or Dark.".to_string());
        }
        self.data.lock().unwrap().appearance = appearance;
        self.save();
        Ok(())
    }

    pub fn onboarding_completed(&self) -> bool {
        self.data.lock().unwrap().onboarding_completed
    }

    pub fn close_behavior(&self) -> String {
        let value = self.data.lock().unwrap().close_behavior.clone();
        match value.as_str() {
            "quit" => value,
            _ => "background".to_string(),
        }
    }

    pub fn set_onboarding_completed(&self, completed: bool) {
        self.data.lock().unwrap().onboarding_completed = completed;
        self.save();
    }

    pub fn delivery(&self) -> String {
        self.data
            .lock()
            .unwrap()
            .delivery
            .as_deref()
            .and_then(normalize_delivery)
            .unwrap_or(DELIVERY_ISLAND)
            .to_string()
    }

    /// Used by onboarding's delivery step, which sets this one preference
    /// without touching the rest of the notification settings.
    pub fn set_delivery(&self, delivery: &str) -> Result<(), String> {
        let Some(value) = normalize_delivery(delivery) else {
            return Err("Delivery must be Samlu, Samlu Island, or System.".to_string());
        };
        {
            let mut data = self.data.lock().unwrap();
            data.delivery = Some(value.to_string());
            data.show_agent_island = value == DELIVERY_ISLAND;
        }
        self.save();
        Ok(())
    }

    pub fn preferences(&self) -> NotificationPreferences {
        let data = self.data.lock().unwrap();
        NotificationPreferences {
            debounce_secs: data.debounce_secs,
            notify_needs_input: data.notify_needs_input,
            notify_task_completed: data.notify_task_completed,
            notify_turn_finished: data.notify_turn_finished,
            include_agent_summary: data.include_agent_summary,
            delivery: data
                .delivery
                .as_deref()
                .and_then(normalize_delivery)
                .unwrap_or(DELIVERY_ISLAND)
                .to_string(),
            clipboard_history_enabled: data.clipboard_history_enabled,
            close_behavior: match data.close_behavior.as_str() {
                "quit" => "quit".to_string(),
                _ => "background".to_string(),
            },
        }
    }

    pub fn set_preferences(&self, preferences: NotificationPreferences) {
        {
            let mut data = self.data.lock().unwrap();
            data.debounce_secs = preferences.debounce_secs.clamp(3, 300);
            data.notify_needs_input = preferences.notify_needs_input;
            data.notify_task_completed = preferences.notify_task_completed;
            data.notify_turn_finished = preferences.notify_turn_finished;
            data.include_agent_summary = preferences.include_agent_summary;
            if let Some(delivery) = normalize_delivery(&preferences.delivery) {
                data.delivery = Some(delivery.to_string());
                data.show_agent_island = delivery == DELIVERY_ISLAND;
            }
            data.clipboard_history_enabled = preferences.clipboard_history_enabled;
            data.close_behavior = match preferences.close_behavior.as_str() {
                "quit" => "quit".to_string(),
                _ => "background".to_string(),
            };
        }
        self.save();
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

#[tauri::command]
pub fn get_notification_preferences(
    config: tauri::State<'_, std::sync::Arc<AppConfig>>,
) -> NotificationPreferences {
    config.preferences()
}

#[tauri::command]
pub fn set_notification_preferences(
    preferences: NotificationPreferences,
    config: tauri::State<'_, std::sync::Arc<AppConfig>>,
) {
    config.set_preferences(preferences);
}

#[tauri::command]
pub fn get_appearance(config: tauri::State<'_, std::sync::Arc<AppConfig>>) -> String {
    config.appearance()
}

#[tauri::command]
pub fn set_appearance(
    app: tauri::AppHandle,
    appearance: String,
    config: tauri::State<'_, std::sync::Arc<AppConfig>>,
) -> Result<(), String> {
    config.set_appearance(appearance.clone())?;
    use tauri::Emitter;
    let _ = app.emit("appearance://changed", appearance);
    Ok(())
}

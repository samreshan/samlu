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
    /// `island` | `system`. Absent in configs written before delivery became a
    /// choice, and derived from `show_agent_island` then. Configs from the pet
    /// era may still hold `pet`; it now reads as `island`.
    delivery: Option<String>,
    /// How the Samlu Island presents an update: `bloom` | `buddy`.
    island_style: String,
    clipboard_history_enabled: bool,
    onboarding_completed: bool,
    /// `background` keeps shortcuts and agent notifications active after
    /// Settings closes; `quit` exits the process instead.
    close_behavior: String,
    /// Look for a newer release every few hours.
    update_auto_check: bool,
    /// Install what a check finds without asking. It takes effect on the next
    /// start of Samlu.
    update_auto_install: bool,
    /// RFC 3339 time of the last finished check, successful or not.
    update_last_checked_at: Option<String>,
    agent_alerts: AgentAlertSettings,
}

/// Sounds, shortcuts and holds for agent alerts. Defaults differ by platform
/// because the shortcut chords and system sound names do.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentAlertSettings {
    pub sounds_enabled: bool,
    pub volume: u8,
    pub sound_needs_input: String,
    pub sound_completed: String,
    pub sound_turn_finished: String,
    pub codex_wait_secs: u64,
    pub approve_shortcut: String,
    pub deny_shortcut: String,
    pub reminder_mins: u64,
}

impl Default for AgentAlertSettings {
    fn default() -> Self {
        let (needs_input, completed, approve, deny) = if cfg!(windows) {
            (
                "system:Windows Exclamation",
                "system:Windows Notify System Generic",
                "Ctrl+Alt+Y",
                "Ctrl+Alt+N",
            )
        } else {
            ("system:Glass", "system:Hero", "Alt+Cmd+Y", "Alt+Cmd+N")
        };
        Self {
            sounds_enabled: true,
            volume: 70,
            sound_needs_input: needs_input.to_string(),
            sound_completed: completed.to_string(),
            sound_turn_finished: "none".to_string(),
            codex_wait_secs: 30,
            approve_shortcut: approve.to_string(),
            deny_shortcut: deny.to_string(),
            reminder_mins: 3,
        }
    }
}

/// The only reminder delays the Settings page offers, in minutes.
const REMINDER_CHOICES_MINS: [u64; 5] = [0, 1, 3, 5, 10];

/// Brings a settings write into the ranges the alert code relies on. A blank
/// shortcut or an unknown sound keeps the stored value, so a half-filled form
/// cannot erase a working setting.
fn normalize_agent_alerts(
    mut next: AgentAlertSettings,
    previous: &AgentAlertSettings,
) -> AgentAlertSettings {
    next.volume = next.volume.min(100);
    next.codex_wait_secs = next.codex_wait_secs.clamp(10, 120);
    next.reminder_mins = nearest_reminder_choice(next.reminder_mins);
    next.approve_shortcut = keep_unless_blank(&next.approve_shortcut, &previous.approve_shortcut);
    next.deny_shortcut = keep_unless_blank(&next.deny_shortcut, &previous.deny_shortcut);
    next.sound_needs_input =
        keep_unless_valid_sound(&next.sound_needs_input, &previous.sound_needs_input);
    next.sound_completed =
        keep_unless_valid_sound(&next.sound_completed, &previous.sound_completed);
    next.sound_turn_finished =
        keep_unless_valid_sound(&next.sound_turn_finished, &previous.sound_turn_finished);
    next
}

/// Ties go to the lower choice, so 2 becomes 1 and 4 becomes 3.
fn nearest_reminder_choice(minutes: u64) -> u64 {
    let mut best = REMINDER_CHOICES_MINS[0];
    for &choice in &REMINDER_CHOICES_MINS[1..] {
        if choice.abs_diff(minutes) < best.abs_diff(minutes) {
            best = choice;
        }
    }
    best
}

fn keep_unless_blank(value: &str, previous: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        previous.to_string()
    } else {
        trimmed.to_string()
    }
}

fn keep_unless_valid_sound(value: &str, previous: &str) -> String {
    if value == "none" || value.starts_with("system:") || value.starts_with("custom:") {
        value.to_string()
    } else {
        previous.to_string()
    }
}

/// Retired with the pet overlay. Kept only so older configs migrate cleanly.
const LEGACY_DELIVERY_PET: &str = "pet";
pub const DELIVERY_ISLAND: &str = "island";
pub const DELIVERY_SYSTEM: &str = "system";

pub const ISLAND_STYLE_BLOOM: &str = "bloom";
pub const ISLAND_STYLE_BUDDY: &str = "buddy";

fn normalize_island_style(value: &str) -> Option<&'static str> {
    match value {
        ISLAND_STYLE_BLOOM => Some(ISLAND_STYLE_BLOOM),
        ISLAND_STYLE_BUDDY => Some(ISLAND_STYLE_BUDDY),
        _ => None,
    }
}

fn default_island_style() -> String {
    ISLAND_STYLE_BLOOM.to_string()
}

fn normalize_delivery(value: &str) -> Option<&'static str> {
    match value {
        LEGACY_DELIVERY_PET => Some(DELIVERY_ISLAND),
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
            island_style: default_island_style(),
            clipboard_history_enabled: false,
            onboarding_completed: false,
            close_behavior: "background".to_string(),
            update_auto_check: true,
            update_auto_install: true,
            update_last_checked_at: None,
            agent_alerts: AgentAlertSettings::default(),
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
    /// Defaulted so a Settings page from before the choice existed can still
    /// save the rest of its preferences.
    #[serde(default = "default_island_style")]
    pub island_style: String,
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

    pub fn island_style(&self) -> &'static str {
        normalize_island_style(&self.data.lock().unwrap().island_style)
            .unwrap_or(ISLAND_STYLE_BLOOM)
    }

    pub fn update_auto_check(&self) -> bool {
        self.data.lock().unwrap().update_auto_check
    }

    /// The stored choice. It only takes effect while automatic checks are on,
    /// and is kept as chosen while they are off so turning them back on
    /// restores it.
    pub fn update_auto_install(&self) -> bool {
        self.data.lock().unwrap().update_auto_install
    }

    pub fn set_update_preferences(&self, auto_check: bool, auto_install: bool) {
        {
            let mut data = self.data.lock().unwrap();
            data.update_auto_check = auto_check;
            data.update_auto_install = auto_install;
        }
        self.save();
    }

    pub fn update_last_checked_at(&self) -> Option<String> {
        self.data.lock().unwrap().update_last_checked_at.clone()
    }

    pub fn set_update_last_checked_at(&self, at: String) {
        self.data.lock().unwrap().update_last_checked_at = Some(at);
        self.save();
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
            island_style: normalize_island_style(&data.island_style)
                .unwrap_or(ISLAND_STYLE_BLOOM)
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
            if let Some(style) = normalize_island_style(&preferences.island_style) {
                data.island_style = style.to_string();
            }
            data.clipboard_history_enabled = preferences.clipboard_history_enabled;
            data.close_behavior = match preferences.close_behavior.as_str() {
                "quit" => "quit".to_string(),
                _ => "background".to_string(),
            };
        }
        self.save();
    }

    pub fn agent_alerts(&self) -> AgentAlertSettings {
        self.data.lock().unwrap().agent_alerts.clone()
    }

    /// Normalizes against the stored values, saves, and returns what was
    /// stored so the caller can echo the true state back to the page.
    pub fn set_agent_alerts(&self, settings: AgentAlertSettings) -> AgentAlertSettings {
        let stored = {
            let mut data = self.data.lock().unwrap();
            data.agent_alerts = normalize_agent_alerts(settings, &data.agent_alerts);
            data.agent_alerts.clone()
        };
        self.save();
        stored
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
pub fn get_agent_alert_settings(
    config: tauri::State<'_, std::sync::Arc<AppConfig>>,
) -> AgentAlertSettings {
    config.agent_alerts()
}

#[tauri::command]
pub fn set_agent_alert_settings(
    app: tauri::AppHandle,
    settings: AgentAlertSettings,
    config: tauri::State<'_, std::sync::Arc<AppConfig>>,
) -> AgentAlertSettings {
    let stored = config.set_agent_alerts(settings);
    use tauri::Emitter;
    let _ = app.emit("agents://alerts-changed", stored.clone());
    stored
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
    apply_window_theme(&app, &appearance);
    use tauri::Emitter;
    let _ = app.emit("appearance://changed", appearance);
    Ok(())
}

/// Gives every native window the chosen appearance, so window controls,
/// scrollbars and each page's `prefers-color-scheme` agree with Samlu's
/// setting instead of only with the system's.
pub fn apply_window_theme(app: &tauri::AppHandle, appearance: &str) {
    use tauri::Manager;
    let theme = match appearance {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    };
    for window in app.webview_windows().values() {
        let _ = window.set_theme(theme);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_agent_alerts, normalize_island_style, AgentAlertSettings, Data,
        NotificationPreferences, ISLAND_STYLE_BLOOM,
    };

    #[test]
    fn agent_alert_defaults() {
        let defaults = AgentAlertSettings::default();
        assert!(defaults.sounds_enabled);
        assert_eq!(defaults.volume, 70);
        assert_eq!(defaults.codex_wait_secs, 30);
        assert_eq!(defaults.reminder_mins, 3);
        assert_eq!(defaults.sound_turn_finished, "none");
        if cfg!(windows) {
            assert_eq!(defaults.sound_needs_input, "system:Windows Exclamation");
            assert_eq!(
                defaults.sound_completed,
                "system:Windows Notify System Generic"
            );
            assert_eq!(defaults.approve_shortcut, "Ctrl+Alt+Y");
            assert_eq!(defaults.deny_shortcut, "Ctrl+Alt+N");
        } else {
            assert_eq!(defaults.sound_needs_input, "system:Glass");
            assert_eq!(defaults.sound_completed, "system:Hero");
            assert_eq!(defaults.approve_shortcut, "Alt+Cmd+Y");
            assert_eq!(defaults.deny_shortcut, "Alt+Cmd+N");
        }
    }

    #[test]
    fn clamps_and_normalizes() {
        let previous = AgentAlertSettings {
            approve_shortcut: "Cmd+Y".to_string(),
            deny_shortcut: "Cmd+N".to_string(),
            sound_needs_input: "custom:Ping".to_string(),
            sound_completed: "custom:Pong".to_string(),
            ..AgentAlertSettings::default()
        };
        let next = AgentAlertSettings {
            volume: 150,
            codex_wait_secs: 5,
            reminder_mins: 4,
            approve_shortcut: "  ".to_string(),
            deny_shortcut: " Ctrl+N ".to_string(),
            sound_needs_input: "bogus".to_string(),
            sound_completed: "custom:Chime".to_string(),
            ..previous.clone()
        };
        let got = normalize_agent_alerts(next, &previous);
        assert_eq!(got.volume, 100);
        assert_eq!(got.codex_wait_secs, 10);
        assert_eq!(got.reminder_mins, 3);
        assert_eq!(
            got.approve_shortcut, "Cmd+Y",
            "blank shortcut keeps previous"
        );
        assert_eq!(got.deny_shortcut, "Ctrl+N", "shortcut is trimmed");
        assert_eq!(
            got.sound_needs_input, "custom:Ping",
            "unknown sound keeps previous"
        );
        assert_eq!(got.sound_completed, "custom:Chime");

        let high = AgentAlertSettings {
            codex_wait_secs: 500,
            ..previous.clone()
        };
        assert_eq!(normalize_agent_alerts(high, &previous).codex_wait_secs, 120);
    }

    #[test]
    fn reminder_snaps_to_nearest_choice_with_ties_lower() {
        let previous = AgentAlertSettings::default();
        let snap = |minutes: u64| {
            normalize_agent_alerts(
                AgentAlertSettings {
                    reminder_mins: minutes,
                    ..previous.clone()
                },
                &previous,
            )
            .reminder_mins
        };
        assert_eq!(snap(0), 0);
        assert_eq!(snap(2), 1, "tie between 1 and 3 goes lower");
        assert_eq!(snap(4), 3, "tie between 3 and 5 goes lower");
        assert_eq!(snap(7), 5);
        assert_eq!(snap(8), 10);
        assert_eq!(snap(60), 10);
    }

    #[test]
    fn old_config_without_alerts_loads_defaults() {
        let data: Data = serde_json::from_str(r#"{"authToken":"abc"}"#).unwrap();
        assert!(data.agent_alerts == AgentAlertSettings::default());
    }

    #[test]
    fn island_style_accepts_only_known_styles() {
        assert_eq!(normalize_island_style("bloom"), Some("bloom"));
        assert_eq!(normalize_island_style("buddy"), Some("buddy"));
        assert_eq!(normalize_island_style("ticket"), None);
    }

    #[test]
    fn preferences_without_island_style_still_parse() {
        let json = r#"{"debounceSecs":20,"notifyNeedsInput":true,"notifyTaskCompleted":true,
            "notifyTurnFinished":false,"includeAgentSummary":true,"delivery":"island",
            "clipboardHistoryEnabled":false,"closeBehavior":"background"}"#;
        let prefs: NotificationPreferences = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.island_style, ISLAND_STYLE_BLOOM);
    }
}

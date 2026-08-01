//! Persisted launcher preferences. Project indexing is intentionally manual
//! for the first beta, which avoids a permanent file-system watcher.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Not Cmd+Space — that's already bound to macOS Spotlight system-wide and
/// can't be silently taken over from another app.
pub const DEFAULT_HOTKEY: &str = "Alt+H";

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Data {
    hotkey: String,
    project_roots: Vec<String>,
    excluded_project_paths: Vec<String>,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            project_roots: default_project_roots(),
            excluded_project_paths: Vec::new(),
        }
    }
}

pub struct LauncherConfig {
    path: PathBuf,
    data: Mutex<Data>,
}

impl LauncherConfig {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("launcher-config.json");
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    pub fn hotkey(&self) -> String {
        self.data.lock().unwrap().hotkey.clone()
    }

    pub fn set_hotkey(&self, hotkey: String) {
        {
            let mut data = self.data.lock().unwrap();
            data.hotkey = hotkey;
        }
        self.save();
    }

    pub fn project_roots(&self) -> Vec<String> {
        self.data.lock().unwrap().project_roots.clone()
    }

    pub fn excluded_project_paths(&self) -> Vec<String> {
        self.data.lock().unwrap().excluded_project_paths.clone()
    }

    pub fn set_project_settings(&self, roots: Vec<String>, excluded_paths: Vec<String>) {
        {
            let mut data = self.data.lock().unwrap();
            data.project_roots = roots;
            data.excluded_project_paths = excluded_paths;
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

fn default_project_roots() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    [
        "Programming",
        "Developer",
        "Projects",
        "Code",
        "Documents/GitHub",
    ]
    .iter()
    .map(|directory| home.join(directory))
    .filter(|directory| directory.is_dir())
    .map(|directory| directory.to_string_lossy().to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_hotkey_only_config_receives_project_defaults() {
        let data: Data = serde_json::from_str(r#"{"hotkey":"Cmd+Shift+P"}"#).unwrap();
        assert_eq!(data.hotkey, "Cmd+Shift+P");
        assert!(data.excluded_project_paths.is_empty());
    }
}

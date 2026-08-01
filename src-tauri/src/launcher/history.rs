//! Small persisted frecency store for the launcher's ranking — how often
//! (and how recently) each result id has been picked, so frequently-used
//! apps/files/actions/projects rise to the top over time. This is Samlu's
//! first real on-disk settings-style store (separate from the Claude Code
//! `settings.json` merge in `setup/claude_hooks.rs`, which touches a file
//! outside the app's own data dir).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Entry {
    uses: u32,
    last_used_secs: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    entries: HashMap<String, Entry>,
}

pub struct LauncherHistory {
    path: PathBuf,
    store: Mutex<Store>,
}

impl LauncherHistory {
    /// Loads the store from `<app_data_dir>/launcher-history.json`, or
    /// starts empty if it doesn't exist yet / fails to parse.
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("launcher-history.json");
        let store = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            store: Mutex::new(store),
        }
    }

    pub fn record_selection(&self, id: &str) {
        {
            let mut store = self.store.lock().unwrap();
            let entry = store.entries.entry(id.to_string()).or_default();
            entry.uses += 1;
            entry.last_used_secs = chrono::Utc::now().timestamp();
        }
        self.save();
    }

    /// Usage count decayed by how long it's been since last picked — recent,
    /// frequently-picked results float to the top without a stale favorite
    /// dominating forever. Zero for anything never picked.
    pub fn score(&self, id: &str) -> f64 {
        let store = self.store.lock().unwrap();
        let Some(entry) = store.entries.get(id) else {
            return 0.0;
        };
        let age_hours =
            (chrono::Utc::now().timestamp() - entry.last_used_secs).max(0) as f64 / 3600.0;
        entry.uses as f64 / (1.0 + age_hours / 24.0)
    }

    fn save(&self) {
        let store = self.store.lock().unwrap();
        let Ok(json) = serde_json::to_string_pretty(&*store) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.path, json);
    }
}

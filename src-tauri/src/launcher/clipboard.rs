use crate::settings::AppConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

const CAPACITY: usize = 40;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: String,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

pub struct ClipboardHistory {
    path: PathBuf,
    items: Mutex<VecDeque<ClipboardItem>>,
}

impl ClipboardHistory {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("clipboard-history.json");
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<ClipboardItem> {
        let query = query.to_lowercase();
        self.items
            .lock()
            .unwrap()
            .iter()
            .filter(|item| query.is_empty() || item.text.to_lowercase().contains(&query))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn record(&self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || text.chars().count() > 10_000 {
            return;
        }
        let mut items = self.items.lock().unwrap();
        if items.front().is_some_and(|item| item.text == text) {
            return;
        }
        items.retain(|item| item.text != text);
        items.push_front(ClipboardItem {
            id: format!(
                "{:x}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
            text,
            timestamp: Utc::now(),
        });
        while items.len() > CAPACITY {
            items.pop_back();
        }
        drop(items);
        self.save();
    }

    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
        self.save();
    }

    fn save(&self) {
        let items = self.items.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&*items) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

pub fn start_monitor(app: AppHandle, history: Arc<ClipboardHistory>) {
    std::thread::spawn(move || {
        let mut previous = String::new();
        loop {
            std::thread::sleep(Duration::from_millis(1200));
            if !app
                .state::<Arc<AppConfig>>()
                .preferences()
                .clipboard_history_enabled
            {
                continue;
            }
            if let Ok(text) = app.clipboard().read_text() {
                if text != previous {
                    previous = text.clone();
                    history.record(text);
                }
            }
        }
    });
}

#[tauri::command]
pub fn clear_clipboard_history(history: tauri::State<'_, Arc<ClipboardHistory>>) {
    history.clear();
}

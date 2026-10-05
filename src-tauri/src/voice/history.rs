//! Local history of delivered dictations. Text only — audio is never kept.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const CAPACITY: usize = 200;
const FILE_NAME: &str = "voice-history.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Inserted,
    Copied,
    /// Waiting in the editable preview; accept or cancel updates it.
    Previewing,
    PreviewCancelled,
    CleanupSkipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    /// `dictation`, `summary`, or `prompt`.
    pub mode: String,
    pub text: String,
    /// The transcript, when cleanup or a transform changed it.
    pub raw: Option<String>,
    pub engine: String,
    pub target_app: String,
    pub outcome: Outcome,
}

impl HistoryEntry {
    pub fn new(
        mode: &str,
        text: String,
        raw: Option<String>,
        engine: String,
        target_app: String,
        outcome: Outcome,
    ) -> Self {
        let timestamp = Utc::now();
        Self {
            // The random suffix keeps ids unique when the clock is coarse.
            id: format!(
                "{:x}-{:08x}",
                timestamp.timestamp_nanos_opt().unwrap_or_default(),
                rand::random::<u32>()
            ),
            timestamp,
            mode: mode.to_string(),
            text,
            raw,
            engine,
            target_app,
            outcome,
        }
    }
}

pub struct VoiceHistory {
    path: PathBuf,
    items: Mutex<VecDeque<HistoryEntry>>,
}

impl VoiceHistory {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join(FILE_NAME);
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    pub fn record(&self, entry: HistoryEntry) {
        let mut items = self.items.lock().unwrap();
        items.push_front(entry);
        items.truncate(CAPACITY);
        self.save(&items);
    }

    pub fn set_outcome(&self, id: &str, outcome: Outcome) {
        let mut items = self.items.lock().unwrap();
        if let Some(item) = items.iter_mut().find(|item| item.id == id) {
            item.outcome = outcome;
            self.save(&items);
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<HistoryEntry> {
        let query = query.trim().to_lowercase();
        self.items
            .lock()
            .unwrap()
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.text.to_lowercase().contains(&query)
                    || item
                        .raw
                        .as_ref()
                        .is_some_and(|raw| raw.to_lowercase().contains(&query))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.path);
    }

    fn save(&self, items: &VecDeque<HistoryEntry>) {
        let Ok(json) = serde_json::to_string(items) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(text: &str) -> HistoryEntry {
        HistoryEntry::new(
            "dictation",
            text.to_string(),
            None,
            "Groq".into(),
            "com.apple.TextEdit".into(),
            Outcome::Inserted,
        )
    }

    #[test]
    fn keeps_the_newest_entries_up_to_capacity() {
        let history = VoiceHistory::load(&temp_dir("capacity"));
        for index in 0..CAPACITY + 5 {
            history.record(entry(&format!("entry {index}")));
        }
        let all = history.search("", CAPACITY + 10);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].text, format!("entry {}", CAPACITY + 4));
    }

    #[test]
    fn persists_and_searches_case_insensitively() {
        let dir = temp_dir("persist");
        VoiceHistory::load(&dir).record(entry("Ship the Samlu release"));
        let reloaded = VoiceHistory::load(&dir);
        assert_eq!(reloaded.search("samlu", 10).len(), 1);
        assert!(reloaded.search("nothing", 10).is_empty());
    }

    #[test]
    fn outcome_can_be_updated() {
        let history = VoiceHistory::load(&temp_dir("outcome"));
        let item = entry("draft");
        let id = item.id.clone();
        history.record(item);
        history.set_outcome(&id, Outcome::PreviewCancelled);
        assert_eq!(history.search("", 1)[0].outcome, Outcome::PreviewCancelled);
    }

    #[test]
    fn clear_empties_memory_and_deletes_the_file() {
        let dir = temp_dir("clear");
        let history = VoiceHistory::load(&dir);
        history.record(entry("secret"));
        assert!(dir.join(FILE_NAME).exists());
        history.clear();
        assert!(history.search("", 10).is_empty());
        assert!(!dir.join(FILE_NAME).exists());
    }
}

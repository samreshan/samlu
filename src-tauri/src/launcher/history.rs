//! Small persisted frecency store for the launcher's ranking — how often
//! (and how recently) each result id has been picked, so frequently-used
//! apps/files/actions/projects rise to the top over time. Entries also keep
//! what was picked, so an empty query can offer recent items without
//! searching for them again.

use super::search::LauncherResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Enough to remember months of use without the file growing forever; the
/// least useful entries are dropped first.
const MAX_ENTRIES: usize = 400;

/// Kinds that are worth offering again from an empty query. Clipboard text,
/// snippets and calculations are one-off copies, not places to go back to.
const SUGGESTIBLE_KINDS: &[&str] = &["app", "file", "project", "action"];

#[derive(Serialize, Deserialize, Default, Clone)]
struct Entry {
    uses: u32,
    last_used_secs: i64,
    /// Absent in entries written before suggestions existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<StoredItem>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredItem {
    kind: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    target: String,
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

    pub fn record_selection(&self, result: &LauncherResult) {
        {
            let mut store = self.store.lock().unwrap();
            let now = chrono::Utc::now().timestamp();
            let entry = store.entries.entry(result.id.clone()).or_default();
            entry.uses = entry.uses.saturating_add(1);
            entry.last_used_secs = now;
            entry.item = SUGGESTIBLE_KINDS
                .contains(&result.kind.as_str())
                .then(|| StoredItem {
                    kind: result.kind.clone(),
                    title: result.title.clone(),
                    subtitle: result.subtitle.clone(),
                    target: result.target.clone(),
                });
            prune(&mut store, now);
        }
        self.save();
    }

    /// Every entry's frecency, computed once so a search can rank hundreds of
    /// candidates without taking the lock for each one.
    pub fn scores(&self) -> HashMap<String, f64> {
        let now = chrono::Utc::now().timestamp();
        self.store
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), frecency(entry, now)))
            .collect()
    }

    /// The most-used recent picks that can be opened again, best first.
    pub fn recent(&self, limit: usize) -> Vec<LauncherResult> {
        let now = chrono::Utc::now().timestamp();
        let store = self.store.lock().unwrap();
        let mut items: Vec<(f64, LauncherResult)> = store
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                let item = entry.item.as_ref()?;
                Some((
                    frecency(entry, now),
                    LauncherResult {
                        id: id.clone(),
                        kind: item.kind.clone(),
                        title: item.title.clone(),
                        subtitle: item.subtitle.clone(),
                        target: item.target.clone(),
                        section: None,
                    },
                ))
            })
            .collect();
        items.sort_by(|left, right| right.0.total_cmp(&left.0));
        items
            .into_iter()
            .map(|(_, item)| item)
            .take(limit)
            .collect()
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

/// Usage count decayed by how long it's been since last picked — recent,
/// frequently-picked results float to the top without a stale favorite
/// dominating forever. Zero for anything never picked.
fn frecency(entry: &Entry, now: i64) -> f64 {
    let age_hours = (now - entry.last_used_secs).max(0) as f64 / 3600.0;
    entry.uses as f64 / (1.0 + age_hours / 24.0)
}

fn prune(store: &mut Store, now: i64) {
    if store.entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut ranked: Vec<(f64, String)> = store
        .entries
        .iter()
        .map(|(id, entry)| (frecency(entry, now), id.clone()))
        .collect();
    ranked.sort_by(|left, right| left.0.total_cmp(&right.0));
    let excess = store.entries.len() - MAX_ENTRIES;
    for (_, id) in ranked.into_iter().take(excess) {
        store.entries.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(id: &str, kind: &str) -> LauncherResult {
        LauncherResult {
            id: id.to_string(),
            kind: kind.to_string(),
            title: id.to_string(),
            subtitle: String::new(),
            target: format!("/tmp/{id}"),
            section: None,
        }
    }

    fn temp_history(name: &str) -> (LauncherHistory, PathBuf) {
        let dir = std::env::temp_dir().join(format!("samlu-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (LauncherHistory::load(&dir), dir)
    }

    #[test]
    fn recent_offers_places_but_not_copies() {
        let (history, dir) = temp_history("recent");
        history.record_selection(&result("app:Safari", "app"));
        history.record_selection(&result("app:Safari", "app"));
        history.record_selection(&result("clipboard:1", "clipboard"));
        history.record_selection(&result("project:samlu", "project"));

        let recent = history.recent(10);
        let ids: Vec<&str> = recent.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["app:Safari", "project:samlu"]);
        assert!(history.scores()["app:Safari"] > history.scores()["project:samlu"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_files_without_items_still_load() {
        let dir = std::env::temp_dir().join(format!("samlu-history-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("launcher-history.json"),
            r#"{"entries":{"app:Xcode":{"uses":3,"last_used_secs":0}}}"#,
        )
        .unwrap();
        let history = LauncherHistory::load(&dir);
        assert!(history.scores().contains_key("app:Xcode"));
        assert!(history.recent(5).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_keeps_the_store_bounded() {
        let mut store = Store::default();
        for index in 0..(MAX_ENTRIES + 25) {
            store.entries.insert(
                format!("file:{index}"),
                Entry {
                    uses: 1,
                    last_used_secs: index as i64,
                    item: None,
                },
            );
        }
        prune(&mut store, 10_000);
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert!(!store.entries.contains_key("file:0"));
        assert!(store
            .entries
            .contains_key(&format!("file:{}", MAX_ENTRIES + 24)));
    }
}

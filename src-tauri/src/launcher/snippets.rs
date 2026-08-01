use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub title: String,
    pub keyword: String,
    pub content: String,
}

pub struct SnippetStore {
    path: PathBuf,
    snippets: Mutex<Vec<Snippet>>,
}

impl SnippetStore {
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join("snippets.json");
        let snippets = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self {
            path,
            snippets: Mutex::new(snippets),
        }
    }

    pub fn list(&self) -> Vec<Snippet> {
        self.snippets.lock().unwrap().clone()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Snippet> {
        let query = query.to_lowercase();
        self.snippets
            .lock()
            .unwrap()
            .iter()
            .filter(|snippet| {
                query.is_empty()
                    || snippet.title.to_lowercase().contains(&query)
                    || snippet.keyword.to_lowercase().contains(&query)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn upsert(&self, mut snippet: Snippet) -> Result<Snippet, String> {
        snippet.title = snippet.title.trim().to_string();
        snippet.keyword = snippet.keyword.trim().to_string();
        snippet.content = snippet.content.trim().to_string();
        if snippet.title.is_empty() || snippet.content.is_empty() {
            return Err("Snippet title and content are required.".to_string());
        }
        if snippet.id.is_empty() {
            snippet.id = format!(
                "{:x}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
        }
        {
            let mut snippets = self.snippets.lock().unwrap();
            if let Some(existing) = snippets.iter_mut().find(|item| item.id == snippet.id) {
                *existing = snippet.clone();
            } else {
                snippets.push(snippet.clone());
            }
        }
        self.save();
        Ok(snippet)
    }

    pub fn delete(&self, id: &str) {
        self.snippets
            .lock()
            .unwrap()
            .retain(|snippet| snippet.id != id);
        self.save();
    }

    fn save(&self) {
        let snippets = self.snippets.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&*snippets) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

#[tauri::command]
pub fn get_snippets(store: tauri::State<'_, std::sync::Arc<SnippetStore>>) -> Vec<Snippet> {
    store.list()
}

#[tauri::command]
pub fn save_snippet(
    snippet: Snippet,
    store: tauri::State<'_, std::sync::Arc<SnippetStore>>,
) -> Result<Snippet, String> {
    store.upsert(snippet)
}

#[tauri::command]
pub fn delete_snippet(id: String, store: tauri::State<'_, std::sync::Arc<SnippetStore>>) {
    store.delete(&id);
}

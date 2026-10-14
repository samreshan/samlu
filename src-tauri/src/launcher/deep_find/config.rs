//! Descriptive Find's preferences: off until turned on, and then only the
//! folders you choose.

use super::embed::Choice;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub roots: Vec<String>,
    pub excluded: Vec<String>,
    /// Source files in code projects; off by default, as in DigUp, because
    /// they flood an index meant for documents.
    pub include_code: bool,
    /// Which model supplies meaning: none, small (Model2Vec) or large
    /// (EmbeddingGemma 2).
    pub model: Choice,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_roots(),
            excluded: Vec::new(),
            include_code: false,
            model: Choice::Small,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Documents and Desktop: where people keep what they'd describe. Downloads
/// is left for the person to add, since it is mostly installers and copies.
fn default_roots() -> Vec<String> {
    [dirs::document_dir(), dirs::desktop_dir()]
        .into_iter()
        .flatten()
        .filter(|dir: &PathBuf| dir.is_dir())
        .map(|dir| dir.to_string_lossy().to_string())
        .collect()
}

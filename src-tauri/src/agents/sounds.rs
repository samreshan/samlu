//! Alert sounds: the system catalog, custom imports, and playback.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    NeedsInput,
    Completed,
    TurnFinished,
}

/// Display names of the system sounds, sorted.
pub fn system_sounds() -> Vec<String> {
    Vec::new()
}

/// `"none"` | `"system:<Name>"` | `"custom:<file>"`.
pub fn resolve(choice: &str, app_data_dir: &Path) -> Option<PathBuf> {
    let _ = (choice, app_data_dir);
    None
}

/// Copies `source` into the app data dir; returns `"custom:<file>"`.
pub fn import_custom(source: &Path, app_data_dir: &Path) -> Result<String, String> {
    let _ = (source, app_data_dir);
    Err("not implemented".to_string())
}

/// No-op on `none` or a missing file.
pub fn play_choice(choice: &str, volume: u8, app_data_dir: &Path) {
    let _ = (choice, volume, app_data_dir);
}

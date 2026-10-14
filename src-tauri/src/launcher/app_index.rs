//! In-memory index of installed macOS applications — built via a direct
//! filesystem scan instead of querying Spotlight/`mdfind`. Installed apps
//! are a small (a few hundred), rarely-changing dataset; going through
//! Spotlight's metadata database via a subprocess for this is unnecessary
//! overhead. (Raycast's own engineering write-up on their v2 search describes
//! moving away from Spotlight-backed file search to their own filesystem
//! indexer for the same reason.) Rebuilt on every launcher-open, which is
//! cheap enough (low single-digit milliseconds for a few hundred bundles)
//! that no file-watching is needed for a v1.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
pub struct AppEntry {
    /// The name Finder shows: the bundle's file name without `.app`.
    pub name: String,
    pub path: String,
    /// Other names the bundle answers to, such as "Code" for Visual Studio
    /// Code. Matched, never displayed.
    pub aliases: Vec<String>,
}

pub struct AppIndex {
    apps: Mutex<Vec<AppEntry>>,
}

impl AppIndex {
    pub fn new() -> Self {
        let index = Self {
            apps: Mutex::new(Vec::new()),
        };
        index.rebuild();
        index
    }

    /// Re-scans all known app directories. Cheap enough to call every time
    /// the launcher opens, so staleness after installing/removing an app is
    /// never visible for more than one open.
    pub fn rebuild(&self) {
        let mut apps = Vec::new();
        for dir in app_directories() {
            scan_directory(&dir, &mut apps);
        }
        for path in standalone_apps() {
            if let Some(entry) = entry_for(&path) {
                apps.push(entry);
            }
        }
        let mut seen = HashSet::new();
        apps.retain(|entry| seen.insert(entry.path.clone()));
        *self.apps.lock().unwrap() = apps;
    }

    pub fn entries(&self) -> Vec<AppEntry> {
        self.apps.lock().unwrap().clone()
    }
}

fn app_directories() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/Applications/Chromium Apps"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
        PathBuf::from("/System/Library/CoreServices/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
        dirs.push(home.join("Applications/Chrome Apps.localized"));
        // Setapp's member apps are real .app bundles nested inside
        // Setapp.app's own Resources folder — not loose in /Applications —
        // so they need this explicit extra path or Setapp users get nothing.
        dirs.push(
            home.join(
                "Library/Application Support/Setapp/Setapp.app/Contents/Resources/Applications",
            ),
        );
    }
    dirs
}

/// Apps people expect to find that live outside the scanned folders.
fn standalone_apps() -> Vec<PathBuf> {
    vec![PathBuf::from("/System/Library/CoreServices/Finder.app")]
}

/// One level deep only — deliberately does not recurse into a found .app's
/// own Contents/ (that's where helper/background apps like
/// Xcode.app/Contents/Applications/Simulator.app live, which aren't meant to
/// be launched directly from a top-level search).
fn scan_directory(dir: &Path, out: &mut Vec<AppEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(app) = entry_for(&entry.path()) {
            out.push(app);
        }
    }
}

fn entry_for(path: &Path) -> Option<AppEntry> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("app") || !path.exists() {
        return None;
    }
    let file_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let info = bundle_names(path);
    let name = if file_name.is_empty() {
        info.first().cloned()?
    } else {
        file_name
    };
    let mut aliases: Vec<String> = Vec::new();
    for alias in info {
        if !alias.eq_ignore_ascii_case(&name) && !aliases.contains(&alias) {
            aliases.push(alias);
        }
    }
    Some(AppEntry {
        name,
        path: path.to_string_lossy().to_string(),
        aliases,
    })
}

#[derive(Deserialize, Default)]
struct InfoPlist {
    #[serde(rename = "CFBundleDisplayName")]
    display_name: Option<String>,
    #[serde(rename = "CFBundleName")]
    bundle_name: Option<String>,
}

/// The bundle's own names from Info.plist, which can differ from the file
/// name ("Code" for "Visual Studio Code.app"). Malformed or missing plists
/// simply contribute nothing.
fn bundle_names(app_path: &Path) -> Vec<String> {
    let Ok(info) = plist::from_file::<_, InfoPlist>(app_path.join("Contents/Info.plist")) else {
        return Vec::new();
    };
    [info.display_name, info.bundle_name]
        .into_iter()
        .flatten()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

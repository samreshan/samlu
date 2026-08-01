//! In-memory index of installed macOS applications — built via a direct
//! filesystem scan instead of querying Spotlight/`mdfind`. Installed apps
//! are a small (a few hundred), rarely-changing dataset; going through
//! Spotlight's metadata database via a subprocess for this is unnecessary
//! overhead. (Raycast's own engineering write-up on their v2 search describes
//! moving away from Spotlight-backed file search to their own filesystem
//! indexer for the same reason.) Rebuilt on every launcher-open, which is
//! cheap enough (low single-digit milliseconds for a few hundred bundles)
//! that no file-watching is needed for a v1.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
}

impl AsRef<str> for AppEntry {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

pub struct AppIndex {
    apps: Mutex<Vec<AppEntry>>,
    matcher: Mutex<Matcher>,
}

impl AppIndex {
    pub fn new() -> Self {
        let index = Self {
            apps: Mutex::new(Vec::new()),
            matcher: Mutex::new(Matcher::new(Config::DEFAULT)),
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
        *self.apps.lock().unwrap() = apps;
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<AppEntry> {
        if query.is_empty() {
            return Vec::new();
        }
        let apps = self.apps.lock().unwrap().clone();
        let mut matcher = self.matcher.lock().unwrap();
        Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart)
            .match_list(apps, &mut matcher)
            .into_iter()
            .take(limit)
            .map(|(entry, _score)| entry)
            .collect()
    }
}

fn app_directories() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Chromium Apps"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
        PathBuf::from("/System/Library/CoreServices/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
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

/// One level deep only — deliberately does not recurse into a found .app's
/// own Contents/ (that's where helper/background apps like
/// Xcode.app/Contents/Applications/Simulator.app live, which aren't meant to
/// be launched directly from a top-level search).
fn scan_directory(dir: &Path, out: &mut Vec<AppEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("app") {
            continue;
        }
        let name = app_display_name(&path).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string()
        });
        if name.is_empty() {
            continue;
        }
        out.push(AppEntry {
            name,
            path: path.to_string_lossy().to_string(),
        });
    }
}

#[derive(Deserialize, Default)]
struct InfoPlist {
    #[serde(rename = "CFBundleDisplayName")]
    display_name: Option<String>,
    #[serde(rename = "CFBundleName")]
    bundle_name: Option<String>,
}

/// Reads the bundle's real display name from Info.plist — falls back to the
/// folder name (handled by the caller) for malformed/missing plists rather
/// than skipping the app entirely.
fn app_display_name(app_path: &Path) -> Option<String> {
    let info_plist: InfoPlist = plist::from_file(app_path.join("Contents/Info.plist")).ok()?;
    info_plist.display_name.or(info_plist.bundle_name)
}

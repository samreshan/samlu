//! In-memory index of installed macOS applications — built via a direct
//! filesystem scan instead of querying Spotlight/`mdfind`. Installed apps
//! are a small (a few hundred), rarely-changing dataset; going through
//! Spotlight's metadata database via a subprocess for this is unnecessary
//! overhead. (Raycast's own engineering write-up on their v2 search describes
//! moving away from Spotlight-backed file search to their own filesystem
//! indexer for the same reason.) Rebuilt on every launcher-open, which is
//! cheap enough (low single-digit milliseconds for a few hundred bundles)
//! that no file-watching is needed for a v1.

#[cfg(not(windows))]
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
        #[cfg(windows)]
        apps.extend(store_apps::entries());
        let mut seen = HashSet::new();
        apps.retain(|entry| seen.insert(entry.path.clone()));
        *self.apps.lock().unwrap() = apps;
    }

    pub fn entries(&self) -> Vec<AppEntry> {
        self.apps.lock().unwrap().clone()
    }
}

#[cfg(not(windows))]
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
#[cfg(not(windows))]
fn standalone_apps() -> Vec<PathBuf> {
    vec![PathBuf::from("/System/Library/CoreServices/Finder.app")]
}

/// One level deep only — deliberately does not recurse into a found .app's
/// own Contents/ (that's where helper/background apps like
/// Xcode.app/Contents/Applications/Simulator.app live, which aren't meant to
/// be launched directly from a top-level search).
#[cfg(not(windows))]
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

#[cfg(not(windows))]
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

#[cfg(not(windows))]
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
#[cfg(not(windows))]
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

/// The Start Menu, for everyone and for this user: each app installs a
/// shortcut there.
#[cfg(windows)]
fn app_directories() -> Vec<PathBuf> {
    const PROGRAMS: &str = r"Microsoft\Windows\Start Menu\Programs";
    [
        std::env::var_os("ProgramData").map(|folder| PathBuf::from(folder).join(PROGRAMS)),
        dirs::data_dir().map(|folder| folder.join(PROGRAMS)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(windows)]
fn standalone_apps() -> Vec<PathBuf> {
    Vec::new()
}

/// Shortcuts sit in folders per vendor, a level or two down.
#[cfg(windows)]
fn scan_directory(dir: &Path, out: &mut Vec<AppEntry>) {
    for entry in walkdir::WalkDir::new(dir)
        .max_depth(4)
        .into_iter()
        .flatten()
    {
        if let Some(app) = entry_for(entry.path()) {
            out.push(app);
        }
    }
}

/// A Start Menu shortcut, named as Windows shows it. Uninstallers, readmes
/// and help files that installers put next to the app are left out.
#[cfg(windows)]
fn entry_for(path: &Path) -> Option<AppEntry> {
    let is_shortcut = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
    if !is_shortcut {
        return None;
    }
    let name = path.file_stem()?.to_string_lossy().to_string();
    let lower = name.to_lowercase();
    if ["uninstall", "readme", "release notes", "documentation"]
        .iter()
        .any(|word| lower.contains(word))
        || lower == "help"
        || lower.ends_with(" help")
    {
        return None;
    }
    Some(AppEntry {
        name,
        path: path.to_string_lossy().to_string(),
        aliases: Vec::new(),
    })
}

/// Microsoft Store apps have no shortcut file. Windows lists them through
/// `Get-StartApps`, which takes a moment, so the list is refreshed in the
/// background and joins the index once it arrives. Each opens through
/// `shell:AppsFolder`.
#[cfg(windows)]
mod store_apps {
    use super::AppEntry;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const REFRESH_EVERY: Duration = Duration::from_secs(10 * 60);

    struct Cache {
        refreshed: Option<Instant>,
        running: bool,
        apps: Vec<AppEntry>,
    }

    static CACHE: Mutex<Cache> = Mutex::new(Cache {
        refreshed: None,
        running: false,
        apps: Vec::new(),
    });

    pub fn entries() -> Vec<AppEntry> {
        let mut cache = CACHE.lock().unwrap();
        let stale = cache
            .refreshed
            .is_none_or(|refreshed| refreshed.elapsed() > REFRESH_EVERY);
        if stale && !cache.running {
            cache.running = true;
            std::thread::spawn(refresh);
        }
        cache.apps.clone()
    }

    fn refresh() {
        let apps = list().unwrap_or_default();
        let mut cache = CACHE.lock().unwrap();
        cache.running = false;
        cache.refreshed = Some(Instant::now());
        if !apps.is_empty() {
            cache.apps = apps;
        }
    }

    fn list() -> Option<Vec<AppEntry>> {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "ConvertTo-Json -Compress -InputObject @(Get-StartApps)",
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| parse(&String::from_utf8_lossy(&output.stdout)))
    }

    #[derive(serde::Deserialize)]
    struct StartApp {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "AppID")]
        app_id: String,
    }

    /// Store apps are the ones whose id names a package (`Package!App`);
    /// the rest have Start Menu shortcuts already.
    pub(super) fn parse(json: &str) -> Vec<AppEntry> {
        serde_json::from_str::<Vec<StartApp>>(json.trim())
            .unwrap_or_default()
            .into_iter()
            .filter(|app| app.app_id.contains('!') && !app.name.trim().is_empty())
            .map(|app| AppEntry {
                name: app.name.trim().to_string(),
                path: format!(r"shell:AppsFolder\{}", app.app_id),
                aliases: Vec::new(),
            })
            .collect()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn store_apps_are_the_packaged_ones() {
        let apps = store_apps::parse(
            r#"[{"Name":"Calculator","AppID":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"},{"Name":"Visual Studio Code","AppID":"{6D809377-6AF0-444B-8957-A3773F02200E}\\Microsoft VS Code\\Code.exe"}]"#,
        );
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Calculator");
        assert_eq!(
            apps[0].path,
            r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn shortcuts_are_apps_and_uninstallers_are_not() {
        let app = entry_for(Path::new(r"C:\Start\Programs\Zed\Zed.lnk")).unwrap();
        assert_eq!(
            (app.name.as_str(), app.path.as_str()),
            ("Zed", r"C:\Start\Programs\Zed\Zed.lnk")
        );
        assert!(entry_for(Path::new(r"C:\Start\Programs\Zed\Uninstall Zed.lnk")).is_none());
        assert!(entry_for(Path::new(r"C:\Start\Programs\Zed\notes.txt")).is_none());
    }
}

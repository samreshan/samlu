//! Alert sounds: the system catalog, custom imports, and playback.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    NeedsInput,
    Completed,
    TurnFinished,
}

/// File extension of the system alert sounds on this platform.
fn system_sound_ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "aiff"
    } else {
        "wav"
    }
}

/// Extensions accepted for custom imports. Windows plays through PlaySound,
/// which only takes WAV.
fn importable_extensions() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["aiff", "aif", "wav", "mp3", "m4a", "caf"]
    } else {
        &["wav"]
    }
}

/// Display names of the system sounds, sorted.
pub fn system_sounds() -> Vec<String> {
    let Some(dir) = crate::platform::system_sound_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let ext = system_sound_ext();
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| has_extension(path, &[ext]))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

/// `"none"` | `"system:<Name>"` | `"custom:<file>"`.
pub fn resolve(choice: &str, app_data_dir: &Path) -> Option<PathBuf> {
    if let Some(name) = choice.strip_prefix("system:") {
        if !is_plain_name(name) {
            return None;
        }
        let file = format!("{name}.{}", system_sound_ext());
        let path = crate::platform::system_sound_dir()?.join(file);
        return path.is_file().then_some(path);
    }
    if let Some(file) = choice.strip_prefix("custom:") {
        if !is_plain_name(file) {
            return None;
        }
        let path = custom_dir(app_data_dir).join(file);
        return path.is_file().then_some(path);
    }
    None
}

/// Copies `source` into the app data dir; returns `"custom:<file>"`.
/// A name already taken gets a `-2`, `-3`… suffix instead of overwriting.
pub fn import_custom(source: &Path, app_data_dir: &Path) -> Result<String, String> {
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !importable_extensions().contains(&ext.as_str()) {
        return Err(format!("unsupported sound file: {}", source.display()));
    }
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("no file name: {}", source.display()))?;

    let dir = custom_dir(app_data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut file = format!("{stem}.{ext}");
    let mut n = 2;
    while dir.join(&file).exists() {
        file = format!("{stem}-{n}.{ext}");
        n += 1;
    }
    std::fs::copy(source, dir.join(&file)).map_err(|e| e.to_string())?;
    Ok(format!("custom:{file}"))
}

/// No-op on `none` or a missing file. Playback runs on its own thread, so
/// the caller never waits for the sound to finish.
pub fn play_choice(choice: &str, volume: u8, app_data_dir: &Path) {
    let Some(path) = resolve(choice, app_data_dir) else {
        return;
    };
    std::thread::spawn(move || crate::platform::play_sound(&path, volume));
}

fn custom_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("sounds")
}

/// A single normal path component: no separators, no `..`, no drive prefix.
fn is_plain_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    )
}

fn has_extension(path: &Path, allowed: &[&str]) -> bool {
    path.extension()
        .map(|e| {
            let e = e.to_string_lossy();
            allowed.iter().any(|a| e.eq_ignore_ascii_case(a))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to one test; removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("samlu-sounds-{}-{}", std::process::id(), name));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolve_none_and_unknown() {
        let scratch = Scratch::new("resolve-none");
        assert!(resolve("none", scratch.path()).is_none());
        assert!(resolve("bogus", scratch.path()).is_none());
    }

    #[test]
    fn resolve_custom_inside_sounds_dir_only() {
        let scratch = Scratch::new("resolve-custom");
        // A file outside the sounds folder must never be reachable.
        std::fs::write(scratch.path().join("x.wav"), b"x").unwrap();
        assert!(resolve("custom:../x.wav", scratch.path()).is_none());

        let sounds = scratch.path().join("sounds");
        std::fs::create_dir_all(&sounds).unwrap();
        std::fs::write(sounds.join("a.wav"), b"a").unwrap();
        assert_eq!(
            resolve("custom:a.wav", scratch.path()),
            Some(sounds.join("a.wav"))
        );
        assert!(resolve("custom:missing.wav", scratch.path()).is_none());
    }

    #[test]
    fn import_custom_copies_and_dedupes_name() {
        let scratch = Scratch::new("import-dedupe");
        let source_dir = scratch.path().join("picked");
        std::fs::create_dir_all(&source_dir).unwrap();
        let ext = importable_extensions()[0];
        let source = source_dir.join(format!("ding.{ext}"));
        std::fs::write(&source, b"ding").unwrap();

        // Importing the same file twice must keep both copies.
        let app = scratch.path().join("app");
        let first = import_custom(&source, &app).unwrap();
        let second = import_custom(&source, &app).unwrap();
        assert_eq!(first, format!("custom:ding.{ext}"));
        assert_eq!(second, format!("custom:ding-2.{ext}"));
        assert!(app.join("sounds").join(format!("ding.{ext}")).is_file());
        assert!(app.join("sounds").join(format!("ding-2.{ext}")).is_file());
    }

    #[test]
    fn import_rejects_unsupported_extension() {
        let scratch = Scratch::new("import-reject");
        let app = scratch.path().join("app");
        let text = scratch.path().join("notes.txt");
        std::fs::write(&text, b"hello").unwrap();
        assert!(import_custom(&text, &app).is_err());

        for ext in importable_extensions() {
            let file = scratch.path().join(format!("clip.{ext}"));
            std::fs::write(&file, b"audio").unwrap();
            assert!(import_custom(&file, &app).is_ok(), "{ext} should import");
        }
        if !cfg!(target_os = "macos") {
            let aiff = scratch.path().join("clip.aiff");
            std::fs::write(&aiff, b"audio").unwrap();
            assert!(import_custom(&aiff, &app).is_err());
        }
    }
}

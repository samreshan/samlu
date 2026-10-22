//! Streams catalog models to `<name>.part`, verifies SHA-256, then renames.
//! Only a verified file ever appears under its final name. A model made of
//! several files (Bark) downloads into a `<name>.part` folder, every file is
//! verified, and the folder is renamed once all of them are.

use super::models::{self, CatalogEntry, CatalogFile, ModelKind};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const PROGRESS_EVENT: &str = "voice://model-download";
pub const CANCELLED: &str = "Download cancelled.";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub id: String,
    pub received: u64,
    pub total: u64,
}

#[derive(Default)]
pub struct Downloads {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Downloads {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.lock().unwrap().contains_key(id)
    }

    pub fn cancel(&self, id: &str) -> bool {
        match self.active.lock().unwrap().get(id) {
            Some(flag) => {
                flag.store(true, Ordering::Release);
                true
            }
            None => false,
        }
    }

    fn begin(&self, id: &str) -> Result<Arc<AtomicBool>, String> {
        let mut active = self.active.lock().unwrap();
        if active.contains_key(id) {
            return Err("This model is already downloading.".to_string());
        }
        let flag = Arc::new(AtomicBool::new(false));
        active.insert(id.to_string(), flag.clone());
        Ok(flag)
    }

    fn end(&self, id: &str) {
        self.active.lock().unwrap().remove(id);
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn finish(part: &Path, dest: &Path, expected: &str, actual: &str) -> Result<(), String> {
    if !expected.eq_ignore_ascii_case(actual) {
        let _ = std::fs::remove_file(part);
        return Err("The download was corrupted (checksum mismatch). Try again.".to_string());
    }
    std::fs::rename(part, dest).map_err(|error| {
        let _ = std::fs::remove_file(part);
        format!("Could not save the model: {error}")
    })
}

/// Refuses anything outside Samlu's own models folder, so discovered or
/// hand-added files belonging to other apps are never deleted.
pub fn delete_downloaded(models_dir: &Path, path: &Path) -> Result<(), String> {
    let dir = std::fs::canonicalize(models_dir).map_err(|error| error.to_string())?;
    let file = std::fs::canonicalize(path).map_err(|_| models::MODEL_NOT_FOUND.to_string())?;
    if file.parent() != Some(dir.as_path()) {
        return Err("Only models Samlu downloaded can be deleted.".to_string());
    }
    let removed = if file.is_dir() {
        std::fs::remove_dir_all(&file)
    } else {
        std::fs::remove_file(&file)
    };
    removed.map_err(|error| format!("Could not delete the model: {error}"))
}

/// Moves a fully verified staging folder into place, replacing any older
/// copy.
pub fn finish_folder(staging: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|error| format!("Could not replace the model: {error}"))?;
    }
    std::fs::rename(staging, dest).map_err(|error| {
        let _ = std::fs::remove_dir_all(staging);
        format!("Could not save the model: {error}")
    })
}

/// No overall timeout: large models take minutes. A stalled connection still
/// fails through the read timeout.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(60))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub async fn download(
    app: &AppHandle,
    entry: &'static CatalogEntry,
    dir: &Path,
    downloads: &Downloads,
) -> Result<PathBuf, String> {
    let cancel = downloads.begin(entry.id)?;
    let dest = dir.join(entry.file);
    let result = match entry.kind {
        ModelKind::Whisper => download_file(app, entry, dir, &dest, &cancel).await,
        ModelKind::Bark => download_folder(app, entry, dir, &dest, &cancel).await,
    };
    downloads.end(entry.id);
    result.map(|()| dest)
}

async fn download_file(
    app: &AppHandle,
    entry: &CatalogEntry,
    dir: &Path,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let file = &entry.files[0];
    let part = dir.join(format!("{}.part", entry.file));
    let mut progress = Tracker::new(app, entry.id, entry.bytes());
    match stream_to(entry, file, &part, &mut progress, cancel).await {
        Ok(actual) => finish(&part, dest, file.sha256, &actual),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

async fn download_folder(
    app: &AppHandle,
    entry: &CatalogEntry,
    dir: &Path,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let staging = dir.join(format!("{}.part", entry.file));
    let _ = std::fs::remove_dir_all(&staging);
    let mut progress = Tracker::new(app, entry.id, entry.bytes());
    for file in entry.files {
        let path = staging.join(file.local);
        let verified = match stream_to(entry, file, &path, &mut progress, cancel).await {
            Ok(actual) if file.sha256.eq_ignore_ascii_case(&actual) => Ok(()),
            Ok(_) => Err("The download was corrupted (checksum mismatch). Try again.".to_string()),
            Err(error) => Err(error),
        };
        if let Err(error) = verified {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    }
    finish_folder(&staging, dest)
}

/// Progress across every file of one model, emitted at most every
/// `PROGRESS_INTERVAL`.
struct Tracker<'a> {
    app: &'a AppHandle,
    id: &'static str,
    received: u64,
    total: u64,
    last_emit: Option<Instant>,
}

impl<'a> Tracker<'a> {
    fn new(app: &'a AppHandle, id: &'static str, total: u64) -> Self {
        Self {
            app,
            id,
            received: 0,
            total,
            last_emit: None,
        }
    }

    fn advance(&mut self, bytes: u64) {
        self.received += bytes;
        if self
            .last_emit
            .is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL)
        {
            self.last_emit = Some(Instant::now());
            let _ = self.app.emit(
                PROGRESS_EVENT,
                Progress {
                    id: self.id.to_string(),
                    received: self.received,
                    total: self.total.max(self.received),
                },
            );
        }
    }
}

/// Writes one file's response to `part` and returns its SHA-256.
async fn stream_to(
    entry: &CatalogEntry,
    source: &CatalogFile,
    part: &Path,
    progress: &mut Tracker<'_>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut response = client()?
        .get(models::download_url(entry, source))
        .send()
        .await
        .map_err(|error| format!("Download failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed ({}).", response.status()));
    }
    let mut file = std::fs::File::create(part).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(CANCELLED.to_string());
        }
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("Download failed: {error}"))?
        else {
            break;
        };
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|error| format!("Could not write the model: {error}"))?;
        progress.advance(chunk.len() as u64);
    }
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("samlu-download-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hex_is_lowercase_two_digits_per_byte() {
        assert_eq!(hex(&[0x00, 0xab, 0x0f]), "00ab0f");
    }

    #[test]
    fn checksum_mismatch_deletes_the_partial_file() {
        let dir = temp_dir("mismatch");
        let part = dir.join("m.bin.part");
        let dest = dir.join("m.bin");
        std::fs::write(&part, b"data").unwrap();
        assert!(finish(&part, &dest, "aa", "bb").is_err());
        assert!(!part.exists());
        assert!(!dest.exists());
    }

    #[test]
    fn matching_checksum_moves_the_file_into_place() {
        let dir = temp_dir("match");
        let part = dir.join("m.bin.part");
        let dest = dir.join("m.bin");
        std::fs::write(&part, b"data").unwrap();
        finish(&part, &dest, "AB", "ab").unwrap();
        assert!(!part.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    }

    #[test]
    fn a_verified_folder_replaces_the_old_copy() {
        let dir = temp_dir("folder");
        let staging = dir.join("Bark.part");
        let dest = dir.join("Bark");
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(staging.join("sub/new"), b"new").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old"), b"old").unwrap();
        finish_folder(&staging, &dest).unwrap();
        assert!(!staging.exists());
        assert!(!dest.join("old").exists());
        assert_eq!(std::fs::read(dest.join("sub/new")).unwrap(), b"new");
    }

    #[test]
    fn one_download_per_model_and_cancel_flags_it() {
        let downloads = Downloads::new();
        let flag = downloads.begin("base").unwrap();
        assert!(downloads.begin("base").is_err());
        assert!(downloads.is_active("base"));
        assert!(downloads.cancel("base"));
        assert!(flag.load(Ordering::Acquire));
        downloads.end("base");
        assert!(!downloads.is_active("base"));
        assert!(!downloads.cancel("base"));
    }

    #[test]
    fn only_samlu_downloaded_models_can_be_deleted() {
        let data = temp_dir("delete");
        let dir = crate::voice::models::models_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("ggml-base.bin");
        std::fs::write(&inside, b"x").unwrap();
        let outside = data.join("ggml-other.bin");
        std::fs::write(&outside, b"x").unwrap();
        assert!(delete_downloaded(&dir, &outside).is_err());
        assert!(outside.exists());
        delete_downloaded(&dir, &inside).unwrap();
        assert!(!inside.exists());
        let folder = dir.join("Underdog-Bark-3.0");
        std::fs::create_dir_all(folder.join("BarkEncoder.mlpackage")).unwrap();
        std::fs::write(folder.join("vocab.json"), b"{}").unwrap();
        delete_downloaded(&dir, &folder).unwrap();
        assert!(!folder.exists());
        // A folder's contents are not models of their own.
        let nested = dir.join("Other/vocab.json");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"{}").unwrap();
        assert!(delete_downloaded(&dir, &nested).is_err());
    }
}

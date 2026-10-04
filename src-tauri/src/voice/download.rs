//! Streams catalog models to `<name>.part`, verifies SHA-256, then renames.
//! Only a verified file ever appears under its final name.

use super::models::{self, CatalogEntry};
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
    std::fs::remove_file(&file).map_err(|error| format!("Could not delete the model: {error}"))
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
    let part = dir.join(format!("{}.part", entry.file));
    let dest = dir.join(entry.file);
    let result = stream_to(app, entry, &part, &cancel).await;
    downloads.end(entry.id);
    match result {
        Ok(actual) => finish(&part, &dest, entry.sha256, &actual).map(|()| dest),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

/// Writes the response to `part` and returns its SHA-256.
async fn stream_to(
    app: &AppHandle,
    entry: &CatalogEntry,
    part: &Path,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut response = client()?
        .get(models::download_url(entry))
        .send()
        .await
        .map_err(|error| format!("Download failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed ({}).", response.status()));
    }
    let total = response.content_length().unwrap_or(entry.bytes);
    let mut file = std::fs::File::create(part).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut last_emit: Option<Instant> = None;
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
        received += chunk.len() as u64;
        if last_emit.map_or(true, |at| at.elapsed() >= PROGRESS_INTERVAL) {
            last_emit = Some(Instant::now());
            let _ = app.emit(
                PROGRESS_EVENT,
                Progress {
                    id: entry.id.to_string(),
                    received,
                    total,
                },
            );
        }
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
    }
}

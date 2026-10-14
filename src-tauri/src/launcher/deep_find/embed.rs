//! The meaning half of Descriptive Find, and the downloads behind it.
//!
//! Two models can supply meaning (or none, for exact words only):
//! - small: a static Model2Vec model, `potion-retrieval-32M` (~125 MB). A
//!   query embeds in microseconds on the CPU, pure Rust on macOS and Windows.
//! - large: EmbeddingGemma 2, the model DigUp uses (`gemma.rs`), on the GPU.
//!
//! Every file is downloaded once from a pinned revision, checked against its
//! SHA-256 and only then given its real name.

use model2vec_rs::model::StaticModel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const MODEL_ID: &str = "potion-retrieval-32M";
const REPO: &str = "minishlab/potion-retrieval-32M";
/// Pinned, so the vectors in an index always come from the same weights.
const REVISION: &str = "6fc8051fab2a1e0ee76689cf08c853792ac285e7";
/// Stored with the index; vectors made by anything else are made again.
pub const VECTOR_SOURCE: &str = "potion-retrieval-32M@6fc8051";
pub const CANCELLED: &str = "Download cancelled.";

/// Which model supplies meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    /// Exact words only.
    None,
    #[default]
    Small,
    Large,
}

impl Choice {
    pub fn as_str(self) -> &'static str {
        match self {
            Choice::None => "none",
            Choice::Small => "small",
            Choice::Large => "large",
        }
    }
}

/// A file to download: where from, its name on disk, and what it must be.
pub struct Remote {
    pub url: String,
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

fn small_files() -> Vec<Remote> {
    let file = |name: &'static str, bytes: u64, sha256: &'static str| Remote {
        url: format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{name}"),
        name,
        bytes,
        sha256,
    };
    vec![
        file(
            "config.json",
            202,
            "63c00d90824c832c04ec1d02b6a983fb90489bf049f29fbff15ba481b8a432ee",
        ),
        file(
            "tokenizer.json",
            1_493_150,
            "7d75cbc54318138807c401b0f0c9721117c628b39de8e8e0edb6cb17e0ee7d18",
        ),
        file(
            "model.safetensors",
            129_210_456,
            "07609e5bd33aad37900b3fd62f4ec96f6daec88ca4d46b9d8b928bfababf6ea0",
        ),
    ]
}

pub fn total_bytes() -> u64 {
    small_files().iter().map(|file| file.bytes).sum()
}

pub fn model_dir(root: &Path) -> PathBuf {
    root.join("models").join(MODEL_ID)
}

pub fn is_downloaded(root: &Path) -> bool {
    files_present(&model_dir(root), &small_files())
}

pub fn files_present(dir: &Path, files: &[Remote]) -> bool {
    files.iter().all(|file| {
        std::fs::metadata(dir.join(file.name)).is_ok_and(|meta| meta.len() == file.bytes)
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub model: String,
    pub received: u64,
    pub total: u64,
}

pub async fn download(
    root: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<(), String> {
    let total = total_bytes();
    fetch(
        &model_dir(root),
        &small_files(),
        cancel,
        0,
        total,
        &progress,
    )
    .await
}

/// Streams each file to `<name>.part` in `dir`, checks its SHA-256 and only
/// then gives it its real name. Files already there are kept. Progress counts
/// from `offset` of `total`, so several calls can make one progress bar.
pub async fn fetch(
    dir: &Path,
    files: &[Remote],
    cancel: &AtomicBool,
    offset: u64,
    total: u64,
    progress: &impl Fn(Progress),
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| error.to_string())?;
    let mut done = offset;
    let mut last_emit: Option<Instant> = None;
    let report = |received: u64| {
        progress(Progress {
            model: String::new(),
            received,
            total,
        })
    };
    for file in files {
        let dest = dir.join(file.name);
        if std::fs::metadata(&dest).is_ok_and(|meta| meta.len() == file.bytes) {
            done += file.bytes;
            continue;
        }
        let part = dir.join(format!("{}.part", file.name));
        // A dropped connection resumes where it stopped (Hugging Face and
        // GitHub both take ranges); big files are retried a few times.
        let mut attempt = 0;
        let actual = loop {
            attempt += 1;
            let have = std::fs::metadata(&part).map_or(0, |meta| meta.len());
            let have = if have > file.bytes { 0 } else { have };
            match fetch_one(&client, file, &part, have, cancel, &mut |received| {
                if last_emit.is_none_or(|at| at.elapsed() >= Duration::from_millis(250)) {
                    last_emit = Some(Instant::now());
                    report(done + received);
                }
            })
            .await
            {
                Ok(hash) => break hash,
                Err(error) if error == CANCELLED => {
                    let _ = std::fs::remove_file(&part);
                    return Err(error);
                }
                Err(error) if attempt >= 4 => return Err(error),
                Err(error) => {
                    log::warn!(
                        "descriptive find: download of {} interrupted ({error}), resuming",
                        file.name
                    );
                    tokio_sleep(Duration::from_secs(2 * attempt)).await;
                }
            }
        };
        if actual != file.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err("The download was corrupted (checksum mismatch). Try again.".into());
        }
        std::fs::rename(&part, &dest).map_err(|error| error.to_string())?;
        done += file.bytes;
    }
    report(done);
    Ok(())
}

/// Downloads one file into `part`, continuing after the `have` bytes already
/// there, and returns the SHA-256 of the whole file.
async fn fetch_one(
    client: &reqwest::Client,
    file: &Remote,
    part: &Path,
    have: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<String, String> {
    let mut request = client.get(&file.url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request
        .send()
        .await
        .map_err(|error| format!("Download failed: {error}"))?;
    let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !response.status().is_success() {
        return Err(format!("Download failed ({}).", response.status()));
    }
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut out = if resumed {
        // The part already on disk counts toward the hash.
        let mut existing = std::fs::File::open(part).map_err(|error| error.to_string())?;
        std::io::copy(&mut existing, &mut hasher).map_err(|error| error.to_string())?;
        received = have;
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|error| error.to_string())?
    } else {
        std::fs::File::create(part).map_err(|error| error.to_string())?
    };
    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|error| format!("Download failed: {error}"))?
    {
        if cancel.load(Ordering::Acquire) {
            return Err(CANCELLED.to_string());
        }
        out.write_all(&bytes).map_err(|error| error.to_string())?;
        hasher.update(&bytes);
        received += bytes.len() as u64;
        progress(received);
    }
    out.flush().map_err(|error| error.to_string())?;
    if received != file.bytes {
        return Err(format!(
            "Download stopped at {received} of {} bytes",
            file.bytes
        ));
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

async fn tokio_sleep(duration: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(duration)).await;
}

pub fn load(root: &Path) -> Result<StaticModel, String> {
    let dir = model_dir(root);
    let read = |name: &str| std::fs::read(dir.join(name)).map_err(|error| error.to_string());
    StaticModel::from_bytes(
        read("tokenizer.json")?,
        read("model.safetensors")?,
        read("config.json")?,
        Some(true),
    )
    .map_err(|error| error.to_string())
}

/// Embeds passages. A tokenizer panic on odd input must not take the
/// indexer down, so it is caught and the batch skipped.
pub fn embed_batch(model: &StaticModel, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        model.encode_with_args(texts, Some(512), 64)
    }))
    .ok()
}

pub fn embed_query(model: &StaticModel, query: &str) -> Option<Vec<f32>> {
    embed_batch(model, &[query.to_string()])?.into_iter().next()
}

/// A unit vector stored as one byte a dimension and its scale: a quarter of
/// the room of floats, and cosine similarity barely moves.
pub fn quantize(vector: &[f32]) -> Vec<u8> {
    let max = vector.iter().fold(0f32, |max, value| max.max(value.abs()));
    let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
    let mut out = Vec::with_capacity(4 + vector.len());
    out.extend_from_slice(&scale.to_le_bytes());
    out.extend(
        vector
            .iter()
            .map(|value| (value / scale).round().clamp(-127.0, 127.0) as i8 as u8),
    );
    out
}

/// The scale and the bytes of a stored vector, or `None` if it is malformed.
pub fn dequantize_parts(blob: &[u8]) -> Option<(f32, &[u8])> {
    let scale = f32::from_le_bytes(blob.get(..4)?.try_into().ok()?);
    Some((scale, &blob[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_vectors_keep_their_direction() {
        let vector: Vec<f32> = (0..512)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 300.0)
            .collect();
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        let unit: Vec<f32> = vector.iter().map(|v| v / norm).collect();
        let blob = quantize(&unit);
        let (scale, bytes) = dequantize_parts(&blob).unwrap();
        let dot: f32 = unit
            .iter()
            .zip(bytes)
            .map(|(value, &byte)| value * (byte as i8) as f32 * scale)
            .sum();
        assert!((dot - 1.0).abs() < 0.01, "cosine with itself was {dot}");
    }
}

//! whisper.cpp models: the download catalog, file validation, and the merged
//! list of downloaded, discovered, and hand-added models.

use serde::Serialize;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MODEL_NOT_FOUND: &str = "Model not found — choose another in Voice settings.";
pub const NOT_A_MODEL: &str = "Not a whisper.cpp model";

/// Discovered files smaller than this are not models (the smallest catalog
/// model is 148 MB; tiny models are ~75 MB).
const MIN_MODEL_BYTES: u64 = 10 * 1024 * 1024;
const SCAN_DEPTH: usize = 6;
/// `ggml` stored little-endian, as whisper.cpp reads it.
const GGML_MAGIC: u32 = 0x6767_6d6c;
const HEADER_BYTES: usize = 44;

pub struct CatalogEntry {
    pub id: &'static str,
    pub file: &'static str,
    pub label: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// Official `ggerganov/whisper.cpp` GGML files. Sizes and SHA-256 values are
/// the Hugging Face LFS metadata for each file.
pub const CATALOG: [CatalogEntry; 5] = [
    CatalogEntry {
        id: "base",
        file: "ggml-base.bin",
        label: "Base · fastest",
        bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    },
    CatalogEntry {
        id: "small",
        file: "ggml-small.bin",
        label: "Small · balanced",
        bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    CatalogEntry {
        id: "medium",
        file: "ggml-medium.bin",
        label: "Medium · accurate, slower",
        bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    CatalogEntry {
        id: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        label: "Large v3 Turbo · best",
        bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    CatalogEntry {
        id: "large-v3-turbo-q5_0",
        file: "ggml-large-v3-turbo-q5_0.bin",
        label: "Large v3 Turbo (compact) · best for size",
        bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
];

pub fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.id == id)
}

pub fn download_url(entry: &CatalogEntry) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        entry.file
    )
}

/// Each slice of the universal binary answers for its own architecture.
pub fn recommended_id() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "large-v3-turbo-q5_0"
    } else {
        "small"
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ModelHeader {
    pub n_vocab: i32,
    pub n_mels: i32,
}

/// Validates the GGML magic and the whisper hyperparameters that distinguish
/// a whisper model from other GGML files.
pub fn parse_header(bytes: &[u8]) -> Result<ModelHeader, String> {
    if bytes.len() < HEADER_BYTES {
        return Err(NOT_A_MODEL.to_string());
    }
    let word = |index: usize| {
        let start = index * 4;
        u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap())
    };
    if word(0) != GGML_MAGIC {
        return Err(NOT_A_MODEL.to_string());
    }
    let n_vocab = word(1) as i32;
    let n_mels = word(10) as i32;
    if !(51_000..=52_000).contains(&n_vocab) || !matches!(n_mels, 80 | 128) {
        return Err(NOT_A_MODEL.to_string());
    }
    Ok(ModelHeader { n_vocab, n_mels })
}

pub fn inspect(path: &Path) -> Result<ModelHeader, String> {
    let mut file = std::fs::File::open(path).map_err(|_| MODEL_NOT_FOUND.to_string())?;
    let mut header = [0_u8; HEADER_BYTES];
    file.read_exact(&mut header)
        .map_err(|_| NOT_A_MODEL.to_string())?;
    parse_header(&header)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Downloaded,
    Discovered,
    Added,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub path: String,
    pub name: String,
    pub source: ModelSource,
    pub bytes: u64,
    pub missing: bool,
    pub english_only: bool,
}

pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Folders where other Mac dictation apps and the Hugging Face CLI keep
/// whisper.cpp models. Files are referenced in place, never copied.
pub fn discovery_roots(home: &Path) -> Vec<PathBuf> {
    [
        "Library/Application Support/MacWhisper",
        "Library/Application Support/superwhisper",
        "superwhisper",
        ".cache/huggingface/hub/models--ggerganov--whisper.cpp",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .collect()
}

fn scan_with_min(root: &Path, min_bytes: u64) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `metadata` follows symlinks, which the Hugging Face cache uses.
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if depth < SCAN_DEPTH {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            let is_bin = path.extension().is_some_and(|extension| extension == "bin");
            if is_bin && metadata.len() >= min_bytes && inspect(&path).is_ok() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn info(path: &Path, source: ModelSource, missing: bool) -> ModelInfo {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    ModelInfo {
        path: path.to_string_lossy().into_owned(),
        english_only: super::engines::is_english_only(&name),
        name,
        source,
        bytes: std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        missing,
    }
}

fn list_with_min(
    app_data_dir: &Path,
    home: &Path,
    added: &[String],
    min_bytes: u64,
) -> Vec<ModelInfo> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    let mut push = |path: &Path, source: ModelSource, missing: bool| {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.insert(key) {
            models.push(info(path, source, missing));
        }
    };
    for path in scan_with_min(&models_dir(app_data_dir), min_bytes) {
        push(&path, ModelSource::Downloaded, false);
    }
    for path in added.iter().map(PathBuf::from) {
        let missing = inspect(&path).is_err();
        push(&path, ModelSource::Added, missing);
    }
    for root in discovery_roots(home) {
        for path in scan_with_min(&root, min_bytes) {
            push(&path, ModelSource::Discovered, false);
        }
    }
    models
}

pub fn list(app_data_dir: &Path, home: &Path, added: &[String]) -> Vec<ModelInfo> {
    list_with_min(app_data_dir, home, added, MIN_MODEL_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 44 bytes of the real `ggml-tiny.en.bin`.
    const TINY_EN_HEADER: [u8; 44] = [
        0x6c, 0x6d, 0x67, 0x67, 0x98, 0xca, 0x00, 0x00, 0xdc, 0x05, 0x00, 0x00, 0x80, 0x01, 0x00,
        0x00, 0x06, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0xc0, 0x01, 0x00, 0x00, 0x80, 0x01,
        0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00,
    ];

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_model(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = TINY_EN_HEADER.to_vec();
        bytes.extend([0_u8; 64]);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn accepts_a_real_whisper_header() {
        let header = parse_header(&TINY_EN_HEADER).unwrap();
        assert_eq!(header.n_vocab, 51_864);
        assert_eq!(header.n_mels, 80);
    }

    #[test]
    fn rejects_wrong_magic_truncated_and_non_whisper_files() {
        let mut wrong_magic = TINY_EN_HEADER;
        wrong_magic[0] = 0;
        assert_eq!(parse_header(&wrong_magic).unwrap_err(), NOT_A_MODEL);
        assert_eq!(
            parse_header(&TINY_EN_HEADER[..20]).unwrap_err(),
            NOT_A_MODEL
        );
        let mut llama_like = TINY_EN_HEADER;
        llama_like[4..8].copy_from_slice(&32_000_i32.to_le_bytes());
        assert_eq!(parse_header(&llama_like).unwrap_err(), NOT_A_MODEL);
    }

    #[test]
    fn inspect_reports_missing_files() {
        let missing = temp_dir("missing").join("nope.bin");
        assert_eq!(inspect(&missing).unwrap_err(), MODEL_NOT_FOUND);
    }

    #[test]
    fn scan_finds_nested_models_and_skips_other_files() {
        let root = temp_dir("scan");
        write_model(&root.join("a/b/ggml-base.en.bin"));
        std::fs::write(root.join("notes.bin"), b"plain text").unwrap();
        std::fs::write(root.join("ggml-small.bin.part"), TINY_EN_HEADER).unwrap();
        let found = scan_with_min(&root, 0);
        assert_eq!(found, vec![root.join("a/b/ggml-base.en.bin")]);
    }

    #[test]
    fn list_marks_missing_added_models_and_dedupes() {
        let data = temp_dir("list-data");
        let home = temp_dir("list-home");
        let downloaded = models_dir(&data).join("ggml-base.bin");
        write_model(&downloaded);
        let missing = home.join("gone/ggml-medium.bin");
        let added = vec![
            downloaded.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ];
        let models = list_with_min(&data, &home, &added, 0);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].source, ModelSource::Downloaded);
        assert!(!models[0].missing);
        assert_eq!(models[1].source, ModelSource::Added);
        assert!(models[1].missing);
    }

    #[test]
    fn catalog_ids_resolve_and_recommendation_exists() {
        assert!(catalog_entry(recommended_id()).is_some());
        let entry = catalog_entry("large-v3-turbo-q5_0").unwrap();
        assert_eq!(
            download_url(entry),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
        );
    }

    #[test]
    fn discovery_roots_excludes_documents() {
        let home = Path::new("/Users/test");
        let roots = discovery_roots(home);
        assert_eq!(roots.len(), 4);
        assert!(!roots
            .iter()
            .any(|p| p.to_string_lossy().contains("Documents")));
    }
}

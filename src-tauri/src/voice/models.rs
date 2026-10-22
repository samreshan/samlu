//! Local speech models: the download catalog, file validation, and the
//! merged list of downloaded, discovered, and hand-added models. A whisper.cpp
//! model is one GGML file; an Underdog Bark model is a folder holding a Core
//! ML encoder, its decoder weights and its vocabulary.

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
/// The files that make a folder an Underdog Bark model.
const BARK_FILES: [&str; 3] = [
    "BarkEncoder.mlpackage/Manifest.json",
    "decoder.npz",
    "vocab.json",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Whisper,
    Bark,
}

/// One file of a catalog model: where it lives in the Hugging Face repo and
/// where it goes inside Samlu's models folder.
pub struct CatalogFile {
    pub remote: &'static str,
    pub local: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub struct CatalogEntry {
    pub id: &'static str,
    /// The model's name in the models folder: a file for whisper, a folder
    /// for Bark.
    pub file: &'static str,
    pub label: &'static str,
    pub kind: ModelKind,
    /// The Hugging Face repo and revision the files come from.
    repo: &'static str,
    pub files: &'static [CatalogFile],
}

impl CatalogEntry {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    pub fn english_only(&self) -> bool {
        self.kind == ModelKind::Bark || super::engines::is_english_only(self.file)
    }
}

const WHISPER_REPO: &str = "ggerganov/whisper.cpp/resolve/main";

const fn whisper(
    id: &'static str,
    label: &'static str,
    files: &'static [CatalogFile],
) -> CatalogEntry {
    CatalogEntry {
        id,
        file: files[0].local,
        label,
        kind: ModelKind::Whisper,
        repo: WHISPER_REPO,
        files,
    }
}

/// Official `ggerganov/whisper.cpp` GGML files, and Underdog Bark 3.0's Core
/// ML (Neural Engine) package pinned to one revision. Sizes and SHA-256
/// values are the Hugging Face LFS metadata for each file, or the hash of the
/// file at that revision for small non-LFS files.
pub const CATALOG: [CatalogEntry; 6] = [
    whisper(
        "base",
        "Whisper Base · fastest",
        &[CatalogFile {
            remote: "ggml-base.bin",
            local: "ggml-base.bin",
            bytes: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        }],
    ),
    whisper(
        "small",
        "Whisper Small · balanced",
        &[CatalogFile {
            remote: "ggml-small.bin",
            local: "ggml-small.bin",
            bytes: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }],
    ),
    whisper(
        "medium",
        "Whisper Medium · accurate, slower",
        &[CatalogFile {
            remote: "ggml-medium.bin",
            local: "ggml-medium.bin",
            bytes: 1_533_763_059,
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        }],
    ),
    whisper(
        "large-v3-turbo",
        "Whisper Large v3 Turbo · best",
        &[CatalogFile {
            remote: "ggml-large-v3-turbo.bin",
            local: "ggml-large-v3-turbo.bin",
            bytes: 1_624_555_275,
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        }],
    ),
    whisper(
        "large-v3-turbo-q5_0",
        "Whisper Large v3 Turbo (compact) · best for size",
        &[CatalogFile {
            remote: "ggml-large-v3-turbo-q5_0.bin",
            local: "ggml-large-v3-turbo-q5_0.bin",
            bytes: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        }],
    ),
    CatalogEntry {
        id: "underdog-bark-3.0",
        file: "Underdog-Bark-3.0",
        label: "Underdog Bark 3.0 · English, most accurate for size",
        kind: ModelKind::Bark,
        repo: "ConwayResearch/Underdog-Bark-3.0/resolve/85db2ad18ab41e534a222ee253c29191d77ed919",
        files: &[
            CatalogFile {
                remote: "coreml/ane/BarkEncoder.mlpackage/Manifest.json",
                local: "BarkEncoder.mlpackage/Manifest.json",
                bytes: 617,
                sha256: "d37c121561b99da49b9bf1e28c00152c14c56a943815fa83008ee0691b8f1149",
            },
            CatalogFile {
                remote: "coreml/ane/BarkEncoder.mlpackage/Data/com.apple.CoreML/model.mlmodel",
                local: "BarkEncoder.mlpackage/Data/com.apple.CoreML/model.mlmodel",
                bytes: 1_784_701,
                sha256: "7c3f1c1612ba21bcc4d8efff1b32154b39cb3e1e2f10020117793acc5c96fcdf",
            },
            CatalogFile {
                remote: "coreml/ane/BarkEncoder.mlpackage/Data/com.apple.CoreML/weights/weight.bin",
                local: "BarkEncoder.mlpackage/Data/com.apple.CoreML/weights/weight.bin",
                bytes: 189_133_632,
                sha256: "98979e5dd731486ddfb2c312f80a3038205c058aac9fe0f403ba26e1a158d385",
            },
            CatalogFile {
                remote: "coreml/ane/decoder.npz",
                local: "decoder.npz",
                bytes: 11_168_286,
                sha256: "1ce79071f54c3dcec9aa9bb1d488486766f1fdbb3fd988b776da3f4da8024665",
            },
            CatalogFile {
                remote: "coreml/ane/vocab.json",
                local: "vocab.json",
                bytes: 115_878,
                sha256: "eee86951e9f74921342fdc60ba26711eb70f7188e8784d98d2aecfc525b72270",
            },
            // CC-BY-4.0 attribution travels with the model.
            CatalogFile {
                remote: "coreml/NOTICE",
                local: "NOTICE",
                bytes: 603,
                sha256: "7760825e72004ccf97f7a8b63a75e2890105a7cae8ca875b70d4df78cff9abcd",
            },
        ],
    },
];

/// Whether this machine can run a kind of model. Bark's Core ML package is
/// built for Apple silicon and needs macOS 15.
pub fn kind_supported(kind: ModelKind) -> bool {
    match kind {
        ModelKind::Whisper => true,
        ModelKind::Bark => cfg!(target_arch = "aarch64") && super::engines::bark::available(),
    }
}

/// The catalog this machine can run.
pub fn catalog() -> impl Iterator<Item = &'static CatalogEntry> {
    CATALOG.iter().filter(|entry| kind_supported(entry.kind))
}

pub fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    catalog().find(|entry| entry.id == id)
}

pub fn download_url(entry: &CatalogEntry, file: &CatalogFile) -> String {
    format!("https://huggingface.co/{}/{}", entry.repo, file.remote)
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

/// A folder holding everything Bark loads. A download still in progress
/// (`*.part`) is not one yet.
pub fn is_bark_dir(path: &Path) -> bool {
    path.is_dir()
        && !path
            .extension()
            .is_some_and(|extension| extension == "part")
        && BARK_FILES.iter().all(|file| path.join(file).is_file())
}

/// What kind of local model `path` is, or why it is not one.
pub fn kind(path: &Path) -> Result<ModelKind, String> {
    if path.is_dir() {
        return if is_bark_dir(path) {
            Ok(ModelKind::Bark)
        } else {
            Err(MODEL_NOT_FOUND.to_string())
        };
    }
    inspect(path).map(|_| ModelKind::Whisper)
}

/// Whether `path` is a model this machine can run now.
pub fn usable(path: &Path) -> bool {
    kind(path).is_ok_and(kind_supported)
}

fn size_on_disk(path: &Path) -> u64 {
    if path.is_dir() {
        walkdir::WalkDir::new(path)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .sum()
    } else {
        std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    }
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
    pub kind: ModelKind,
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
                if is_bark_dir(&path) {
                    found.push(path);
                } else if depth < SCAN_DEPTH {
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
    let kind = if is_bark_dir(path) {
        ModelKind::Bark
    } else {
        ModelKind::Whisper
    };
    ModelInfo {
        english_only: kind == ModelKind::Bark || super::engines::is_english_only(&name),
        path: path.to_string_lossy().into_owned(),
        name,
        source,
        kind,
        bytes: size_on_disk(path),
        // A Bark model on a Mac that cannot run it is listed but not usable.
        missing: missing || !kind_supported(kind),
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
        let missing = kind(&path).is_err();
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

    fn write_bark(dir: &Path) {
        for file in BARK_FILES {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }
    }

    #[test]
    fn catalog_ids_resolve_and_recommendation_exists() {
        assert!(catalog_entry(recommended_id()).is_some());
        let entry = catalog_entry("large-v3-turbo-q5_0").unwrap();
        assert_eq!(
            download_url(entry, &entry.files[0]),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
        );
        assert_eq!(entry.file, "ggml-large-v3-turbo-q5_0.bin");
        assert!(!entry.english_only());
    }

    #[test]
    fn bark_is_one_pinned_english_only_folder() {
        let bark = CATALOG
            .iter()
            .find(|entry| entry.kind == ModelKind::Bark)
            .unwrap();
        assert!(bark.english_only());
        assert_eq!(bark.bytes(), 202_203_717);
        assert!(
            bark.repo.contains("/resolve/85db2ad"),
            "downloads are pinned"
        );
        assert_eq!(
            download_url(bark, &bark.files[3]),
            "https://huggingface.co/ConwayResearch/Underdog-Bark-3.0/resolve/85db2ad18ab41e534a222ee253c29191d77ed919/coreml/ane/decoder.npz"
        );
        for required in BARK_FILES {
            assert!(
                bark.files.iter().any(|file| file.local == required),
                "{required}"
            );
        }
        assert!(bark.files.iter().all(|file| file.sha256.len() == 64));
        // The catalog only offers Bark where Core ML can run it.
        assert_eq!(
            catalog_entry(bark.id).is_some(),
            kind_supported(ModelKind::Bark)
        );
        if cfg!(not(target_os = "macos")) {
            assert!(!kind_supported(ModelKind::Bark));
        }
    }

    #[test]
    fn bark_folders_are_found_whole_and_partial_downloads_skipped() {
        let root = temp_dir("bark-scan");
        let bark = root.join("Underdog-Bark-3.0");
        write_bark(&bark);
        write_bark(&root.join("Underdog-Bark-3.0.part"));
        // Missing its vocabulary: not a model.
        write_bark(&root.join("broken"));
        std::fs::remove_file(root.join("broken/vocab.json")).unwrap();
        assert!(is_bark_dir(&bark));
        assert_eq!(kind(&bark).unwrap(), ModelKind::Bark);
        assert_eq!(kind(&root.join("broken")).unwrap_err(), MODEL_NOT_FOUND);
        assert_eq!(scan_with_min(&root, 0), vec![bark.clone()]);
    }

    #[test]
    fn listed_bark_models_are_english_only_and_sized_as_a_folder() {
        let data = temp_dir("bark-list-data");
        let home = temp_dir("bark-list-home");
        let bark = models_dir(&data).join("Underdog-Bark-3.0");
        write_bark(&bark);
        let models = list_with_min(&data, &home, &[], 0);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].kind, ModelKind::Bark);
        assert!(models[0].english_only);
        assert_eq!(models[0].bytes, 6);
        assert_eq!(models[0].name, "Underdog-Bark-3.0");
        assert_eq!(models[0].missing, !kind_supported(ModelKind::Bark));
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

//! The large meaning model: Google DeepMind's EmbeddingGemma 2, the model
//! DigUp is built on, run by llama.cpp on the GPU (Metal on macOS, Vulkan on
//! Windows, falling back to the CPU).
//!
//! llama.cpp can't be linked into Samlu: Whisper already brings its own copy
//! of ggml, and two copies clash. So, as DigUp does on Windows and Linux, the
//! model runs in a helper process: llama.cpp's own `llama-server`, from its
//! pinned official release, answering embedding requests on a local port. It
//! starts when the launcher opens or indexing needs it and quits after ten
//! minutes unused, so its memory is only taken while you search.

use super::embed::{self, Remote};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MODEL_ID: &str = "embeddinggemma-2";
const REPO: &str = "ggml-org/embeddinggemma-2-GGUF";
/// The revision and files DigUp pins.
const REVISION: &str = "bfcd298762cc34d0357ece5ebdd31791a3a374d8";
pub const VECTOR_SOURCE: &str = "embeddinggemma-2-Q8_0@bfcd298";
const TEXT_MODEL: &str = "embeddinggemma-2-Q8_0.gguf";
/// The model's image and audio encoders. Downloaded with the model so it is
/// complete; document search uses the text part.
const MEDIA_MODEL: &str = "mmproj-embeddinggemma-2-Q8_0.gguf";
/// The first llama.cpp releases with EmbeddingGemma 2 are b11452+; this is
/// the one DigUp builds from.
const LLAMA_TAG: &str = "b11461";

struct Runtime {
    asset: &'static str,
    bytes: u64,
    sha256: &'static str,
}

/// llama.cpp's official build for this computer: Metal on macOS, Vulkan on
/// Windows and Linux (its CPU backend is in the same package).
fn runtime() -> Option<Runtime> {
    let runtime = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Runtime {
            asset: "llama-b11461-bin-macos-arm64.tar.gz",
            bytes: 12_007_683,
            sha256: "6cbf2bcbfe616e9e14f1e326fe34d15fbe127846ad4275f076b36694101ff2c1",
        }
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Runtime {
            asset: "llama-b11461-bin-macos-x64.tar.gz",
            bytes: 11_525_318,
            sha256: "a7a535031f6d54418ff8f34f29cdc53d5aa3bce3a5225ed19088b136a74ee1d5",
        }
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Runtime {
            asset: "llama-b11461-bin-win-vulkan-x64.zip",
            bytes: 33_377_638,
            sha256: "6b9ca8ad3e43ac3e166d1997875709e4547214bb5bf1c2ebfdd17a2350a8380c",
        }
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Runtime {
            asset: "llama-b11461-bin-win-vulkan-arm64.zip",
            bytes: 25_915_514,
            sha256: "0ae64fba0c134a4cad0d7d371515a2073ab4b93f3b9d7bd3df6684471fc2448c",
        }
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Runtime {
            asset: "llama-b11461-bin-ubuntu-vulkan-x64.tar.gz",
            bytes: 31_678_968,
            sha256: "6e1e169c5b045cf2ce83bb017448f11929ef40a02e028b5e7bdda9835fdc80fe",
        }
    } else {
        return None;
    };
    Some(runtime)
}

pub fn is_supported() -> bool {
    runtime().is_some()
}

fn model_files() -> Vec<Remote> {
    let url = |name: &str| format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{name}");
    vec![
        Remote {
            url: url(TEXT_MODEL),
            name: TEXT_MODEL,
            bytes: 309_855_456,
            sha256: "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
        },
        Remote {
            url: url(MEDIA_MODEL),
            name: MEDIA_MODEL,
            bytes: 554_821_024,
            sha256: "c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f",
        },
    ]
}

pub fn total_bytes() -> u64 {
    model_files().iter().map(|file| file.bytes).sum::<u64>()
        + runtime().map_or(0, |runtime| runtime.bytes)
}

fn model_dir(root: &Path) -> PathBuf {
    root.join("models").join(MODEL_ID)
}

fn runtime_dir(root: &Path) -> PathBuf {
    root.join("runtime").join(format!("llama-{LLAMA_TAG}"))
}

fn server_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// `llama-server` wherever the release archive put it.
fn server_binary(root: &Path) -> Option<PathBuf> {
    walkdir::WalkDir::new(runtime_dir(root))
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| entry.file_type().is_file() && entry.file_name() == server_name())
        .map(|entry| entry.into_path())
}

pub fn is_downloaded(root: &Path) -> bool {
    embed::files_present(&model_dir(root), &model_files()) && server_binary(root).is_some()
}

/// The model files and the llama.cpp build, each checked against its
/// SHA-256, then the build unpacked next to the models.
pub async fn download(
    root: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(embed::Progress),
) -> Result<(), String> {
    let runtime = runtime().ok_or("EmbeddingGemma 2 isn't available for this computer.")?;
    let archive_dir = root.join("runtime");
    let archive = Remote {
        url: format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_TAG}/{}",
            runtime.asset
        ),
        name: runtime.asset,
        bytes: runtime.bytes,
        sha256: runtime.sha256,
    };
    let models = model_files();
    let total = total_bytes();
    let need_runtime = server_binary(root).is_none();
    let runtime_bytes = if need_runtime { runtime.bytes } else { 0 };
    if need_runtime {
        embed::fetch(
            &archive_dir,
            std::slice::from_ref(&archive),
            cancel,
            0,
            total,
            &progress,
        )
        .await?;
    }
    embed::fetch(
        &model_dir(root),
        &models,
        cancel,
        runtime_bytes,
        total,
        &progress,
    )
    .await?;
    if need_runtime {
        let archive_path = archive_dir.join(runtime.asset);
        let dest = runtime_dir(root);
        tauri::async_runtime::spawn_blocking(move || unpack(&archive_path, &dest))
            .await
            .map_err(|error| error.to_string())??;
        let _ = std::fs::remove_file(archive_dir.join(runtime.asset));
    }
    if server_binary(root).is_none() {
        return Err("The llama.cpp download didn't contain llama-server.".into());
    }
    Ok(())
}

fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(dest))
            .map_err(|error| format!("Couldn't unpack llama.cpp: {error}"))?;
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(dest)
            .map_err(|error| format!("Couldn't unpack llama.cpp: {error}"))?;
    }
    #[cfg(unix)]
    for entry in walkdir::WalkDir::new(dest)
        .into_iter()
        .filter_map(Result::ok)
    {
        use std::os::unix::fs::PermissionsExt;
        let is_tool = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("llama-"));
        if entry.file_type().is_file() && is_tool {
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/* -------------------------------------------------------------- prompts */

/// EmbeddingGemma's task prompts, from its model card (as DigUp uses them).
pub fn query_prompt(query: &str) -> String {
    format!("task: search result | query: {query}")
}

pub fn document_prompt(title: &str, text: &str) -> String {
    let title = if title.trim().is_empty() {
        "none"
    } else {
        title
    };
    format!("title: {title} | text: {text}")
}

/* --------------------------------------------------------------- server */

pub struct Server {
    child: Mutex<Child>,
    base: String,
    agent: ureq::Agent,
    pid_file: PathBuf,
}

#[derive(Deserialize)]
struct EmbeddingReply {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl Server {
    /// Starts `llama-server` with the text model and waits until it answers.
    pub fn start(root: &Path) -> Result<Self, String> {
        let binary = server_binary(root).ok_or("llama.cpp isn't downloaded.")?;
        let model = model_dir(root).join(TEXT_MODEL);
        let pid_file = root.join("llama-server.pid");
        end_stale(&pid_file);
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|addr| addr.port())
            .map_err(|error| error.to_string())?;
        let log_path = root.join("llama-server.log");
        let log = std::fs::File::create(&log_path).map_err(|error| error.to_string())?;
        let mut command = Command::new(&binary);
        command
            .current_dir(binary.parent().unwrap_or(root))
            .arg("--model")
            .arg(&model)
            .args(["--embedding", "--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            // A passage is ~450 tokens; non-causal embedding needs a whole
            // input in one batch.
            .args([
                "--ctx-size",
                "8192",
                "--batch-size",
                "4096",
                "--ubatch-size",
                "4096",
            ])
            .args(["--parallel", "1", "--n-gpu-layers", "999", "--no-webui"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Couldn't start llama.cpp: {error}"))?;
        let _ = std::fs::write(&pid_file, child.id().to_string());
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .build();
        let base = format!("http://127.0.0.1:{port}");
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: String = log.lines().rev().take(4).collect::<Vec<_>>().join(" / ");
                return Err(format!("llama.cpp stopped ({status}): {tail}"));
            }
            match agent.get(&format!("{base}/health")).call() {
                Ok(response) if response.status() == 200 => break,
                _ => {}
            }
            if started.elapsed() > Duration::from_secs(90) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("llama.cpp didn't start in time.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        log::info!(
            "descriptive find: EmbeddingGemma 2 ready in {:?}",
            started.elapsed()
        );
        Ok(Self {
            child: Mutex::new(child),
            base,
            agent,
            pid_file,
        })
    }

    pub fn is_alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    /// Unit vectors for `inputs` (already prompted), in order.
    pub fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let reply: EmbeddingReply = self
            .agent
            .post(&format!("{}/v1/embeddings", self.base))
            .send_json(serde_json::json!({ "input": inputs, "encoding_format": "float" }))
            .map_err(|error| format!("llama.cpp: {error}"))?
            .into_json()
            .map_err(|error| format!("llama.cpp: {error}"))?;
        let mut items = reply.data;
        items.sort_by_key(|item| item.index);
        if items.len() != inputs.len() {
            return Err("llama.cpp returned the wrong number of vectors".into());
        }
        Ok(items
            .into_iter()
            .map(|item| {
                let norm = item.embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    item.embedding.iter().map(|v| v / norm).collect()
                } else {
                    item.embedding
                }
            })
            .collect())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

/// A helper left running by a Samlu that didn't get to stop it (a crash, a
/// forced quit) is ended before a new one starts, if it is still llama.cpp.
fn end_stale(pid_file: &Path) {
    let Some(pid) = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return;
    };
    let _ = std::fs::remove_file(pid_file);
    #[cfg(unix)]
    {
        let name = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();
        if name.contains("llama-server") {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let listing = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .creation_flags(0x0800_0000)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();
        if listing.contains("llama-server") {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .creation_flags(0x0800_0000)
                .status();
        }
    }
}

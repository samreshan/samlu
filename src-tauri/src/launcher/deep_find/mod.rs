//! Descriptive Find: an optional launcher add-on that searches inside your
//! files by what they say, not only by their names. Describe what you're
//! looking for ("the clause about pets in the lease", "invoice from the
//! design agency") and it finds the passage, with the page or line.
//!
//! The search follows DigUp (github.com/ARahim3/DigUp): meaning and exact
//! words in one ranking, keyword results while you type and meaning results
//! once you pause, and an index that only reads the folders you pick and
//! stays on this computer. The engine is portable Rust (SQLite FTS5,
//! `notify` for folder changes), so it runs the same on macOS and Windows.
//!
//! Meaning comes from the model you choose: none (exact words only), a small
//! Model2Vec model on the CPU, or EmbeddingGemma 2, DigUp's own model, on the
//! GPU through llama.cpp.

pub mod config;
pub mod embed;
pub mod engine;
pub mod extract;
pub mod gemma;
pub mod indexer;
pub mod rules;
pub mod searcher;
pub mod store;
pub mod text;

use config::Config;
use embed::Choice;
use engine::Engine;
use indexer::Job;
use rusqlite::Connection;
use searcher::Vectors;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use super::search::LauncherResult;

pub const STATUS_EVENT: &str = "deep-find://status";
pub const DOWNLOAD_EVENT: &str = "deep-find://download";
/// The model's memory is given back after this long unused (the large
/// model's helper quits); the next search loads it again, matching words in
/// the meantime, like DigUp's query encoder.
const MODEL_IDLE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// off | idle | indexing | error
    pub state: String,
    pub files: usize,
    pub passages: usize,
    pub vectors: usize,
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub current: Option<String>,
    pub error: Option<String>,
    /// The model that is loaded and answering, if any.
    pub model_loaded: Option<Choice>,
    /// The model being downloaded, if any.
    pub downloading: Option<Choice>,
    pub small_downloaded: bool,
    pub small_bytes: u64,
    pub large_downloaded: bool,
    pub large_bytes: u64,
    /// EmbeddingGemma 2 has a llama.cpp build for this computer.
    pub large_supported: bool,
}

#[derive(Default)]
struct ModelSlot {
    model: Option<Engine>,
    last_used: Option<Instant>,
    loading: bool,
}

pub struct DeepFind {
    pub(crate) root: PathBuf,
    config_path: PathBuf,
    config: Mutex<Config>,
    model: Mutex<ModelSlot>,
    vectors: RwLock<Arc<Vectors>>,
    reader: Mutex<Option<Connection>>,
    status: Mutex<Status>,
    jobs: Mutex<Option<Sender<Job>>>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    download_cancel: Mutex<Option<(Choice, Arc<AtomicBool>)>>,
    stopping: AtomicBool,
    app: Mutex<Option<AppHandle>>,
    last_emit: Mutex<Option<Instant>>,
}

impl DeepFind {
    pub fn new(app_data_dir: &Path) -> Arc<Self> {
        let root = app_data_dir.join("descriptive-find");
        let config_path = app_data_dir.join("descriptive-find.json");
        let config = Config::load(&config_path);
        Arc::new(Self {
            root,
            config_path,
            config: Mutex::new(config),
            model: Mutex::new(ModelSlot::default()),
            vectors: RwLock::new(Arc::new(Vectors::default())),
            reader: Mutex::new(None),
            status: Mutex::new(Status {
                state: "off".into(),
                ..Status::default()
            }),
            jobs: Mutex::new(None),
            watcher: Mutex::new(None),
            download_cancel: Mutex::new(None),
            stopping: AtomicBool::new(false),
            app: Mutex::new(None),
            last_emit: Mutex::new(None),
        })
    }

    /// Called once at startup: starts indexing if the add-on is on, and the
    /// timer that gives the model's memory back when it sits unused.
    pub fn start(self: &Arc<Self>, app: AppHandle) {
        *self.app.lock().unwrap() = Some(app);
        if self.config().enabled {
            self.turn_on();
        }
        let service = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("descriptive-find-idle".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(60));
                let Some(service) = service.upgrade() else {
                    return;
                };
                service.release_idle_model();
            })
            .ok();
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    fn update_config(&self, change: impl FnOnce(&mut Config)) -> Config {
        let mut config = self.config.lock().unwrap();
        change(&mut config);
        config.save(&self.config_path);
        config.clone()
    }

    fn turn_on(self: &Arc<Self>) {
        self.stopping.store(false, Ordering::Release);
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.is_none() {
            *jobs = Some(indexer::spawn(self.clone()));
        }
        let tx = jobs.clone().unwrap();
        drop(jobs);
        {
            let mut status = self.status.lock().unwrap();
            status.state = "idle".into();
            status.error = None;
        }
        self.restart_watcher();
        let _ = tx.send(Job::Sync);
        self.prewarm();
    }

    fn turn_off(&self) {
        self.stopping.store(true, Ordering::Release);
        *self.watcher.lock().unwrap() = None;
        if let Some(tx) = self.jobs.lock().unwrap().take() {
            let _ = tx.send(Job::Stop);
        }
        self.model.lock().unwrap().model = None;
        *self.vectors.write().unwrap() = Arc::new(Vectors::default());
        *self.reader.lock().unwrap() = None;
        self.status.lock().unwrap().state = "off".into();
        self.emit_status(true);
    }

    fn restart_watcher(&self) {
        let config = self.config();
        let Some(tx) = self.jobs.lock().unwrap().clone() else {
            return;
        };
        // Dropping the old watcher first ends its forwarding thread.
        *self.watcher.lock().unwrap() = None;
        *self.watcher.lock().unwrap() = indexer::watch(&config.roots, tx);
    }

    fn send(&self, job: Job) {
        if let Some(tx) = self.jobs.lock().unwrap().as_ref() {
            let _ = tx.send(job);
        }
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /* ------------------------------------------------------------- model */

    /// The launcher opening is the moment to get the model ready: by the
    /// time a description is typed, meaning search can answer.
    pub fn prewarm(self: &Arc<Self>) {
        let config = self.config();
        if !config.enabled || !self.is_downloaded(config.model) {
            return;
        }
        {
            let mut slot = self.model.lock().unwrap();
            slot.last_used = Some(Instant::now());
            let current = slot
                .model
                .as_ref()
                .is_some_and(|engine| engine.choice() == config.model && engine.is_alive());
            if current || slot.loading {
                return;
            }
        }
        let service = self.clone();
        std::thread::spawn(move || {
            if service.load_model().is_some() {
                service.emit_status(true);
            }
        });
    }

    /// The loaded model, loading it first if need be. Loading happens outside
    /// the lock, so a search asking meanwhile is never held up by it, and
    /// only once however many ask at the same time.
    fn load_model(&self) -> Option<Engine> {
        let choice = self.config().model;
        if !self.is_downloaded(choice) {
            return None;
        }
        loop {
            let mut slot = self.model.lock().unwrap();
            slot.last_used = Some(Instant::now());
            if let Some(engine) = &slot.model {
                if engine.choice() == choice && engine.is_alive() {
                    return Some(engine.clone());
                }
            }
            if !slot.loading {
                slot.loading = true;
                // Another model, or a helper that went away: let it go.
                slot.model = None;
                break;
            }
            drop(slot);
            std::thread::sleep(Duration::from_millis(50));
        }
        let loaded = Engine::load(choice, &self.root);
        let mut slot = self.model.lock().unwrap();
        slot.loading = false;
        match loaded {
            Ok(engine) => {
                slot.model = engine.clone();
                engine
            }
            Err(error) => {
                log::warn!("descriptive find: couldn't load the model: {error}");
                None
            }
        }
    }

    fn is_downloaded(&self, choice: Choice) -> bool {
        match choice {
            Choice::None => false,
            Choice::Small => embed::is_downloaded(&self.root),
            Choice::Large => gemma::is_downloaded(&self.root),
        }
    }

    /// Stops the large model's helper when Samlu quits.
    pub fn shutdown(&self) {
        self.model.lock().unwrap().model = None;
    }

    /// The model for a search, if it's loaded; otherwise loading starts and
    /// this search matches words only.
    fn model_for_query(self: &Arc<Self>) -> Option<Engine> {
        let choice = self.config().model;
        let model = {
            let mut slot = self.model.lock().unwrap();
            slot.last_used = Some(Instant::now());
            slot.model
                .clone()
                .filter(|engine| engine.choice() == choice && engine.is_alive())
        };
        if model.is_none() {
            self.prewarm();
        }
        model
    }

    /// The indexer waits for the model: passages read without it would need
    /// a second pass.
    pub(crate) fn model_for_indexing(&self) -> Option<Engine> {
        self.load_model()
    }

    fn release_idle_model(&self) {
        let mut slot = self.model.lock().unwrap();
        let busy = self.status.lock().unwrap().state == "indexing";
        if !busy
            && slot.model.is_some()
            && slot.last_used.is_some_and(|at| at.elapsed() > MODEL_IDLE)
        {
            slot.model = None;
        }
    }

    /* ------------------------------------------------------------ search */

    pub(crate) fn refresh_vectors(&self, conn: &Connection) {
        match Vectors::load(conn) {
            Ok(vectors) => *self.vectors.write().unwrap() = Arc::new(vectors),
            Err(error) => log::warn!("descriptive find: couldn't load vectors: {error}"),
        }
    }

    pub fn search(
        self: &Arc<Self>,
        query: &str,
        meaning: bool,
        limit: usize,
    ) -> Result<FindResponse, String> {
        let config = self.config();
        if !config.enabled {
            return Ok(FindResponse::default());
        }
        if query.trim().is_empty() {
            return Ok(FindResponse {
                enabled: true,
                ..FindResponse::default()
            });
        }
        let model = if meaning {
            self.model_for_query()
        } else {
            None
        };
        let query_vector = model.as_ref().and_then(|engine| engine.embed_query(query));
        let vectors = self.vectors.read().unwrap().clone();
        let mut reader = self.reader.lock().unwrap();
        if reader.is_none() {
            *reader = Some(store::open(&self.root)?);
        }
        let conn = reader.as_ref().unwrap();
        let found = searcher::search(
            conn,
            &vectors,
            query,
            query_vector.as_deref(),
            !meaning,
            limit,
        )?;
        let results = found.hits.into_iter().map(result_for).collect();
        Ok(FindResponse {
            enabled: true,
            results,
            strong: found.strong,
            meaning: query_vector.is_some(),
            warming: meaning && query_vector.is_none() && self.is_downloaded(config.model),
        })
    }

    /* ------------------------------------------------------------ status */

    pub fn status(&self) -> Status {
        let mut status = self.status.lock().unwrap().clone();
        status.model_loaded = self
            .model
            .lock()
            .unwrap()
            .model
            .as_ref()
            .map(Engine::choice);
        status.downloading = self
            .download_cancel
            .lock()
            .unwrap()
            .as_ref()
            .map(|(choice, _)| *choice);
        status.small_downloaded = embed::is_downloaded(&self.root);
        status.small_bytes = embed::total_bytes();
        status.large_downloaded = gemma::is_downloaded(&self.root);
        status.large_bytes = gemma::total_bytes();
        status.large_supported = gemma::is_supported();
        status.vectors = self.vectors.read().unwrap().len();
        status
    }

    pub(crate) fn begin_indexing(&self, phase: &str, total: usize) {
        {
            let mut status = self.status.lock().unwrap();
            status.state = "indexing".into();
            status.phase = phase.into();
            status.done = 0;
            status.total = total;
            status.current = None;
        }
        self.emit_status(true);
    }

    pub(crate) fn progress(&self, done: usize, total: usize, current: Option<&str>) {
        {
            let mut status = self.status.lock().unwrap();
            status.done = done;
            status.total = total;
            status.current = current.map(str::to_string);
        }
        self.emit_status(false);
    }

    pub(crate) fn finish_indexing(&self, conn: &Connection) {
        let (files, passages) = store::counts(conn);
        {
            let mut status = self.status.lock().unwrap();
            if status.state != "off" && status.state != "error" {
                status.state = "idle".into();
            }
            status.files = files;
            status.passages = passages;
            status.current = None;
            status.done = 0;
            status.total = 0;
        }
        self.emit_status(true);
    }

    pub(crate) fn set_error(&self, error: String) {
        log::warn!("descriptive find: {error}");
        {
            let mut status = self.status.lock().unwrap();
            status.state = "error".into();
            status.error = Some(error);
        }
        self.emit_status(true);
    }

    fn emit_status(&self, force: bool) {
        {
            let mut last = self.last_emit.lock().unwrap();
            if !force && last.is_some_and(|at| at.elapsed() < Duration::from_millis(250)) {
                return;
            }
            *last = Some(Instant::now());
        }
        if let Some(app) = self.app.lock().unwrap().as_ref() {
            let _ = app.emit(STATUS_EVENT, self.status());
        }
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FindResponse {
    /// Whether the add-on is on at all.
    pub enabled: bool,
    pub results: Vec<LauncherResult>,
    /// How many of `results` are strong matches; the rest trail far behind.
    pub strong: usize,
    /// Whether meaning took part (false: words only).
    pub meaning: bool,
    /// Meaning was asked for but the model is still loading.
    pub warming: bool,
}

/// A hit as a launcher row: the file's name, and under it where the match is
/// and the passage, its matched words marked « ».
fn result_for(hit: searcher::Hit) -> LauncherResult {
    let path = Path::new(&hit.path);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| hit.path.clone());
    let place = hit.loc.map(|loc| {
        if hit.kind == "pdf" {
            format!("Page {loc}")
        } else {
            format!("Line {loc}")
        }
    });
    let folder = path
        .parent()
        .map(|parent| tilde(&parent.to_string_lossy()))
        .unwrap_or_default();
    let subtitle = match (place, hit.snippet) {
        (Some(place), Some(snippet)) => format!("{place} · {snippet}"),
        (None, Some(snippet)) => snippet,
        (Some(place), None) => format!("{place} · {folder}"),
        (None, None) => folder,
    };
    LauncherResult {
        id: format!("content:{}", hit.path),
        kind: "content".into(),
        title: name,
        subtitle,
        target: hit.path,
        section: None,
    }
}

fn tilde(path: &str) -> String {
    match dirs::home_dir() {
        Some(home) => {
            let home = home.to_string_lossy();
            match path.strip_prefix(home.as_ref()) {
                Some(rest) => format!("~{rest}"),
                None => path.to_string(),
            }
        }
        None => path.to_string(),
    }
}

/* ---------------------------------------------------------------- commands */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(flatten)]
    config: Config,
    status: Status,
}

fn settings(service: &DeepFind) -> Settings {
    Settings {
        config: service.config(),
        status: service.status(),
    }
}

/// Searches inside files. `meaning: false` is the typing pass (words only,
/// the last word as a prefix); `true` is the pause, with meaning.
#[tauri::command]
pub async fn launcher_find_in_files(
    app: AppHandle,
    query: String,
    meaning: bool,
    limit: Option<usize>,
) -> Result<FindResponse, String> {
    let service = app.state::<Arc<DeepFind>>().inner().clone();
    let limit = limit.unwrap_or(24).clamp(1, 60);
    tauri::async_runtime::spawn_blocking(move || service.search(&query, meaning, limit))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_deep_find_settings(service: tauri::State<'_, Arc<DeepFind>>) -> Settings {
    settings(&service)
}

#[tauri::command]
pub fn set_deep_find_enabled(enabled: bool, service: tauri::State<'_, Arc<DeepFind>>) -> Settings {
    let service = service.inner().clone();
    let was = service.config().enabled;
    service.update_config(|config| config.enabled = enabled);
    if enabled && !was {
        service.turn_on();
    } else if !enabled && was {
        service.turn_off();
    }
    settings(&service)
}

#[tauri::command]
pub fn set_deep_find_folders(
    roots: Vec<String>,
    excluded: Vec<String>,
    include_code: bool,
    service: tauri::State<'_, Arc<DeepFind>>,
) -> Result<Settings, String> {
    let roots = normalize_folders(roots)?;
    let excluded = normalize_folders(excluded)?;
    let config = service.update_config(|config| {
        config.roots = roots;
        config.excluded = excluded;
        config.include_code = include_code;
    });
    if config.enabled {
        service.restart_watcher();
        service.send(Job::Sync);
    }
    Ok(settings(&service))
}

#[tauri::command]
pub fn deep_find_rebuild(service: tauri::State<'_, Arc<DeepFind>>) {
    service.send(Job::Rebuild);
}

/// Picks a folder with the system's own dialog (Finder's on macOS, the
/// shell's on Windows).
#[tauri::command]
pub async fn deep_find_choose_folder(app: AppHandle, title: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title(title)
            .blocking_pick_folder()
            .and_then(|path| path.into_path().ok())
            .map(|path| path.to_string_lossy().to_string())
    })
    .await
    .ok()
    .flatten()
}

/// Picks the model that supplies meaning: "none", "small" or "large".
/// Passages are given vectors by the new model in the background; searches
/// match words meanwhile.
#[tauri::command]
pub fn set_deep_find_model(model: Choice, service: tauri::State<'_, Arc<DeepFind>>) -> Settings {
    let service = service.inner().clone();
    service.update_config(|config| config.model = model);
    service.shutdown();
    if service.config().enabled {
        service.send(Job::Backfill);
        service.prewarm();
    }
    service.emit_status(true);
    settings(&service)
}

#[tauri::command]
pub async fn deep_find_download_model(app: AppHandle, model: Choice) -> Result<Settings, String> {
    let service = app.state::<Arc<DeepFind>>().inner().clone();
    if model == Choice::None {
        return Ok(settings(&service));
    }
    let cancel = {
        let mut slot = service.download_cancel.lock().unwrap();
        if slot.is_some() {
            return Err("A model is already downloading.".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        *slot = Some((model, flag.clone()));
        flag
    };
    service.emit_status(true);
    let emitter = app.clone();
    let name = model.as_str().to_string();
    let progress = move |mut progress: embed::Progress| {
        progress.model = name.clone();
        let _ = emitter.emit(DOWNLOAD_EVENT, progress);
    };
    let result = match model {
        Choice::Large => gemma::download(&service.root, &cancel, progress).await,
        _ => embed::download(&service.root, &cancel, progress).await,
    };
    *service.download_cancel.lock().unwrap() = None;
    if result.is_ok() && service.config().enabled && service.config().model == model {
        service.prewarm();
        service.send(Job::Backfill);
    }
    service.emit_status(true);
    result.map(|()| settings(&service))
}

#[tauri::command]
pub fn deep_find_cancel_download(service: tauri::State<'_, Arc<DeepFind>>) {
    if let Some((_, flag)) = service.download_cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Release);
    }
}

/// Turns the add-on off and removes everything it stored: the index and the
/// model. Your files are never touched.
#[tauri::command]
pub fn deep_find_remove_data(service: tauri::State<'_, Arc<DeepFind>>) -> Result<Settings, String> {
    let service = service.inner().clone();
    service.update_config(|config| config.enabled = false);
    service.turn_off();
    // The indexer closes its connection as it stops; give it a moment, since
    // Windows won't delete an open file.
    for _ in 0..20 {
        match std::fs::remove_dir_all(&service.root) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    if service.root.exists() {
        return Err("Couldn't remove the index. Try again in a moment.".into());
    }
    Ok(settings(&service))
}

fn normalize_folders(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for value in paths {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let path = PathBuf::from(value);
        if !path.is_absolute() || !path.is_dir() {
            return Err(format!("Folder is unavailable: {value}"));
        }
        let path = dunce_canonical(&path);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Canonical, without Windows' `\\?\` prefix, which nothing else expects.
fn dunce_canonical(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy().to_string();
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}

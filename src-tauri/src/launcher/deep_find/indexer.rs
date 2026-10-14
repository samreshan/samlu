//! Keeps the index current on a thread of its own. A sync walks the chosen
//! folders and reads only files that are new or changed since they were
//! last read; the folder watcher hands over the paths that changed, so a new
//! file is searchable a moment after it lands. Files are read and embedded a
//! batch at a time, and the in-memory vectors are refreshed as batches land,
//! so searches see the index grow without ever waiting on it.

use super::config::Config;
use super::embed;
use super::engine::Engine;
use super::extract;
use super::rules;
use super::store::{self, NewSegment};
use super::DeepFind;
use notify::{RecursiveMode, Watcher};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

const BATCH: usize = 24;
const MAX_FILES: usize = 250_000;
/// How long the watcher waits for things to go quiet before indexing.
const SETTLE: Duration = Duration::from_millis(1500);
const VECTOR_REFRESH: Duration = Duration::from_secs(2);
const BACKFILL_BATCH: usize = 256;

pub enum Job {
    Sync,
    Paths(Vec<PathBuf>),
    /// Drop the index and read everything again.
    Rebuild,
    /// The model arrived (or changed): give passages without a vector one.
    Backfill,
    Stop,
}

pub fn spawn(service: Arc<DeepFind>) -> Sender<Job> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("descriptive-find-indexer".into())
        .spawn(move || run(service, rx))
        .expect("spawn indexer thread");
    tx
}

/// Watches the roots and forwards what changed, once it settles.
pub fn watch(roots: &[String], jobs: Sender<Job>) -> Option<notify::RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })
    .map_err(|error| log::warn!("descriptive find: no folder watcher: {error}"))
    .ok()?;
    for root in roots {
        if let Err(error) = watcher.watch(Path::new(root), RecursiveMode::Recursive) {
            log::warn!("descriptive find: can't watch {root}: {error}");
        }
    }
    std::thread::Builder::new()
        .name("descriptive-find-watcher".into())
        .spawn(move || {
            let mut pending: HashSet<PathBuf> = HashSet::new();
            loop {
                match rx.recv_timeout(SETTLE) {
                    Ok(path) => {
                        pending.insert(path);
                    }
                    Err(RecvTimeoutError::Timeout) if !pending.is_empty() => {
                        if jobs.send(Job::Paths(pending.drain().collect())).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    // The watcher was dropped: the add-on was turned off or
                    // the folders changed.
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok()?;
    Some(watcher)
}

fn run(service: Arc<DeepFind>, rx: Receiver<Job>) {
    let mut conn = match store::open(&service.root) {
        Ok(conn) => conn,
        Err(error) => {
            service.set_error(format!("Couldn't open the index: {error}"));
            return;
        }
    };
    // What's already indexed is searchable before the first sync starts.
    service.refresh_vectors(&conn);
    service.finish_indexing(&conn);
    let mut indexer = Indexer {
        service: service.clone(),
        last_refresh: Instant::now(),
    };
    while let Ok(first) = rx.recv() {
        // Coalesce whatever queued up meanwhile: one sync covers any number
        // of them, and changed paths are merged.
        let mut sync = false;
        let mut rebuild = false;
        let mut backfill = false;
        let mut paths: Vec<PathBuf> = Vec::new();
        for job in std::iter::once(first).chain(rx.try_iter()) {
            match job {
                Job::Sync => sync = true,
                Job::Rebuild => rebuild = true,
                Job::Backfill => backfill = true,
                Job::Paths(changed) => paths.extend(changed),
                Job::Stop => return,
            }
        }
        let config = service.config();
        if rebuild {
            if let Err(error) = store::reset(&mut conn) {
                service.set_error(error);
                continue;
            }
            service.refresh_vectors(&conn);
            sync = true;
        }
        // A model change first: old vectors are replaced before new files are
        // read, so the index never mixes two models' vectors for long.
        if backfill || sync {
            if let Err(error) = indexer.backfill(&mut conn) {
                service.set_error(error);
            }
        }
        let result = if sync {
            indexer.sync(&mut conn, &config)
        } else if !paths.is_empty() {
            indexer.changed(&mut conn, &config, paths)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            service.set_error(error);
        }
        service.refresh_vectors(&conn);
        service.finish_indexing(&conn);
    }
}

struct Indexer {
    service: Arc<DeepFind>,
    last_refresh: Instant,
}

impl Indexer {
    /// Walks every root, reads what is new or changed, and forgets files
    /// that are gone or no longer in a chosen folder.
    fn sync(&mut self, conn: &mut Connection, config: &Config) -> Result<(), String> {
        self.service.begin_indexing("Looking for files…", 0);
        let mut found: HashMap<String, (rules::Kind, f64, u64)> = HashMap::new();
        for root in &config.roots {
            collect(Path::new(root), true, config, &mut found);
        }
        let known: HashMap<String, (f64, u64)> = store::known_files(conn)?
            .into_iter()
            .map(|(path, mtime, size)| (path, (mtime, size)))
            .collect();
        let gone: Vec<&String> = known
            .keys()
            .filter(|path| !found.contains_key(*path))
            .collect();
        if !gone.is_empty() {
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            for path in gone {
                store::remove_file(&tx, path)?;
            }
            tx.commit().map_err(|error| error.to_string())?;
        }
        let mut todo: Vec<(String, rules::Kind, f64, u64)> = found
            .into_iter()
            .filter(|(path, (_, mtime, size))| known.get(path) != Some(&(*mtime, *size)))
            .map(|(path, (kind, mtime, size))| (path, kind, mtime, size))
            .collect();
        // Newest first: what you were just working on is findable soonest.
        todo.sort_by(|a, b| b.2.total_cmp(&a.2));
        self.read(conn, todo)
    }

    /// Paths the watcher reported: read the files that changed, forget the
    /// ones that are gone, and walk any folder that appeared.
    fn changed(
        &mut self,
        conn: &mut Connection,
        config: &Config,
        paths: Vec<PathBuf>,
    ) -> Result<(), String> {
        let known: HashMap<String, (f64, u64)> = store::known_files(conn)?
            .into_iter()
            .map(|(path, mtime, size)| (path, (mtime, size)))
            .collect();
        let mut found: HashMap<String, (rules::Kind, f64, u64)> = HashMap::new();
        let mut gone: Vec<String> = Vec::new();
        for path in paths {
            if !rules::is_walked(&path, &config.roots, config.include_code, &config.excluded) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    if rules::walks_into(
                        &path,
                        config.roots.contains(&path_str),
                        config.include_code,
                        &config.excluded,
                    ) {
                        collect(&path, false, config, &mut found);
                    }
                }
                Ok(meta) if meta.is_file() => match rules::classify(&path, config.include_code) {
                    Some(kind) => {
                        found.insert(path_str, (kind, mtime(&meta), meta.len()));
                    }
                    None => gone.push(path_str),
                },
                Ok(_) => {}
                // Removed or renamed away: the file, or everything under a
                // folder that was.
                Err(_) => gone.extend(
                    known
                        .keys()
                        .filter(|known| rules::is_inside(known, &path_str))
                        .cloned(),
                ),
            }
        }
        if !gone.is_empty() {
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            for path in &gone {
                store::remove_file(&tx, path)?;
            }
            tx.commit().map_err(|error| error.to_string())?;
        }
        let todo: Vec<_> = found
            .into_iter()
            .filter(|(path, (_, mtime, size))| known.get(path) != Some(&(*mtime, *size)))
            .map(|(path, (kind, mtime, size))| (path, kind, mtime, size))
            .collect();
        self.read(conn, todo)
    }

    fn read(
        &mut self,
        conn: &mut Connection,
        todo: Vec<(String, rules::Kind, f64, u64)>,
    ) -> Result<(), String> {
        if todo.is_empty() {
            return Ok(());
        }
        let model = self.service.model_for_indexing();
        let total = todo.len();
        self.service.begin_indexing("Reading files…", total);
        for (batch_index, batch) in todo.chunks(BATCH).enumerate() {
            if self.service.stopping() {
                return Ok(());
            }
            // Extraction and embedding happen outside the transaction, so a
            // search never waits on a slow PDF.
            let prepared: Vec<_> = batch
                .iter()
                .map(|(path, kind, mtime, size)| {
                    let file = Path::new(path);
                    let (segments, error) = match extract::extract(file, *kind, *size) {
                        Ok(passages) => (embed_passages(model.as_ref(), file, passages), None),
                        Err(error) => (Vec::new(), Some(error)),
                    };
                    (path, kind, *mtime, *size, name_text(file), segments, error)
                })
                .collect();
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            for (path, kind, mtime, size, name, segments, error) in &prepared {
                store::put_file(
                    &tx,
                    path,
                    kind.as_str(),
                    *mtime,
                    *size,
                    name,
                    segments,
                    error.as_deref(),
                )?;
            }
            tx.commit().map_err(|error| error.to_string())?;
            self.service.progress(
                ((batch_index + 1) * BATCH).min(total),
                total,
                batch.last().map(|b| b.0.as_str()),
            );
            if self.last_refresh.elapsed() >= VECTOR_REFRESH {
                self.service.refresh_vectors(conn);
                self.last_refresh = Instant::now();
            }
        }
        // A fresh index takes the model's name; an index of another model's
        // vectors keeps its own until backfill has replaced them all.
        if let Some(model) = &model {
            if store::meta(conn, "vector_source")?.is_none() {
                store::set_meta(conn, "vector_source", model.source())?;
            }
        }
        Ok(())
    }

    /// Passages stored while the model wasn't there (or made by another
    /// model) get their vectors now, from the text the index already holds.
    fn backfill(&mut self, conn: &mut Connection) -> Result<(), String> {
        if self.service.config().model == embed::Choice::None {
            // Exact words only: vectors would only take memory.
            if store::meta(conn, "vector_source")?.as_deref() != Some("none") {
                conn.execute("UPDATE segments SET vec = NULL", [])
                    .map_err(|error| error.to_string())?;
                store::set_meta(conn, "vector_source", "none")?;
            }
            return Ok(());
        }
        let Some(model) = self.service.model_for_indexing() else {
            return Ok(());
        };
        if store::meta(conn, "vector_source")?.as_deref() != Some(model.source()) {
            conn.execute("UPDATE segments SET vec = NULL", [])
                .map_err(|error| error.to_string())?;
            store::set_meta(conn, "vector_source", model.source())?;
            self.service.refresh_vectors(conn);
        }
        let pending: Vec<(i64, String, String)> = {
            let mut statement = conn
                .prepare(
                    "SELECT s.id, f.path, t.body FROM segments s
                     JOIN files f ON f.id = s.file JOIN fts t ON t.rowid = s.id
                     WHERE s.kind = 'chunk' AND s.vec IS NULL ORDER BY s.file, s.id",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|error| error.to_string())?;
            rows.filter_map(Result::ok)
                .filter(|(_, _, body): &(i64, String, String)| super::text::has_words(body))
                .collect()
        };
        if !pending.is_empty() {
            let total = pending.len();
            self.service
                .begin_indexing("Understanding passages…", total);
            for (index, batch) in pending.chunks(BACKFILL_BATCH).enumerate() {
                if self.service.stopping() {
                    return Ok(());
                }
                // Grouped by file, so each passage gets its own file's name.
                let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(batch.len());
                let mut failed = false;
                for group in batch.chunk_by(|a, b| a.1 == b.1) {
                    let texts: Vec<String> =
                        group.iter().map(|(_, _, body)| body.clone()).collect();
                    match model.embed_documents(&title(Path::new(&group[0].1)), &texts) {
                        Some(group_vectors) => vectors.extend(group_vectors),
                        None => {
                            failed = true;
                            break;
                        }
                    }
                }
                if failed {
                    continue;
                }
                let tx = conn.transaction().map_err(|error| error.to_string())?;
                {
                    let mut update = tx
                        .prepare_cached("UPDATE segments SET vec = ?1 WHERE id = ?2")
                        .map_err(|error| error.to_string())?;
                    for ((id, _, _), vector) in batch.iter().zip(vectors) {
                        update
                            .execute(rusqlite::params![embed::quantize(&vector), id])
                            .map_err(|error| error.to_string())?;
                    }
                }
                tx.commit().map_err(|error| error.to_string())?;
                self.service
                    .progress(((index + 1) * BACKFILL_BATCH).min(total), total, None);
                if self.last_refresh.elapsed() >= VECTOR_REFRESH {
                    self.service.refresh_vectors(conn);
                    self.last_refresh = Instant::now();
                }
            }
        }
        Ok(())
    }
}

/// Every indexable file under `folder`, walking only where the rules allow.
fn collect(
    folder: &Path,
    is_root: bool,
    config: &Config,
    found: &mut HashMap<String, (rules::Kind, f64, u64)>,
) {
    let walker = walkdir::WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || rules::walks_into(
                    entry.path(),
                    is_root && entry.depth() == 0,
                    config.include_code,
                    &config.excluded,
                )
        });
    for entry in walker.filter_map(Result::ok) {
        if found.len() >= MAX_FILES {
            log::warn!("descriptive find: stopped at {MAX_FILES} files");
            return;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(kind) = rules::classify(entry.path(), config.include_code) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        found.insert(
            entry.path().to_string_lossy().to_string(),
            (kind, mtime(&meta), meta.len()),
        );
    }
}

fn mtime(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// What keyword search knows a file by: its name, split into words, and its
/// folder's name.
fn name_text(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let folder = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    format!(
        "{} {} {}",
        stem.replace(['_', '-', '.'], " "),
        split_camel(stem),
        folder
    )
}

/// "QuarterlyReport" → "Quarterly Report", so either word finds it.
fn split_camel(word: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in word.chars() {
        if c.is_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_lowercase();
        out.push(c);
    }
    out
}

/// A file's name as a title for the model: its stem, separators as spaces.
fn title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .replace(['_', '-'], " ")
}

/// Vectors for a file's passages. The model sees the file's name too, since
/// a name often says what a document is about. Passages that are mostly
/// numbers get keyword search only.
fn embed_passages(
    model: Option<&Engine>,
    path: &Path,
    passages: Vec<extract::Passage>,
) -> Vec<NewSegment> {
    let vectors = model.and_then(|model| {
        let texts: Vec<String> = passages
            .iter()
            .map(|passage| passage.text.clone())
            .collect();
        model.embed_documents(&title(path), &texts)
    });
    passages
        .into_iter()
        .enumerate()
        .map(|(index, passage)| {
            let vec = vectors
                .as_ref()
                .filter(|_| super::text::has_words(&passage.text))
                .and_then(|vectors| vectors.get(index))
                .map(|vector| embed::quantize(vector));
            NewSegment {
                text: passage.text,
                loc: passage.loc,
                vec,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_split_into_words() {
        let text = name_text(Path::new("/docs/Taxes/QuarterlyReport_2024-final.pdf"));
        assert!(text.contains("QuarterlyReport 2024 final"));
        assert!(text.contains("Quarterly Report"));
        assert!(text.ends_with("Taxes"));
    }
}

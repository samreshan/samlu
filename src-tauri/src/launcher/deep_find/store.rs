//! One SQLite file holds the index, laid out like DigUp's: a row per file, a
//! row per passage (with its meaning vector), and an FTS5 table of file
//! names and passage text for exact words. The indexer writes through its
//! own connection and searches read through another (WAL lets both run at
//! once).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub const FILE_NAME: &str = "index.sqlite";
/// Bumped when the layout changes; an older index is rebuilt.
const SCHEMA_VERSION: &str = "1";

pub fn open(dir: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let path = dir.join(FILE_NAME);
    let mut conn = Connection::open(&path).map_err(|error| error.to_string())?;
    configure(&conn)?;
    if meta(&conn, "schema")?.as_deref() != Some(SCHEMA_VERSION) {
        reset(&mut conn)?;
    }
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), String> {
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);",
    )
    .map_err(|error| error.to_string())
}

/// Drops everything and starts an empty index.
pub fn reset(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS fts;
         DROP TABLE IF EXISTS segments;
         DROP TABLE IF EXISTS files;
         DELETE FROM meta;
         CREATE TABLE files(
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             kind TEXT NOT NULL,
             mtime REAL NOT NULL,
             size INTEGER NOT NULL,
             error TEXT
         );
         CREATE TABLE segments(
             id INTEGER PRIMARY KEY,
             file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
             kind TEXT NOT NULL,
             loc INTEGER,
             vec BLOB
         );
         CREATE INDEX segments_file ON segments(file);
         CREATE VIRTUAL TABLE fts USING fts5(
             name, body, tokenize = 'porter unicode61 remove_diacritics 2'
         );",
    )
    .map_err(|error| error.to_string())?;
    set_meta(conn, "schema", SCHEMA_VERSION)
}

pub fn meta(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
        row.get(0)
    })
    .optional()
    .map_err(|error| error.to_string())
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// A passage ready to store: its text, its place, and its vector if the
/// model was there to make one.
pub struct NewSegment {
    pub text: String,
    pub loc: Option<u32>,
    pub vec: Option<Vec<u8>>,
}

/// Replaces whatever the index held for `path`. Every file has a "name"
/// segment, so a file whose text can't be read is still found by its name.
#[allow(clippy::too_many_arguments)]
pub fn put_file(
    conn: &Connection,
    path: &str,
    kind: &str,
    mtime: f64,
    size: u64,
    name_text: &str,
    segments: &[NewSegment],
    error: Option<&str>,
) -> Result<(), String> {
    remove_file(conn, path)?;
    conn.execute(
        "INSERT INTO files(path, kind, mtime, size, error) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![path, kind, mtime, size as i64, error],
    )
    .map_err(|error| error.to_string())?;
    let file = conn.last_insert_rowid();
    let mut segment = conn
        .prepare_cached("INSERT INTO segments(file, kind, loc, vec) VALUES (?1, ?2, ?3, ?4)")
        .map_err(|error| error.to_string())?;
    let mut fts = conn
        .prepare_cached("INSERT INTO fts(rowid, name, body) VALUES (?1, ?2, ?3)")
        .map_err(|error| error.to_string())?;
    segment
        .execute(params![
            file,
            "name",
            Option::<i64>::None,
            Option::<Vec<u8>>::None
        ])
        .map_err(|error| error.to_string())?;
    fts.execute(params![conn.last_insert_rowid(), name_text, ""])
        .map_err(|error| error.to_string())?;
    for passage in segments {
        segment
            .execute(params![
                file,
                "chunk",
                passage.loc.map(i64::from),
                passage.vec
            ])
            .map_err(|error| error.to_string())?;
        fts.execute(params![conn.last_insert_rowid(), "", passage.text])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn remove_file(conn: &Connection, path: &str) -> Result<(), String> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM files WHERE path = ?", [path], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(id) = id {
        conn.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM segments WHERE file = ?)",
            [id],
        )
        .map_err(|error| error.to_string())?;
        conn.execute("DELETE FROM segments WHERE file = ?", [id])
            .map_err(|error| error.to_string())?;
        conn.execute("DELETE FROM files WHERE id = ?", [id])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Every indexed file with what it looked like when it was read, so a sync
/// reads again only what changed.
pub fn known_files(conn: &Connection) -> Result<Vec<(String, f64, u64)>, String> {
    let mut statement = conn
        .prepare("SELECT path, mtime, size FROM files")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, i64>(2)? as u64,
            ))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| error.to_string())
}

pub fn counts(conn: &Connection) -> (usize, usize) {
    let files = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
        .unwrap_or(0);
    let passages = conn
        .query_row(
            "SELECT COUNT(*) FROM segments WHERE kind = 'chunk'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    (files as usize, passages as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_file_leaves_no_stale_passages() {
        let dir = std::env::temp_dir().join(format!("samlu-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = open(&dir).unwrap();
        let passage = |text: &str| NewSegment {
            text: text.into(),
            loc: Some(1),
            vec: None,
        };
        put_file(
            &conn,
            "/a/lease.txt",
            "doc",
            1.0,
            10,
            "lease",
            &[passage("pets allowed")],
            None,
        )
        .unwrap();
        put_file(
            &conn,
            "/a/lease.txt",
            "doc",
            2.0,
            12,
            "lease",
            &[passage("no smoking")],
            None,
        )
        .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts WHERE fts MATCH 'pets'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hits, 0);
        assert_eq!(counts(&conn), (1, 1));
        remove_file(&conn, "/a/lease.txt").unwrap();
        assert_eq!(counts(&conn), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

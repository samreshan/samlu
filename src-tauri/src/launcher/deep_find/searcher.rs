//! Hybrid search, after DigUp's `Searcher`: meaning (cosine similarity with
//! every passage vector, put on one scale per query) plus words (FTS5 with
//! Porter stemming over file names and passage text).
//!
//! Only passages whose meaning stands out from the rest for this query count
//! as meaning matches (`Z_GATE` standard deviations), which also gives an
//! honest "nothing found". A file qualifies by standing out in meaning or by
//! containing at least half of the query's words; words rescue what a vector
//! can't hold exactly, such as invoice numbers, error codes and names.

use super::embed;
use super::text;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

/// How far above the query's average a passage must score to count.
pub const Z_GATE: f64 = 2.0;
/// How much finding the query's words counts against meaning.
const WORD_WEIGHT: f64 = 2.0;
/// Results scoring under this share of the top one are "weaker matches".
const STRONG_RATIO: f64 = 0.5;

/// Every passage vector, one byte a dimension, row-major, held in memory so a
/// search never touches the disk for meaning.
#[derive(Default)]
pub struct Vectors {
    dim: usize,
    data: Vec<i8>,
    scales: Vec<f32>,
    segment: Vec<i64>,
    file: Vec<i64>,
}

impl Vectors {
    pub fn load(conn: &Connection) -> Result<Self, String> {
        let mut vectors = Vectors::default();
        let mut statement = conn
            .prepare("SELECT id, file, vec FROM segments WHERE vec IS NOT NULL ORDER BY id")
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let blob: Vec<u8> = row.get(2).map_err(|error| error.to_string())?;
            let Some((scale, bytes)) = embed::dequantize_parts(&blob) else {
                continue;
            };
            if vectors.dim == 0 {
                vectors.dim = bytes.len();
            }
            if bytes.len() != vectors.dim {
                continue;
            }
            vectors.data.extend(bytes.iter().map(|&byte| byte as i8));
            vectors.scales.push(scale);
            vectors
                .segment
                .push(row.get(0).map_err(|error| error.to_string())?);
            vectors
                .file
                .push(row.get(1).map_err(|error| error.to_string())?);
        }
        Ok(vectors)
    }

    pub fn len(&self) -> usize {
        self.scales.len()
    }

    /// Cosine similarity of `query` (a unit vector) with every row, in
    /// integer arithmetic: the query is quantized the same way the rows are.
    fn scores(&self, query: &[f32]) -> Vec<f32> {
        let max = query.iter().fold(0f32, |max, value| max.max(value.abs()));
        let query_scale = if max > 0.0 { max / 127.0 } else { 1.0 };
        let q: Vec<i32> = query
            .iter()
            .map(|value| (value / query_scale).round().clamp(-127.0, 127.0) as i32)
            .collect();
        self.data
            .chunks_exact(self.dim)
            .zip(&self.scales)
            .map(|(row, &scale)| {
                let dot: i32 = row.iter().zip(&q).map(|(&a, &b)| a as i32 * b).sum();
                dot as f32 * scale * query_scale
            })
            .collect()
    }
}

/// Each score in standard deviations above this query's average. The floor
/// on the deviation keeps a tiny or uniform index from inflating noise.
fn standardized(scores: &[f32]) -> Vec<f64> {
    let count = scores.len().max(1) as f64;
    let mean = scores.iter().map(|&s| s as f64).sum::<f64>() / count;
    let variance = scores
        .iter()
        .map(|&s| (s as f64 - mean).powi(2))
        .sum::<f64>()
        / count;
    let deviation = variance.sqrt().max(0.02);
    scores
        .iter()
        .map(|&s| (s as f64 - mean) / deviation)
        .collect()
}

pub struct Hit {
    pub path: String,
    pub kind: String,
    pub score: f64,
    /// The line (text, code) or page (PDF) the hit points at.
    pub loc: Option<i64>,
    /// One line of the passage, the query's words marked « ».
    pub snippet: Option<String>,
}

pub struct Found {
    pub hits: Vec<Hit>,
    /// How many of `hits` (best first) are strong; the rest trail far behind.
    pub strong: usize,
}

pub fn search(
    conn: &Connection,
    vectors: &Vectors,
    query: &str,
    query_vector: Option<&[f32]>,
    prefix_last: bool,
    limit: usize,
) -> Result<Found, String> {
    let sql = |error: rusqlite::Error| error.to_string();

    // Meaning: score every vector on this query's scale, keep each file's
    // best passage.
    let mut best: HashMap<i64, (f64, usize)> = HashMap::new();
    if let Some(query_vector) = query_vector.filter(|q| q.len() == vectors.dim && vectors.len() > 0)
    {
        let z = standardized(&vectors.scores(query_vector));
        for (row, &value) in z.iter().enumerate() {
            let file = vectors.file[row];
            if best.get(&file).is_none_or(|&(top, _)| value > top) {
                best.insert(file, (value, row));
            }
        }
    }

    // Words. A word that is in no file as typed matches as the start of
    // longer words, as the last word does while it is being typed; a word
    // that is in the index stays whole, so "cat" doesn't bring in "category".
    let terms = text::terms(query);
    let mut patterns = text::patterns(&terms, prefix_last);
    for (index, term) in terms.iter().enumerate() {
        if term.chars().count() >= 3
            && !patterns[index].ends_with('*')
            && !is_indexed(conn, &patterns[index])?
        {
            patterns[index].push('*');
        }
    }
    let mut words: Vec<(i64, i64, Option<String>)> = Vec::new();
    let mut coverage: HashMap<i64, f64> = HashMap::new();
    if !patterns.is_empty() {
        let mut statement = conn
            .prepare_cached(
                "SELECT s.file, s.id, snippet(fts, 1, '«', '»', '…', 14), s.kind
                 FROM fts JOIN segments s ON s.id = fts.rowid
                 WHERE fts MATCH ?1 ORDER BY bm25(fts, 3.0, 1.0) LIMIT 1000",
            )
            .map_err(sql)?;
        let mut rows = statement.query([patterns.join(" OR ")]).map_err(sql)?;
        let mut seen = HashSet::new();
        while let Some(row) = rows.next().map_err(sql)? {
            let file: i64 = row.get(0).map_err(sql)?;
            if words.len() >= 100 || !seen.insert(file) {
                continue;
            }
            let kind: String = row.get(3).map_err(sql)?;
            let snippet: Option<String> = row.get(2).map_err(sql)?;
            let snippet = snippet.filter(|s| kind == "chunk" && s.contains('«'));
            words.push((file, row.get(1).map_err(sql)?, snippet));
        }
        if !words.is_empty() {
            let files = words
                .iter()
                .map(|w| w.0.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let query = format!(
                "SELECT DISTINCT s.file FROM fts JOIN segments s ON s.id = fts.rowid
                 WHERE fts MATCH ?1 AND s.file IN ({files})"
            );
            let mut statement = conn.prepare(&query).map_err(sql)?;
            for pattern in &patterns {
                let matched = statement
                    .query_map([pattern], |row| row.get::<_, i64>(0))
                    .map_err(sql)?;
                for file in matched {
                    *coverage.entry(file.map_err(sql)?).or_default() += 1.0 / patterns.len() as f64;
                }
            }
        }
    }

    // Score: meaning z + a bonus that grows with word coverage, + 1 for the
    // kind the query asks for. One common word can't beat a strong meaning
    // match; all the words of an exact lookup usually do.
    let (meaning_weight, words_weight) = if text::is_lookup(&terms) {
        (0.5, WORD_WEIGHT * 2.0)
    } else {
        (1.0, WORD_WEIGHT)
    };
    let mut candidates: HashSet<i64> = best
        .iter()
        .filter(|(_, &(z, _))| z >= Z_GATE)
        .map(|(&file, _)| file)
        .collect();
    candidates.extend(
        coverage
            .iter()
            .filter(|(_, &share)| share >= 0.5)
            .map(|(&file, _)| file),
    );
    if candidates.is_empty() {
        return Ok(Found {
            hits: Vec::new(),
            strong: 0,
        });
    }

    let mut files: HashMap<i64, (String, String)> = HashMap::new();
    {
        let ids = candidates
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = conn
            .prepare(&format!(
                "SELECT id, path, kind FROM files WHERE id IN ({ids})"
            ))
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(sql)?;
        for row in rows {
            let (id, path, kind) = row.map_err(sql)?;
            files.insert(id, (path, kind));
        }
    }
    let hinted = text::kind_hints(query);
    let mut scored: Vec<(i64, f64)> = candidates
        .iter()
        .filter(|file| files.contains_key(file))
        .map(|&file| {
            let z = best.get(&file).map_or(0.0, |&(z, _)| z.max(0.0));
            let share = coverage.get(&file).copied().unwrap_or(0.0);
            let mut score = meaning_weight * z + words_weight * share.powf(1.5);
            if hinted.contains(files[&file].1.as_str()) {
                score += 1.0;
            }
            (file, score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(limit);

    let keyword: HashMap<i64, (i64, Option<String>)> = words
        .into_iter()
        .map(|(file, segment, snippet)| (file, (segment, snippet)))
        .collect();
    let mut passage = conn
        .prepare_cached(
            "SELECT s.kind, s.loc, f.body FROM segments s JOIN fts f ON f.rowid = s.id WHERE s.id = ?1",
        )
        .map_err(sql)?;
    let mut hits = Vec::with_capacity(scored.len());
    for (file, score) in scored {
        let semantic = best
            .get(&file)
            .filter(|&&(z, _)| z >= Z_GATE || !keyword.contains_key(&file));
        // Point at the best meaning passage if it stands out; else at the
        // passage the words matched.
        let segment = match (semantic, keyword.get(&file)) {
            (Some(&(z, row)), _) if z >= Z_GATE => vectors.segment[row],
            (_, Some((segment, _))) => *segment,
            (Some(&(_, row)), None) => vectors.segment[row],
            (None, None) => continue,
        };
        let (kind, loc, body): (String, Option<i64>, String) = passage
            .query_row(params![segment], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(sql)?;
        // Quote the matched words only when they come from the passage we
        // point at, so a hit on page 5 never shows page 2's words.
        let snippet = keyword
            .get(&file)
            .filter(|(matched, _)| *matched == segment)
            .and_then(|(_, snippet)| snippet.clone())
            .or_else(|| {
                (kind == "chunk").then(|| {
                    text::snippet(&body, &terms, 14).unwrap_or_else(|| text::excerpt(&body, 140))
                })
            })
            .map(|s| text::excerpt(&s, 180));
        let (path, file_kind) = files.remove(&file).unwrap_or_default();
        hits.push(Hit {
            path,
            kind: file_kind,
            score,
            loc: if kind == "chunk" { loc } else { None },
            snippet,
        });
    }
    let strong = match hits.first().map(|hit| hit.score) {
        Some(top) if top > 0.0 => hits
            .iter()
            .take_while(|hit| hit.score >= top * STRONG_RATIO)
            .count(),
        _ => hits.len(),
    };
    Ok(Found { hits, strong })
}

fn is_indexed(conn: &Connection, pattern: &str) -> Result<bool, String> {
    let mut statement = conn
        .prepare_cached("SELECT 1 FROM fts WHERE fts MATCH ?1 LIMIT 1")
        .map_err(|error| error.to_string())?;
    statement
        .exists([pattern])
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::store::{self, NewSegment};
    use super::*;

    fn unit(seed: &[f32]) -> Vec<f32> {
        let norm = seed.iter().map(|v| v * v).sum::<f32>().sqrt();
        seed.iter().map(|v| v / norm).collect()
    }

    /// A small index with hand-made 4-dimensional "meanings", plus filler
    /// passages so the query's scale has something to stand out from.
    fn fixture() -> (Connection, Vectors, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "samlu-search-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = store::open(&dir).unwrap();
        let seg = |text: &str, v: &[f32]| NewSegment {
            text: text.into(),
            loc: Some(3),
            vec: Some(embed::quantize(&unit(v))),
        };
        store::put_file(
            &conn,
            "/d/lease.pdf",
            "pdf",
            1.0,
            1,
            "lease",
            &[seg(
                "The tenant may keep one cat with consent.",
                &[1.0, 0.0, 0.0, 0.0],
            )],
            None,
        )
        .unwrap();
        store::put_file(
            &conn,
            "/d/invoice.txt",
            "doc",
            1.0,
            1,
            "invoice",
            &[seg(
                "Invoice INV-48151 for consulting.",
                &[0.0, 1.0, 0.0, 0.0],
            )],
            None,
        )
        .unwrap();
        for n in 0..40 {
            let v = [0.1, 0.1, 1.0, (n % 7) as f32 / 7.0];
            store::put_file(
                &conn,
                &format!("/d/filler{n}.md"),
                "doc",
                1.0,
                1,
                "filler",
                &[seg("weather report rainy afternoon", &v)],
                None,
            )
            .unwrap();
        }
        let vectors = Vectors::load(&conn).unwrap();
        (conn, vectors, dir)
    }

    #[test]
    fn meaning_finds_a_passage_without_shared_words() {
        let (conn, vectors, dir) = fixture();
        let query = unit(&[0.9, 0.1, 0.0, 0.0]);
        let found = search(
            &conn,
            &vectors,
            "animals allowed in the flat",
            Some(&query),
            false,
            10,
        )
        .unwrap();
        assert_eq!(found.hits[0].path, "/d/lease.pdf");
        assert_eq!(found.hits[0].loc, Some(3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exact_codes_are_found_by_words() {
        let (conn, vectors, dir) = fixture();
        let found = search(&conn, &vectors, "INV-48151", None, false, 10).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].path, "/d/invoice.txt");
        assert!(found.hits[0].snippet.as_deref().unwrap().contains('«'));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn last_word_matches_as_a_prefix_while_typing() {
        let (conn, vectors, dir) = fixture();
        let found = search(&conn, &vectors, "consul", None, true, 10).unwrap();
        assert_eq!(found.hits[0].path, "/d/invoice.txt");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_found_is_honest() {
        let (conn, vectors, dir) = fixture();
        let query = unit(&[0.1, 0.1, 1.0, 0.4]);
        let found = search(&conn, &vectors, "zebra migration", Some(&query), false, 10).unwrap();
        assert!(
            found.hits.is_empty(),
            "got {:?}",
            found.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Descriptions against the real model. Opt-in, since it needs the model on
/// disk: `SAMLU_FIND_MODEL=<dir with the model files> cargo test --release
/// real_model -- --ignored --nocapture`.
#[cfg(test)]
mod real_model {
    use super::super::store::{self, NewSegment};
    use super::*;
    use std::time::Instant;

    const DOCS: &[(&str, &str)] = &[
        ("lease.txt", "Residential tenancy agreement. The tenant may keep one domestic cat or small dog with the landlord's written consent. Rent is due on the first of each month. The deposit is returned within 30 days after the tenancy ends."),
        ("invoice.txt", "Invoice INV-48151. Billed to Northwind Design Studio for brand identity work: logo, typography and colour palette. Payment terms net 30. Total due 4,200 USD."),
        ("recipe.md", "Grandma's banana bread. Mash three ripe bananas, mix with melted butter, sugar, an egg and flour. Bake at 175 degrees for an hour until a skewer comes out clean."),
        ("trip.md", "Packing list for the hiking trip to the Alps: waterproof jacket, boots, headlamp, first aid kit, water filter, and the train tickets to Zermatt."),
        ("standup.md", "Meeting notes: the database migration slipped to next sprint because the backup restore test failed. Action item: Priya to fix the replication lag alerts."),
        ("medical.txt", "Discharge summary. The patient was treated for a fractured left wrist after a cycling accident. Cast for six weeks, follow-up x-ray scheduled."),
        ("resume.md", "Senior software engineer with eight years of experience building distributed systems in Rust and Go. Led the payments platform team."),
        ("car.txt", "Vehicle service record: oil change, new brake pads on the front axle, tyre rotation. Next inspection due at 60,000 km."),
    ];
    const QUERIES: &[(&str, &str)] = &[
        ("can I have a pet in my apartment", "lease.txt"),
        ("bill from the branding agency", "invoice.txt"),
        ("how to bake a loaf with bananas", "recipe.md"),
        ("what to bring on the mountain trek", "trip.md"),
        ("why was the db move delayed", "standup.md"),
        ("broken arm hospital paperwork", "medical.txt"),
        ("my CV", "resume.md"),
        ("when did the brakes get replaced", "car.txt"),
        ("INV-48151", "invoice.txt"),
    ];

    /// Indexes the documents with `engine`, runs the queries, and returns how
    /// many found their document first.
    fn evaluate(engine: &super::super::engine::Engine) -> usize {
        let index = std::env::temp_dir().join(format!(
            "samlu-real-{}-{:?}",
            std::process::id(),
            engine.choice()
        ));
        let _ = std::fs::remove_dir_all(&index);
        let conn = store::open(&index).unwrap();
        // Filler, so each query's scale has something to stand out from, as
        // in a real folder.
        let filler: Vec<String> = (0..300)
            .map(|n| format!("Weekly status report number {n}. Work continued as planned across the usual projects; no notable changes this week."))
            .collect();
        let mut all: Vec<(String, String)> = DOCS
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        all.extend(
            filler
                .iter()
                .enumerate()
                .map(|(n, t)| (format!("report{n}.md"), t.clone())),
        );
        let started = Instant::now();
        for (name, body) in &all {
            let title = name.split('.').next().unwrap();
            let vector = engine
                .embed_documents(title, std::slice::from_ref(body))
                .unwrap()
                .remove(0);
            let segment = NewSegment {
                text: body.clone(),
                loc: Some(1),
                vec: Some(embed::quantize(&vector)),
            };
            store::put_file(
                &conn,
                &format!("/d/{name}"),
                "doc",
                1.0,
                1,
                name,
                &[segment],
                None,
            )
            .unwrap();
        }
        println!(
            "{:?}: indexed {} passages in {:?}",
            engine.choice(),
            all.len(),
            started.elapsed()
        );
        let vectors = Vectors::load(&conn).unwrap();
        let mut first = 0;
        for (query, expected) in QUERIES {
            let started = Instant::now();
            let vector = engine.embed_query(query).unwrap();
            let embedded = started.elapsed();
            let found = search(&conn, &vectors, query, Some(&vector), false, 5).unwrap();
            let top = found
                .hits
                .first()
                .map(|hit| hit.path.trim_start_matches("/d/"))
                .unwrap_or("-");
            println!(
                "{:<40} → {:<12} (embed {:?}, total {:?}, {} hits)",
                query,
                top,
                embedded,
                started.elapsed(),
                found.hits.len()
            );
            if top == *expected {
                first += 1;
            }
        }
        println!("{first} of {} found first", QUERIES.len());
        let _ = std::fs::remove_dir_all(&index);
        first
    }

    /// `SAMLU_FIND_MODEL=<dir with the Model2Vec files>`
    #[test]
    #[ignore]
    fn descriptions_find_their_documents() {
        let dir =
            std::path::PathBuf::from(std::env::var("SAMLU_FIND_MODEL").expect("SAMLU_FIND_MODEL"));
        let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
        let model = model2vec_rs::model::StaticModel::from_bytes(
            read("tokenizer.json"),
            read("model.safetensors"),
            read("config.json"),
            Some(true),
        )
        .unwrap();
        let first = evaluate(&super::super::engine::Engine::Small(std::sync::Arc::new(
            model,
        )));
        assert!(
            first * 10 >= QUERIES.len() * 7,
            "only {first} of {} found first",
            QUERIES.len()
        );
    }

    /// `SAMLU_GEMMA_ROOT=<a Descriptive Find folder with EmbeddingGemma 2 and
    /// llama.cpp downloaded>`: starts the helper as the app does.
    #[test]
    #[ignore]
    fn embeddinggemma_finds_their_documents() {
        let root =
            std::path::PathBuf::from(std::env::var("SAMLU_GEMMA_ROOT").expect("SAMLU_GEMMA_ROOT"));
        let started = Instant::now();
        let server = super::super::gemma::Server::start(&root).unwrap();
        println!("llama.cpp ready in {:?}", started.elapsed());
        let first = evaluate(&super::super::engine::Engine::Large(std::sync::Arc::new(
            server,
        )));
        assert!(
            first * 10 >= QUERIES.len() * 8,
            "only {first} of {} found first",
            QUERIES.len()
        );
    }
}

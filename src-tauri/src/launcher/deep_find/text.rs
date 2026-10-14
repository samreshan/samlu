//! Text the way Descriptive Find keeps and matches it: normalized query
//! words, FTS5 patterns, passages cut from a document, and the one-line
//! excerpts shown under a result. The rules follow DigUp's `SearchText` and
//! `Searcher` helpers so a word is found however it was typed or extracted.

use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

/// About 450 tokens a passage, overlapping a little so a sentence cut at a
/// boundary is whole in one of the two.
pub const CHUNK_CHARS: usize = 1800;
pub const CHUNK_OVERLAP: usize = 200;

/// NFKC (ligatures and full-width forms become plain letters), invisible
/// formatting characters dropped, and Arabic the way people type it: no
/// tatweel, no short-vowel marks, one alef.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.nfkc() {
        match c as u32 {
            0x200B => out.push(' '),
            0x00AD
            | 0x061C
            | 0x200C..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x2069
            | 0xFEFF => {}
            0x0640 | 0x064B..=0x065F | 0x0670 => {}
            0x0622 | 0x0623 | 0x0625 | 0x0671 => out.push('\u{0627}'),
            _ => out.push(c),
        }
    }
    out
}

/// Words that describe what to look for rather than what is in it.
const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "in",
    "on",
    "at",
    "to",
    "for",
    "with",
    "and",
    "or",
    "my",
    "me",
    "that",
    "this",
    "these",
    "those",
    "from",
    "about",
    "where",
    "what",
    "which",
    "who",
    "when",
    "is",
    "was",
    "are",
    "were",
    "be",
    "it",
    "its",
    "some",
    "any",
    "find",
    "show",
    "file",
    "files",
    "document",
    "documents",
    "pdf",
    "note",
    "notes",
    "code",
];

/// The content words of a query, normalized and lowercased. Words of two
/// characters or more count; stopwords and kind words are dropped.
pub fn terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    normalize(query)
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2 && !STOPWORDS.contains(word))
        .filter(|word| seen.insert(word.to_string()))
        .map(str::to_string)
        .collect()
}

/// Scripts that attach endings to words (Bengali, Hindi, Arabic) match as
/// prefixes, so a stem finds its inflected forms.
pub fn inflects(word: &str) -> bool {
    word.chars()
        .any(|c| matches!(c as u32, 0x0900..=0x0DFF | 0x0600..=0x06FF))
}

/// FTS5 patterns for the query's words: quoted, and as prefixes for the last
/// word while it is being typed.
pub fn patterns(terms: &[String], prefix_last: bool) -> Vec<String> {
    terms
        .iter()
        .enumerate()
        .map(|(index, term)| {
            let prefix = (prefix_last && index + 1 == terms.len()) || inflects(term);
            format!(
                "\"{}\"{}",
                term.replace('"', ""),
                if prefix { "*" } else { "" }
            )
        })
        .collect()
}

/// Codes, IDs and long numbers ("K7Q2LM", "INV-48151") mean nothing to an
/// embedding and everything to keyword search, so a query made only of them
/// weighs words double and meaning half.
pub fn is_lookup(terms: &[String]) -> bool {
    !terms.is_empty()
        && terms.iter().all(|term| {
            term.chars().any(|c| c.is_numeric())
                && (term.chars().any(|c| c.is_alphabetic()) || term.chars().count() >= 4)
        })
}

/// Whether text says something in words: at least a fifth of what isn't
/// space is letters. A table of numbers gets keyword search only, since a
/// meaning vector of digits matches nothing anyone would describe.
pub fn has_words(text: &str) -> bool {
    let (mut letters, mut others) = (0usize, 0usize);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        if c.is_alphabetic() {
            letters += 1;
        } else {
            others += 1;
        }
    }
    letters > 0 && letters * 4 >= others
}

/// A passage of a document and the line it starts on (1-based).
pub struct Chunk {
    pub text: String,
    pub line: usize,
}

/// Splits text into ~`CHUNK_CHARS`-character passages that overlap by
/// `CHUNK_OVERLAP`, breaking at whitespace, each with the line it starts on.
pub fn chunks(text: &str, limit: usize) -> Vec<Chunk> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    // Line numbers are counted forward from where the last passage started.
    let mut line = 1usize;
    let mut line_at = 0usize;
    while start < chars.len() && out.len() < limit {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            if let Some(space) = (start..end).rev().find(|&i| chars[i].is_whitespace()) {
                if space - start > CHUNK_CHARS / 2 {
                    end = space;
                }
            }
        }
        line += chars[line_at..start].iter().filter(|&&c| c == '\n').count();
        line_at = start;
        // The passage's first line is the first one with something on it.
        let leading = chars[start..end]
            .iter()
            .take_while(|c| c.is_whitespace())
            .filter(|&&c| c == '\n')
            .count();
        let passage: String = chars[start..end].iter().collect();
        let passage = passage.trim();
        if !passage.is_empty() {
            out.push(Chunk {
                text: passage.to_string(),
                line: line + leading,
            });
        }
        if end >= chars.len() {
            break;
        }
        let mut next = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
        while next < end && !chars[next].is_whitespace() {
            next += 1;
        }
        start = if next < end { next } else { end };
    }
    out
}

/// One line, at most `limit` characters, for a result's subtitle.
pub fn excerpt(text: &str, limit: usize) -> String {
    let collapsed = text
        .chars()
        .take(limit * 8)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() > limit {
        format!(
            "{}…",
            collapsed.chars().take(limit).collect::<String>().trim_end()
        )
    } else {
        collapsed
    }
}

/// About `words` words of `text` around the first of `terms` it contains,
/// each matched word wrapped in « » the way SQLite's snippet() marks them.
/// `None` when none of the terms is in it.
pub fn snippet(text: &str, terms: &[String], words: usize) -> Option<String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let matches = |token: &str| {
        let folded = normalize(token).to_lowercase();
        folded
            .split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .any(|part| {
                terms.iter().any(|term| {
                    part == term
                        || (part.starts_with(term.as_str())
                            && (inflects(term) || term.chars().count() >= 4))
                })
            })
    };
    let first = tokens.iter().position(|token| matches(token))?;
    let start = first.saturating_sub(words / 3);
    let end = (start + words).min(tokens.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    for (offset, token) in tokens[start..end].iter().enumerate() {
        if offset > 0 {
            out.push(' ');
        }
        if matches(token) {
            out.push('«');
            out.push_str(token);
            out.push('»');
        } else {
            out.push_str(token);
        }
    }
    if end < tokens.len() {
        out.push('…');
    }
    Some(out)
}

/// The kinds a query asks for ("…in a pdf", "notes about…"): a soft boost,
/// never a filter.
pub fn kind_hints(query: &str) -> HashSet<&'static str> {
    normalize(query)
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| match word {
            "pdf" | "pdfs" | "paper" | "papers" | "invoice" | "receipt" => Some("pdf"),
            "document" | "documents" | "doc" | "docs" | "note" | "notes" | "letter" | "essay" => {
                Some("doc")
            }
            "code" | "function" | "class" | "method" | "script" => Some("code"),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_drop_stopwords_and_duplicates() {
        assert_eq!(
            terms("The clause about PETS in the lease, pets"),
            vec!["clause", "pets", "lease"]
        );
    }

    #[test]
    fn last_word_is_a_prefix_while_typing() {
        let words = terms("retry back");
        assert_eq!(patterns(&words, true), vec!["\"retry\"", "\"back\"*"]);
        assert_eq!(patterns(&words, false), vec!["\"retry\"", "\"back\""]);
    }

    #[test]
    fn lookups_are_codes_and_long_numbers() {
        assert!(is_lookup(&terms("K7Q2LM TRX50")));
        assert!(is_lookup(&terms("4815162342")));
        assert!(!is_lookup(&terms("invoice 2024 taxes")));
    }

    #[test]
    fn chunks_overlap_and_track_lines() {
        let text = (1..=400)
            .map(|n| format!("line number {n} says something"))
            .collect::<Vec<_>>()
            .join("\n");
        let pieces = chunks(&text, 100);
        assert!(pieces.len() > 3);
        assert_eq!(pieces[0].line, 1);
        for piece in &pieces {
            let first = piece.text.lines().next().unwrap();
            let number: usize = first
                .split_whitespace()
                .find_map(|word| word.parse().ok())
                .unwrap_or(0);
            // A passage may start mid-line; it still starts on that line.
            if first.starts_with("line number") {
                assert_eq!(number, piece.line);
            }
        }
        assert!(pieces
            .iter()
            .all(|piece| piece.text.chars().count() <= CHUNK_CHARS));
    }

    #[test]
    fn snippet_marks_the_matched_words() {
        let text = "The tenant may keep one cat. Pets other than cats need written consent.";
        let marked = snippet(text, &terms("pets consent"), 8).unwrap();
        assert!(marked.contains("«Pets»"));
        assert!(snippet(text, &terms("zebra"), 8).is_none());
    }

    #[test]
    fn number_dumps_have_no_words() {
        assert!(has_words("The quarterly figures rose by 4%."));
        assert!(!has_words("1,2,3,4,5,6,7,8,9,10,11,12,13,14,15 0.5 0.25"));
    }
}

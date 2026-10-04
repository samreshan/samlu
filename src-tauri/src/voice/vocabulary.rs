//! The user's custom vocabulary, shaped for each engine's biasing mechanism.
// Temporary: Task 4 adds the first consumer and removes this allow.
#![allow(dead_code)]

use std::collections::HashSet;

pub const MAX_TERMS: usize = 200;
/// ElevenLabs rejects longer key terms; the same cap keeps every engine's
/// limits simple.
pub const MAX_TERM_CHARS: usize = 50;
/// Whisper's prompt window is 224 tokens; 600 characters stays inside it
/// for typical Latin-script terms.
const WHISPER_PROMPT_CHARS: usize = 600;

/// Trims and collapses whitespace, drops empty and overlong terms, removes
/// case-insensitive duplicates (first spelling wins), and caps the count.
pub fn normalize(terms: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for term in terms {
        let term = term.split_whitespace().collect::<Vec<_>>().join(" ");
        if term.is_empty() || term.chars().count() > MAX_TERM_CHARS {
            continue;
        }
        if seen.insert(term.to_lowercase()) {
            out.push(term);
        }
        if out.len() == MAX_TERMS {
            break;
        }
    }
    out
}

/// Whole terms, in order, while their `", "`-joined length fits.
pub fn take_within(terms: &[String], max_chars: usize) -> Vec<&str> {
    let mut used = 0;
    let mut kept = Vec::new();
    for term in terms {
        let cost = term.len() + if kept.is_empty() { 0 } else { 2 };
        if used + cost > max_chars {
            break;
        }
        used += cost;
        kept.push(term.as_str());
    }
    kept
}

pub fn whisper_prompt(terms: &[String]) -> Option<String> {
    let kept = take_within(terms, WHISPER_PROMPT_CHARS);
    (!kept.is_empty()).then(|| format!("Glossary: {}.", kept.join(", ")))
}

pub fn llm_instruction(terms: &[String]) -> Option<String> {
    (!terms.is_empty()).then(|| {
        format!(
            "Spell these terms exactly as written when they occur: {}.",
            terms.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn normalize_trims_collapses_and_dedupes() {
        let out = normalize(terms(&["  Samlu ", "", "Claude   Code", "samlu", "Tauri"]));
        assert_eq!(out, terms(&["Samlu", "Claude Code", "Tauri"]));
    }

    #[test]
    fn normalize_drops_overlong_terms_and_caps_count() {
        let long = "x".repeat(MAX_TERM_CHARS + 1);
        let many: Vec<String> = (0..MAX_TERMS + 10).map(|i| format!("term{i}")).collect();
        assert!(normalize(vec![long]).is_empty());
        assert_eq!(normalize(many).len(), MAX_TERMS);
    }

    #[test]
    fn take_within_keeps_whole_terms_in_order() {
        let list = terms(&["alpha", "beta", "gamma"]);
        // "alpha, beta" is 11 characters; adding ", gamma" would exceed 12.
        assert_eq!(take_within(&list, 12), vec!["alpha", "beta"]);
        assert!(take_within(&list, 3).is_empty());
    }

    #[test]
    fn whisper_prompt_fits_the_prompt_window() {
        assert_eq!(whisper_prompt(&[]), None);
        assert_eq!(
            whisper_prompt(&terms(&["Samlu", "Wispr"])).as_deref(),
            Some("Glossary: Samlu, Wispr.")
        );
        let many: Vec<String> = (0..200).map(|i| format!("ProductName{i}")).collect();
        let prompt = whisper_prompt(&many).unwrap();
        assert!(prompt.len() <= WHISPER_PROMPT_CHARS + "Glossary: .".len());
        assert!(prompt.ends_with('.'));
        assert!(!prompt.contains("ProductName199"));
    }

    #[test]
    fn llm_instruction_lists_terms() {
        assert_eq!(llm_instruction(&[]), None);
        assert_eq!(
            llm_instruction(&terms(&["Samlu"])).as_deref(),
            Some("Spell these terms exactly as written when they occur: Samlu.")
        );
    }
}

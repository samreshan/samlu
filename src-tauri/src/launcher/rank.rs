//! How well a typed query matches a launcher title. Scores are tiered so a
//! better *kind* of match always beats a worse one, whatever the item is: an
//! exact name beats a prefix, a prefix beats a word inside the name, and so
//! on down to a loose in-order fuzzy match. Within a tier, shorter and earlier
//! matches rank higher. `search.rs` adds per-kind weights and usage history on
//! top of these numbers.

pub const EXACT: i64 = 1000;
pub const PREFIX: i64 = 900;
pub const WORD_PREFIX: i64 = 780;
pub const ACRONYM: i64 = 720;
pub const SUBSTRING: i64 = 600;
pub const FUZZY: i64 = 450;

/// The weakest score a match can have. Every tier stays above it.
const FLOOR: i64 = 200;

/// Scores `text` against `query`. `None` means it does not match at all.
pub fn score(query: &str, text: &str) -> Option<i64> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    let text_len = lower.chars().count() as i64;
    let query_len = query.chars().count() as i64;
    let extra = (text_len - query_len).max(0);

    if lower == query {
        return Some(EXACT);
    }
    if lower.starts_with(&query) {
        return Some(PREFIX - extra.min(60));
    }

    let words = words(text);
    if let Some(index) = word_prefix(&query, &words) {
        return Some(WORD_PREFIX - (index as i64 * 8).min(40) - (extra / 2).min(40));
    }
    if query_len >= 2 && is_acronym(&query, &words) {
        return Some(ACRONYM - (extra / 4).min(30));
    }
    if query_len >= 2 {
        if let Some(position) = lower.find(&query) {
            let position = lower[..position].chars().count() as i64;
            return Some(SUBSTRING - position.min(40) - (extra / 4).min(30));
        }
    }
    if query_len >= 3 {
        return fuzzy(&query, text);
    }
    None
}

/// Words of a title, lowercased: split on anything that is not a letter or a
/// digit, and on camel-case humps, so "GitHub Desktop" gives "git", "hub",
/// "desktop" and "zoom.us" gives "zoom", "us".
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in text.chars() {
        if !character.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if character.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = character.is_lowercase() || character.is_numeric();
        current.extend(character.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// A single query word that begins some word of the title gives that word's
/// index. Several query words ("vis stu") must each begin a later word, in
/// order, and give the index of the first.
fn word_prefix(query: &str, words: &[String]) -> Option<usize> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    match tokens.as_slice() {
        [] => None,
        [single] => words.iter().position(|word| word.starts_with(single)),
        [first, rest @ ..] => {
            let start = words.iter().position(|word| word.starts_with(first))?;
            let mut cursor = start + 1;
            for token in rest {
                let offset = words[cursor..]
                    .iter()
                    .position(|word| word.starts_with(token))?;
                cursor += offset + 1;
            }
            Some(start)
        }
    }
}

/// "vsc" for "Visual Studio Code": the query spells the start of the title's
/// word initials. Spaces in the query are ignored.
fn is_acronym(query: &str, words: &[String]) -> bool {
    let compact: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    let initials: String = words
        .iter()
        .filter_map(|word| word.chars().next())
        .collect();
    initials.len() >= 2 && initials.starts_with(&compact)
}

/// Character positions where a word of `text` begins, by the same rules as
/// `words`.
fn word_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut previous: Option<char> = None;
    for (index, character) in text.chars().enumerate() {
        if character.is_alphanumeric() {
            let after_separator = !previous.is_some_and(|c| c.is_alphanumeric());
            let camel_hump = character.is_uppercase()
                && previous.is_some_and(|c| c.is_lowercase() || c.is_numeric());
            if after_separator || camel_hump {
                starts.push(index);
            }
        }
        previous = Some(character);
    }
    starts
}

/// Every query character appears in order, starting at the beginning of a
/// word, with no more skipped letters in between than the query is long —
/// "sfr" finds Safari, but "term" does not find "System Information". Of the
/// word starts it could begin at, the tightest run wins.
fn fuzzy(query: &str, text: &str) -> Option<i64> {
    let wanted: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let (first_wanted, rest) = wanted.split_first()?;
    let chars: Vec<char> = text
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let allowed_gaps = wanted.len() as i64;
    word_starts(text)
        .into_iter()
        .filter(|start| chars[*start] == *first_wanted)
        .filter_map(|start| {
            let mut last = start;
            let mut gaps = 0i64;
            for character in rest {
                let found = chars[last + 1..].iter().position(|c| c == character)? + last + 1;
                gaps += (found - last - 1) as i64;
                last = found;
            }
            if gaps > allowed_gaps {
                return None;
            }
            Some((FUZZY - gaps * 12 - start as i64 * 3).max(FLOOR))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(query: &str, text: &str) -> i64 {
        score(query, text).unwrap_or_else(|| panic!("{query:?} should match {text:?}"))
    }

    #[test]
    fn exact_beats_prefix_beats_word_beats_substring() {
        assert_eq!(tier("safari", "Safari"), EXACT);
        assert!(tier("saf", "Safari") > tier("saf", "Open Safari Tab"));
        assert!(tier("set", "Settings Sync") > tier("set", "System Settings"));
        assert!(tier("set", "System Settings") > tier("set", "Reset"));
    }

    #[test]
    fn shorter_titles_win_within_a_tier() {
        assert!(tier("term", "Terminal") > tier("term", "Terminal Pro Max"));
    }

    #[test]
    fn matches_words_acronyms_and_camel_case() {
        assert!(score("studio", "Visual Studio Code").unwrap() >= WORD_PREFIX - 40);
        assert!(score("vis stu", "Visual Studio Code").unwrap() >= WORD_PREFIX - 40);
        assert!(score("vsc", "Visual Studio Code").unwrap() >= ACRONYM - 30);
        assert!(score("hub", "GitHub Desktop").unwrap() >= WORD_PREFIX - 40);
        assert!(score("us", "zoom.us").unwrap() >= WORD_PREFIX - 40);
    }

    #[test]
    fn single_letters_only_match_starts_of_words() {
        assert!(score("s", "Safari").is_some());
        assert!(score("s", "System Settings").is_some());
        assert!(score("s", "Pick Color").is_none());
        assert!(score("x", "Terminal").is_none());
    }

    #[test]
    fn fuzzy_needs_three_letters_and_order() {
        assert!(score("sfr", "Safari").is_some());
        assert!(score("sf", "Safari").is_none());
        assert!(score("irafas", "Safari").is_none());
        assert!(tier("sfr", "Safari") < SUBSTRING);
    }

    #[test]
    fn fuzzy_starts_at_a_word_and_stays_tight() {
        // A run starting mid-word, or strung across the title, is noise.
        assert!(score("term", "System Information").is_none());
        assert!(score("upd", "supistudio-v1").is_none());
        assert!(score("hex", "Bluetooth File Exchange").is_none());
        // A tight run from a word start still counts, on any word.
        assert!(score("chrm", "Google Chrome").is_some());
        assert!(score("stng", "System Settings").is_some());
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(score("", "Safari").is_none());
        assert!(score("   ", "Safari").is_none());
    }
}

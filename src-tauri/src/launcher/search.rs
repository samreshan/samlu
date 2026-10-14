use super::app_index::AppIndex;
use super::calculator;
use super::clipboard::ClipboardHistory;
use super::history::LauncherHistory;
use super::projects::ProjectIndex;
use super::rank;
use super::snippets::{Snippet, SnippetStore};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;

const MAX_RESULTS: usize = 12;
const MAX_FILE_RESULTS: usize = 6;
const MAX_RECENT: usize = 6;
const RAW_SCAN_LIMIT: usize = 60;
const ALLOWED_FILE_EXTENSIONS: &[&str] = &[
    "rs", "swift", "ts", "tsx", "js", "jsx", "py", "go", "md", "json", "toml", "yaml", "yml",
    "pdf", "fig", "psd", "ai",
];
/// Folders whose files are never what someone means to open by name.
const NOISY_PATH_PARTS: &[&str] = &[
    "/Library/",
    "/node_modules/",
    "/target/",
    "/.build/",
    "/dist/",
    "/.Trash/",
];

/// Commands answer to strong matches only — the start of the name, a word in
/// it, or its initials. A loose match on a command is how "Open Samlu
/// settings" used to sit above Safari when "sa" was typed.
const COMMAND_MIN_SCORE: i64 = rank::ACRONYM - 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LauncherResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub target: String,
    /// Heading for the empty-query suggestions. Typed searches are a single
    /// list ordered by relevance, so they never carry one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

struct LauncherCommand {
    target: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static str,
}

const COMMANDS: &[LauncherCommand] = &[
    LauncherCommand {
        target: "open_settings",
        title: "Samlu Settings",
        subtitle: "Agents, voice, launcher and updates",
        keywords: "preferences configure options open settings",
    },
    LauncherCommand {
        target: "color_picker",
        title: "Pick Color",
        subtitle: "Copy a color from the screen as hex",
        keywords: "eyedropper hex colour sample",
    },
    LauncherCommand {
        target: "manage_snippets",
        title: "Manage Snippets",
        subtitle: "Create reusable text for the launcher",
        keywords: "template boilerplate text",
    },
    LauncherCommand {
        target: "open_terminal",
        title: "Open Terminal",
        subtitle: "Bring up a Terminal window",
        keywords: "shell command line console",
    },
    LauncherCommand {
        target: "check_for_updates",
        title: "Check for Updates",
        subtitle: "See whether a newer Samlu is available",
        keywords: "update upgrade version release",
    },
];

/// The ranking weight of each kind of result. Applications lead a tie, and
/// commands sit well below anything installed or indexed, so they surface when
/// they are what was typed rather than whenever a letter happens to match.
fn kind_weight(kind: &str) -> i64 {
    match kind {
        "app" => 40,
        "project" => 20,
        "snippet" => -60,
        "action" => -120,
        _ => 0,
    }
}

/// Something picked often and recently climbs, but only so far: a heavily used
/// app can overtake a slightly better match, never a much better one.
fn usage_boost(frecency: f64) -> i64 {
    (frecency.ln_1p() * 70.0).min(240.0) as i64
}

/// Tie-break order for equal scores.
fn kind_order(kind: &str) -> u8 {
    match kind {
        "app" => 0,
        "project" => 1,
        "file" => 2,
        "snippet" => 3,
        _ => 4,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn search_fast(
    query: &str,
    app_state: &Arc<AppState>,
    history: &LauncherHistory,
    app_index: &AppIndex,
    project_index: &ProjectIndex,
    snippets: &SnippetStore,
    clipboard: &ClipboardHistory,
    clipboard_enabled: bool,
) -> Vec<LauncherResult> {
    let query = query.trim();

    if let Some(expression) = query.strip_prefix('=') {
        return calculator::evaluate(query)
            .map(|value| {
                vec![LauncherResult {
                    id: format!("calculation:{query}"),
                    kind: "calculation".into(),
                    title: value.clone(),
                    subtitle: expression.trim().to_string(),
                    target: value,
                    section: None,
                }]
            })
            .unwrap_or_default();
    }

    if let Some(snippet_query) = query.strip_prefix(';') {
        let snippet_query = snippet_query.trim();
        let scores = history.scores();
        let mut scored: Vec<(i64, LauncherResult)> = snippets
            .list()
            .into_iter()
            .filter_map(|snippet| {
                let score = if snippet_query.is_empty() {
                    0
                } else {
                    snippet_score(snippet_query, &snippet)?
                };
                let result = snippet_result(snippet);
                Some((score + boost(&scores, &result.id), result))
            })
            .collect();
        return finish(&mut scored, MAX_RESULTS);
    }

    if let Some(clipboard_query) = query.strip_prefix('@') {
        if !clipboard_enabled {
            return vec![LauncherResult {
                id: "action:open_settings".into(),
                kind: "action".into(),
                title: "Turn On Clipboard History".into(),
                subtitle: "Clipboard history is opt-in in Launcher settings".into(),
                target: "open_settings".into(),
                section: None,
            }];
        }
        return clipboard
            .search(clipboard_query.trim(), MAX_RESULTS)
            .into_iter()
            .map(|item| LauncherResult {
                id: format!("clipboard:{}", item.id),
                kind: "clipboard".into(),
                title: compact_text(&item.text, 90),
                subtitle: String::new(),
                target: item.text,
                section: None,
            })
            .collect();
    }

    if let Some(command_query) = query.strip_prefix('>') {
        let command_query = command_query.trim();
        let scores = history.scores();
        let mut scored: Vec<(i64, LauncherResult)> = COMMANDS
            .iter()
            .filter_map(|command| {
                let score = if command_query.is_empty() {
                    0
                } else {
                    command_score(command_query, command)?
                };
                let result = command_result(command);
                Some((score + boost(&scores, &result.id), result))
            })
            .collect();
        return finish(&mut scored, MAX_RESULTS);
    }

    if query.is_empty() {
        return suggestions(app_state, history);
    }

    let scores = history.scores();
    let mut scored: Vec<(i64, LauncherResult)> = Vec::new();

    for app in app_index.entries() {
        let alias_score = app
            .aliases
            .iter()
            .filter_map(|alias| rank::score(query, alias))
            .max()
            .map(|score| score - 15);
        let Some(score) = rank::score(query, &app.name).max(alias_score) else {
            continue;
        };
        let result = LauncherResult {
            id: format!("app:{}", app.path),
            kind: "app".into(),
            title: app.name,
            subtitle: String::new(),
            target: app.path,
            section: None,
        };
        scored.push((
            score + kind_weight("app") + boost(&scores, &result.id),
            result,
        ));
    }

    let mut projects: Vec<(String, String)> = project_index
        .entries()
        .into_iter()
        .map(|project| (project.name, project.path))
        .collect();
    projects.extend(recent_agent_projects(app_state));
    for (name, path) in projects {
        let Some(score) = rank::score(query, &name) else {
            continue;
        };
        let result = project_result(name, path);
        scored.push((
            score + kind_weight("project") + boost(&scores, &result.id),
            result,
        ));
    }

    for snippet in snippets.list() {
        let Some(score) = snippet_score(query, &snippet) else {
            continue;
        };
        let result = snippet_result(snippet);
        scored.push((
            score + kind_weight("snippet") + boost(&scores, &result.id),
            result,
        ));
    }

    for command in COMMANDS {
        let Some(score) = command_score(query, command).filter(|s| *s >= COMMAND_MIN_SCORE) else {
            continue;
        };
        let result = command_result(command);
        scored.push((
            score + kind_weight("action") + boost(&scores, &result.id),
            result,
        ));
    }

    finish(&mut scored, MAX_RESULTS)
}

pub fn search_slow(query: &str, history: &LauncherHistory) -> Vec<LauncherResult> {
    let query = query.trim();
    if query.chars().count() < 2
        || query.starts_with('=')
        || query.starts_with(';')
        || query.starts_with('@')
        || query.starts_with('>')
    {
        return Vec::new();
    }
    let scores = history.scores();
    let mut scored: Vec<(i64, LauncherResult)> = search_files(query)
        .into_iter()
        .map(|result| {
            let score = rank::score(query, &result.title).unwrap_or(rank::FUZZY);
            (score + boost(&scores, &result.id), result)
        })
        .collect();
    finish(&mut scored, MAX_FILE_RESULTS)
}

/// What an empty launcher offers: the places you open most, then Samlu's own
/// commands.
fn suggestions(app_state: &Arc<AppState>, history: &LauncherHistory) -> Vec<LauncherResult> {
    let mut seen = HashSet::new();
    let mut results = Vec::new();

    let recent = history
        .recent(MAX_RECENT * 2)
        .into_iter()
        .filter(is_still_available)
        .chain(
            recent_agent_projects(app_state)
                .into_iter()
                .map(|(name, path)| project_result(name, path)),
        );
    for mut item in recent {
        if results.len() >= MAX_RECENT {
            break;
        }
        if seen.insert(item.id.clone()) {
            item.section = Some("Recent".into());
            results.push(item);
        }
    }

    for command in COMMANDS {
        let mut result = command_result(command);
        if seen.insert(result.id.clone()) {
            result.section = Some("Commands".into());
            results.push(result);
        }
    }
    results
}

fn is_still_available(item: &LauncherResult) -> bool {
    match item.kind.as_str() {
        "app" | "file" | "project" => std::path::Path::new(&item.target).exists(),
        "action" => COMMANDS.iter().any(|command| command.target == item.target),
        _ => false,
    }
}

/// Projects agents have recently worked in, newest first. They may sit
/// outside the indexed folders.
fn recent_agent_projects(app_state: &Arc<AppState>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    app_state
        .event_history()
        .into_iter()
        .filter(|event| !event.project_path.is_empty() && seen.insert(event.project_path.clone()))
        .take(4)
        .map(|event| (event.project_label(), event.project_path))
        .collect()
}

fn project_result(name: String, path: String) -> LauncherResult {
    LauncherResult {
        id: format!("project:{path}"),
        kind: "project".into(),
        title: name,
        subtitle: parent_folder(&path),
        target: path,
        section: None,
    }
}

fn command_result(command: &LauncherCommand) -> LauncherResult {
    LauncherResult {
        id: format!("action:{}", command.target),
        kind: "action".into(),
        title: command.title.into(),
        subtitle: command.subtitle.into(),
        target: command.target.into(),
        section: None,
    }
}

/// The title counts in full; a keyword only by its start, so "hex" finds Pick
/// Color but a stray letter inside a keyword finds nothing.
fn command_score(query: &str, command: &LauncherCommand) -> Option<i64> {
    let keyword = command
        .keywords
        .split_whitespace()
        .filter_map(|keyword| rank::score(query, keyword))
        .filter(|score| *score >= rank::PREFIX - 60)
        .max()
        .map(|score| score - 140);
    rank::score(query, command.title).max(keyword)
}

fn snippet_result(snippet: Snippet) -> LauncherResult {
    let preview = compact_text(&snippet.content, 60);
    LauncherResult {
        id: format!("snippet:{}", snippet.id),
        kind: "snippet".into(),
        title: snippet.title,
        subtitle: if snippet.keyword.is_empty() {
            preview
        } else {
            format!(";{}", snippet.keyword)
        },
        target: snippet.content,
        section: None,
    }
}

/// A snippet's keyword is its shortcut, so typing it exactly is as strong as
/// typing the title.
fn snippet_score(query: &str, snippet: &Snippet) -> Option<i64> {
    let keyword = (!snippet.keyword.is_empty())
        .then(|| rank::score(query, &snippet.keyword))
        .flatten();
    rank::score(query, &snippet.title).max(keyword)
}

fn boost(scores: &HashMap<String, f64>, id: &str) -> i64 {
    scores.get(id).copied().map(usage_boost).unwrap_or(0)
}

/// Orders by score, then kind, then the shorter title, and keeps the best
/// entry for any id that turned up twice.
fn finish(scored: &mut Vec<(i64, LauncherResult)>, limit: usize) -> Vec<LauncherResult> {
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| kind_order(&left.kind).cmp(&kind_order(&right.kind)))
            .then_with(|| left.title.len().cmp(&right.title.len()))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    let mut seen = HashSet::new();
    scored
        .drain(..)
        .map(|(_, result)| result)
        .filter(|result| seen.insert(result.id.clone()))
        .take(limit)
        .collect()
}

fn search_files(query: &str) -> Vec<LauncherResult> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let predicate = extension_predicate(query);
    run_mdfind(&[&home.to_string_lossy()], &predicate)
        .into_iter()
        .filter(|path| {
            !NOISY_PATH_PARTS.iter().any(|part| path.contains(part)) && !path.contains("/.")
        })
        .map(|path| LauncherResult {
            id: format!("file:{path}"),
            kind: "file".into(),
            title: path.rsplit('/').next().unwrap_or(&path).to_string(),
            subtitle: parent_folder(&path),
            target: path,
            section: None,
        })
        .collect()
}

fn extension_predicate(query: &str) -> String {
    let query = query.replace(['"', '\\', '*'], "");
    ALLOWED_FILE_EXTENSIONS
        .iter()
        .map(|extension| format!(r#"kMDItemFSName == "*{query}*.{extension}"cd"#))
        .collect::<Vec<_>>()
        .join(" || ")
}

fn run_mdfind(scopes: &[&str], predicate: &str) -> Vec<String> {
    let mut command = Command::new("/usr/bin/mdfind");
    for scope in scopes {
        command.arg("-onlyin").arg(scope);
    }
    command
        .arg(predicate)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return Vec::new();
    };
    let Some(stdout) = child.stdout.take() else {
        return Vec::new();
    };
    let results = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .take(RAW_SCAN_LIMIT)
        .collect();
    let _ = child.kill();
    let _ = child.wait();
    results
}

/// The folder holding `path`, with the home directory written as `~`.
fn parent_folder(path: &str) -> String {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_default();
    match dirs::home_dir() {
        Some(home) => {
            let home = home.to_string_lossy().to_string();
            if parent == home {
                "~".to_string()
            } else if let Some(rest) = parent.strip_prefix(&format!("{home}/")) {
                format!("~/{rest}")
            } else {
                parent
            }
        }
        None => parent,
    }
}

fn compact_text(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = one_line.chars();
    let compact: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", compact.trim_end())
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> LauncherResult {
        LauncherResult {
            id: format!("app:/Applications/{name}.app"),
            kind: "app".into(),
            title: name.into(),
            subtitle: String::new(),
            target: format!("/Applications/{name}.app"),
            section: None,
        }
    }

    /// Scores a typed query the way `search_fast` does for apps and commands,
    /// without needing a running app to supply the indexes.
    fn rank_names(query: &str, apps: &[&str]) -> Vec<String> {
        let mut scored: Vec<(i64, LauncherResult)> = apps
            .iter()
            .filter_map(|name| {
                let score = rank::score(query, name)?;
                Some((score + kind_weight("app"), app(name)))
            })
            .collect();
        for command in COMMANDS {
            if let Some(score) = command_score(query, command).filter(|s| *s >= COMMAND_MIN_SCORE) {
                scored.push((score + kind_weight("action"), command_result(command)));
            }
        }
        finish(&mut scored, MAX_RESULTS)
            .into_iter()
            .map(|result| result.title)
            .collect()
    }

    const APPS: &[&str] = &[
        "Safari",
        "Slack",
        "System Settings",
        "Terminal",
        "Calculator",
        "Calendar",
        "Claude",
        "Visual Studio Code",
        "Spotify",
        "Samlu",
    ];

    #[test]
    fn an_app_name_puts_that_app_first() {
        assert_eq!(rank_names("safari", APPS)[0], "Safari");
        assert_eq!(rank_names("saf", APPS)[0], "Safari");
        assert_eq!(rank_names("term", APPS)[0], "Terminal");
        assert_eq!(rank_names("calc", APPS)[0], "Calculator");
        assert_eq!(rank_names("vsc", APPS)[0], "Visual Studio Code");
        assert_eq!(rank_names("samlu", APPS)[0], "Samlu");
    }

    #[test]
    fn commands_never_outrank_an_app_that_matches_as_well() {
        let terminal = rank_names("terminal", APPS);
        assert_eq!(terminal[0], "Terminal");
        assert!(terminal.contains(&"Open Terminal".to_string()));

        let settings = rank_names("settings", APPS);
        assert_eq!(settings[0], "System Settings");
        assert_eq!(settings[1], "Samlu Settings");
    }

    #[test]
    fn single_letters_list_every_matching_app_before_any_command() {
        let results = rank_names("c", APPS);
        let is_command = |title: &String| COMMANDS.iter().any(|command| command.title == title);
        let first_command = results.iter().position(is_command).unwrap_or(results.len());
        assert!(first_command >= 3, "{results:?}");
        assert!(
            results[first_command..].iter().all(is_command),
            "{results:?}"
        );
    }

    #[test]
    fn a_letter_run_inside_a_command_name_is_not_enough() {
        assert!(!rank_names("lor", APPS).contains(&"Pick Color".to_string()));
        assert!(!rank_names("ett", APPS).contains(&"Samlu Settings".to_string()));
    }

    #[test]
    fn commands_answer_to_their_names_and_keyword_starts() {
        assert_eq!(rank_names("pick", APPS)[0], "Pick Color");
        assert_eq!(rank_names("hex", APPS)[0], "Pick Color");
        assert_eq!(rank_names("updates", APPS)[0], "Check for Updates");
    }

    #[test]
    fn heavy_use_lifts_a_close_match_but_not_a_poor_one() {
        assert!(usage_boost(20.0) > 150);
        assert!(usage_boost(1_000_000.0) <= 240);
        // A much better match still wins against a heavily used weak one.
        let strong = rank::score("cal", "Calendar").unwrap();
        let weak = rank::score("cal", "Musical Notes").unwrap() + usage_boost(1_000_000.0);
        assert!(strong > weak);
    }

    #[test]
    fn parent_folder_abbreviates_home() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(parent_folder(&format!("{home}/Code/samlu")), "~/Code");
        assert_eq!(parent_folder(&format!("{home}/notes.md")), "~");
        assert_eq!(parent_folder("/opt/tools/x"), "/opt/tools");
    }

    #[test]
    fn mdfind_predicate_strips_wildcards_and_quotes() {
        let predicate = extension_predicate(r#"a"b*c\d"#);
        assert!(predicate.contains(r#""*abcd*.rs"cd"#));
    }
}

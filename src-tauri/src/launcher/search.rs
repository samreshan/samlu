use super::app_index::AppIndex;
use super::calculator;
use super::clipboard::ClipboardHistory;
use super::history::LauncherHistory;
use super::projects::ProjectIndex;
use super::snippets::SnippetStore;
use crate::state::AppState;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;

const MAX_RESULTS: usize = 12;
const RAW_SCAN_LIMIT: usize = 40;
const ALLOWED_FILE_EXTENSIONS: &[&str] = &[
    "rs", "swift", "ts", "tsx", "js", "jsx", "py", "go", "md", "json", "toml", "yaml", "yml",
    "pdf", "fig", "psd", "ai",
];

#[derive(Clone, Serialize)]
pub struct LauncherResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub target: String,
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
    if let Some(value) = calculator::evaluate(query) {
        return vec![LauncherResult {
            id: format!("calculation:{query}"),
            kind: "calculation".into(),
            title: value.clone(),
            subtitle: "Copy calculation result".into(),
            target: value,
        }];
    }

    if let Some(snippet_query) = query.strip_prefix(';') {
        return snippets
            .search(snippet_query.trim(), MAX_RESULTS)
            .into_iter()
            .map(snippet_result)
            .collect();
    }

    if let Some(clipboard_query) = query.strip_prefix('@') {
        if !clipboard_enabled {
            return vec![LauncherResult {
                id: "action:open_settings".into(),
                kind: "action".into(),
                title: "Enable clipboard history".into(),
                subtitle: "Clipboard capture is opt-in in Settings".into(),
                target: "open_settings".into(),
            }];
        }
        return clipboard
            .search(clipboard_query.trim(), MAX_RESULTS)
            .into_iter()
            .map(|item| LauncherResult {
                id: format!("clipboard:{}", item.id),
                kind: "clipboard".into(),
                title: compact_text(&item.text, 90),
                subtitle: "Copy from clipboard history".into(),
                target: item.text,
            })
            .collect();
    }

    let command_query = query.strip_prefix('>').map(str::trim).unwrap_or(query);
    let mut results = commands(command_query);
    results.extend(recent_projects(app_state));
    if !query.is_empty() {
        results.extend(
            project_index
                .search(query, 5)
                .into_iter()
                .map(|project| LauncherResult {
                    id: format!("project:{}", project.path),
                    kind: "project".into(),
                    title: project.name,
                    subtitle: "Developer project".into(),
                    target: project.path,
                }),
        );
        results.extend(search_apps(query, app_index));
        results.extend(snippets.search(query, 3).into_iter().map(snippet_result));
    }
    rank_and_truncate(deduplicate(results), history)
}

pub fn search_slow(query: &str, history: &LauncherHistory) -> Vec<LauncherResult> {
    let query = query.trim();
    if query.is_empty()
        || query.starts_with('=')
        || query.starts_with(';')
        || query.starts_with('@')
        || query.starts_with('>')
    {
        return Vec::new();
    }
    rank_and_truncate(search_files(query), history)
}

fn commands(query: &str) -> Vec<LauncherResult> {
    [
        (
            "open_settings",
            "Open Samlu settings",
            "Configure agents, voice, launcher, and privacy",
            "settings preferences configure",
        ),
        (
            "color_picker",
            "Pick color from screen",
            "Copy a pixel color as a hex value",
            "hex eyedropper colour",
        ),
        (
            "open_terminal",
            "Open Terminal",
            "Launch a new Terminal window",
            "shell command line",
        ),
        (
            "manage_snippets",
            "Manage snippets",
            "Create reusable developer text snippets",
            "template boilerplate",
        ),
    ]
    .into_iter()
    .filter(|(_, title, _, keywords)| {
        query.is_empty() || contains_ci(title, query) || contains_ci(keywords, query)
    })
    .map(|(target, title, subtitle, _)| LauncherResult {
        id: format!("action:{target}"),
        kind: "action".into(),
        title: title.into(),
        subtitle: subtitle.into(),
        target: target.into(),
    })
    .collect()
}

fn recent_projects(app_state: &Arc<AppState>) -> Vec<LauncherResult> {
    let mut seen = HashSet::new();
    app_state
        .event_history()
        .into_iter()
        .filter(|event| !event.project_path.is_empty() && seen.insert(event.project_path.clone()))
        .take(4)
        .map(|event| LauncherResult {
            id: format!("project:{}", event.project_path),
            kind: "project".into(),
            title: event.project_label(),
            subtitle: format!("Recent {} project", event.agent_label()),
            target: event.project_path,
        })
        .collect()
}

fn search_apps(query: &str, app_index: &AppIndex) -> Vec<LauncherResult> {
    app_index
        .search(query, 6)
        .into_iter()
        .map(|entry| LauncherResult {
            id: format!("app:{}", entry.path),
            kind: "app".into(),
            title: entry.name,
            subtitle: "Application".into(),
            target: entry.path,
        })
        .collect()
}

fn snippet_result(snippet: super::snippets::Snippet) -> LauncherResult {
    LauncherResult {
        id: format!("snippet:{}", snippet.id),
        kind: "snippet".into(),
        title: snippet.title,
        subtitle: if snippet.keyword.is_empty() {
            "Copy snippet".into()
        } else {
            format!(";{}  Copy snippet", snippet.keyword)
        },
        target: snippet.content,
    }
}

fn search_files(query: &str) -> Vec<LauncherResult> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let predicate = extension_predicate(query);
    run_mdfind(&[&home.to_string_lossy()], &predicate)
        .into_iter()
        .take(7)
        .map(|path| LauncherResult {
            id: format!("file:{path}"),
            kind: "file".into(),
            title: path.rsplit('/').next().unwrap_or(&path).to_string(),
            subtitle: path.clone(),
            target: path,
        })
        .collect()
}

fn extension_predicate(query: &str) -> String {
    let query = query.replace('"', "");
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

fn deduplicate(results: Vec<LauncherResult>) -> Vec<LauncherResult> {
    let mut seen = HashSet::new();
    results
        .into_iter()
        .filter(|result| seen.insert(result.id.clone()))
        .collect()
}

fn rank_and_truncate(
    mut results: Vec<LauncherResult>,
    history: &LauncherHistory,
) -> Vec<LauncherResult> {
    results.sort_by(|left, right| {
        history
            .score(&right.id)
            .partial_cmp(&history.score(&left.id))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(MAX_RESULTS);
    results
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn compact_text(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = one_line.chars();
    let compact: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}...", compact.trim_end())
    } else {
        compact
    }
}

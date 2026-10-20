//! Installs Samlu's lifecycle and permission hooks into
//! `~/.codex/config.toml` (the inline `[[hooks.<Event>]]` form), keeping
//! everything else in the file, comments included.

use super::{codex_notify, InstallState};
use std::path::{Path, PathBuf};
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

/// Codex event name, the Samlu route it posts to, and the hook timeout in
/// seconds. Only the permission hook can be held open, so only it needs the
/// long timeout (Samlu's wait is at most 120 s).
const EVENTS: [(&str, &str, i64); 6] = [
    ("SessionStart", "session-start", 5),
    ("SessionEnd", "session-end", 5),
    ("UserPromptSubmit", "prompt", 5),
    ("PermissionRequest", "permission", 130),
    ("PostToolUse", "post-tool", 5),
    ("Stop", "stop", 5),
];

/// A hook is Samlu's when its command posts to one of our Codex routes.
const SAMLU_MARKER: &str = "/hooks/codex/";

/// Headers that tell Samlu which window the session lives in.
#[cfg(not(windows))]
const ORIGIN_HEADERS: &str = "-H \"X-Samlu-Warp-Focus: $WARP_FOCUS_URL\" -H \"X-Samlu-Term: $TERM_PROGRAM\" -H \"X-Samlu-Bundle: $__CFBundleIdentifier\" -H \"X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE\"";

#[cfg(windows)]
const ORIGIN_HEADERS: &str = "-H \"X-Samlu-Warp-Focus: %WARP_FOCUS_URL%\" -H \"X-Samlu-Term: %TERM_PROGRAM%\" -H \"X-Samlu-Bundle: %__CFBundleIdentifier%\" -H \"X-Samlu-Originator: %CODEX_INTERNAL_ORIGINATOR_OVERRIDE%\"";

/// The permission hook must print Codex's decision body, so only its stderr
/// is silenced; the others discard everything. Every command ends in a
/// success exit so a stopped Samlu never blocks Codex. `--fail` keeps the
/// body of a 400/401/404 reply from reaching Codex's stdout as a decision.
#[cfg(not(windows))]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    let (max_time, redirect) = if route == "permission" {
        (125, "2>/dev/null")
    } else {
        (3, ">/dev/null 2>&1")
    };
    format!(
        "/usr/bin/curl --silent --fail --max-time {max_time} -X POST -H 'Content-Type: application/json' {ORIGIN_HEADERS} --data-binary @- 'http://127.0.0.1:{port}/hooks/codex/{route}/{token}' {redirect} || true"
    )
}

/// Hook commands run through `cmd.exe` on Windows, which ships `curl.exe`.
#[cfg(windows)]
fn curl_command(port: u16, token: &str, route: &str) -> String {
    let (max_time, redirect) = if route == "permission" {
        (125, "2>NUL")
    } else {
        (3, ">NUL 2>&1")
    };
    format!(
        "curl.exe --silent --fail --max-time {max_time} -X POST -H \"Content-Type: application/json\" {ORIGIN_HEADERS} --data-binary @- \"http://127.0.0.1:{port}/hooks/codex/{route}/{token}\" {redirect} || exit /b 0"
    )
}

fn config_path() -> Result<PathBuf, String> {
    codex_notify::config_path()
}

fn read(path: &Path) -> Result<DocumentMut, String> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    std::fs::read_to_string(path)
        .map_err(|error| error.to_string())?
        .parse::<DocumentMut>()
        .map_err(|error| format!("existing Codex config.toml is invalid: {error}"))
}

fn backup(path: &Path, app_data_dir: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let backup = app_data_dir
        .join("hook-backups")
        .join(format!("codex-config.{stamp}.toml.bak"));
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::copy(path, backup).map_err(|error| error.to_string())?;
    Ok(())
}

/// Turns an inline array (`hooks.Stop = [{...}]`) into an array of tables so
/// matcher groups and handlers can be handled one way. Anything that is not
/// an array of tables is left alone.
fn as_array_of_tables(item: &mut Item) {
    if item.is_array() {
        let original = std::mem::take(item);
        *item = match original.into_array_of_tables() {
            Ok(tables) => Item::ArrayOfTables(tables),
            Err(original) => original,
        };
    }
}

fn is_samlu_handler(handler: &Table) -> bool {
    handler
        .get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| command.contains(SAMLU_MARKER))
}

fn event_groups<'a>(document: &'a mut DocumentMut, event: &str) -> Option<&'a mut ArrayOfTables> {
    let item = document.get_mut("hooks")?.get_mut(event)?;
    as_array_of_tables(item);
    item.as_array_of_tables_mut()
}

/// Removes Samlu's handlers from one event, dropping matcher groups that
/// held only Samlu's handlers. Foreign groups and handlers are untouched.
fn remove_samlu(document: &mut DocumentMut, event: &str) {
    let Some(groups) = event_groups(document, event) else {
        return;
    };
    let mut index = 0;
    while index < groups.len() {
        let group = groups.get_mut(index).expect("index is in range");
        let mut emptied = false;
        if let Some(item) = group.get_mut("hooks") {
            as_array_of_tables(item);
            if let Some(handlers) = item.as_array_of_tables_mut() {
                let before = handlers.len();
                let mut at = 0;
                while at < handlers.len() {
                    if handlers.get(at).is_some_and(is_samlu_handler) {
                        handlers.remove(at);
                    } else {
                        at += 1;
                    }
                }
                emptied = before > 0 && handlers.is_empty();
            }
        }
        if emptied {
            groups.remove(index);
        } else {
            index += 1;
        }
    }
}

fn prune_empty(document: &mut DocumentMut) {
    let Some(hooks) = document.get_mut("hooks").and_then(Item::as_table_mut) else {
        return;
    };
    for (event, _, _) in EVENTS {
        if hooks
            .get(event)
            .and_then(Item::as_array_of_tables)
            .is_some_and(ArrayOfTables::is_empty)
        {
            hooks.remove(event);
        }
    }
    if hooks.is_empty() {
        document.remove("hooks");
    }
}

fn merged(mut document: DocumentMut, port: u16, token: &str) -> Result<DocumentMut, String> {
    if document.get("hooks").is_some_and(|item| !item.is_table()) {
        return Err("the existing \"hooks\" value in config.toml must be a table".to_string());
    }
    if document.get("hooks").is_none() {
        let mut hooks = Table::new();
        hooks.set_implicit(true);
        document["hooks"] = Item::Table(hooks);
    }
    for (event, route, timeout) in EVENTS {
        remove_samlu(&mut document, event);
        let mut handler = Table::new();
        handler["type"] = value("command");
        handler["command"] = value(curl_command(port, token, route));
        handler["timeout"] = value(timeout);
        let mut handlers = ArrayOfTables::new();
        handlers.push(handler);
        let mut group = Table::new();
        group.set_implicit(true);
        group.insert("hooks", Item::ArrayOfTables(handlers));

        let hooks = document["hooks"].as_table_mut().expect("checked above");
        if !hooks.contains_key(event) {
            let mut groups = ArrayOfTables::new();
            groups.push(group);
            hooks.insert(event, Item::ArrayOfTables(groups));
            continue;
        }
        let item = hooks.get_mut(event).expect("checked above");
        as_array_of_tables(item);
        item.as_array_of_tables_mut()
            .ok_or_else(|| format!("the existing hooks.{event} value must be an array"))?
            .push(group);
    }
    Ok(document)
}

fn removed(mut document: DocumentMut) -> DocumentMut {
    for (event, _, _) in EVENTS {
        remove_samlu(&mut document, event);
    }
    prune_empty(&mut document);
    document
}

fn state(mut document: DocumentMut, port: u16, token: &str) -> InstallState {
    let mut found = 0;
    let mut current = 0;
    for (event, route, timeout) in EVENTS {
        let Some(groups) = event_groups(&mut document, event) else {
            continue;
        };
        let expected = curl_command(port, token, route);
        let mut samlu = Vec::new();
        for group in groups.iter_mut() {
            if let Some(item) = group.get_mut("hooks") {
                as_array_of_tables(item);
                if let Some(handlers) = item.as_array_of_tables() {
                    samlu.extend(handlers.iter().filter(|handler| is_samlu_handler(handler)));
                }
            }
        }
        if samlu.is_empty() {
            continue;
        }
        found += 1;
        if samlu.len() == 1
            && samlu[0].get("command").and_then(Item::as_str) == Some(expected.as_str())
            && samlu[0].get("timeout").and_then(Item::as_integer) == Some(timeout)
        {
            current += 1;
        }
    }
    match (found, current) {
        (0, _) => InstallState::NotInstalled,
        (_, current) if current == EVENTS.len() => InstallState::Installed,
        _ => InstallState::NeedsUpdate,
    }
}

pub fn status(port: u16, token: &str) -> Result<InstallState, String> {
    Ok(state(read(&config_path()?)?, port, token))
}

/// The TOML that `apply` would write, for the setup wizard's diff view.
pub fn preview(port: u16, token: &str) -> Result<String, String> {
    preview_at(&config_path()?, port, token)
}

fn preview_at(path: &Path, port: u16, token: &str) -> Result<String, String> {
    Ok(merged(read(path)?, port, token)?.to_string())
}

pub fn apply(app_data_dir: &Path, port: u16, token: &str) -> Result<(), String> {
    apply_at(&config_path()?, app_data_dir, port, token)
}

fn apply_at(path: &Path, app_data_dir: &Path, port: u16, token: &str) -> Result<(), String> {
    let document = merged(read(path)?, port, token)?;
    backup(path, app_data_dir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, document.to_string()).map_err(|error| error.to_string())
}

pub fn uninstall(app_data_dir: &Path) -> Result<(), String> {
    uninstall_at(&config_path()?, app_data_dir)
}

fn uninstall_at(path: &Path, app_data_dir: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let document = removed(read(path)?);
    backup(path, app_data_dir)?;
    std::fs::write(path, document.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    fn merged_text(text: &str, token: &str) -> String {
        merged(parse(text), 47823, token).unwrap().to_string()
    }

    #[test]
    fn merge_into_existing_config_preserves_comments_and_keys() {
        let out = merged_text("# my codex setup\nmodel = \"gpt-5\"\n", "tok");
        assert!(out.contains("# my codex setup"));
        assert!(out.contains("model = \"gpt-5\""));
        for (event, route, _) in EVENTS {
            assert!(out.contains(&format!("[[hooks.{event}]]")), "{event}");
            assert!(out.contains(&format!("[[hooks.{event}.hooks]]")), "{event}");
            assert!(
                out.contains(&format!("/hooks/codex/{route}/tok")),
                "{route}"
            );
        }
        assert!(out.contains("timeout = 130"));
        assert_eq!(out.matches("timeout = 5").count(), 5);
        assert_eq!(
            state(parse(&out), 47823, "tok"),
            InstallState::Installed,
            "{out}"
        );
    }

    #[test]
    fn merge_replaces_stale_samlu_entries_only() {
        let start =
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo mine\"\n";
        let first = merged_text(start, "old");
        let second = merged_text(&first, "new");
        assert!(second.contains("echo mine"));
        assert!(second.contains("/hooks/codex/stop/new"));
        assert!(!second.contains("/old"));
        assert_eq!(second.matches("/hooks/codex/stop/").count(), 1);
    }

    #[test]
    fn status_states() {
        assert_eq!(state(parse(""), 47823, "t"), InstallState::NotInstalled);
        let full = merged(parse(""), 47823, "t").unwrap();
        assert_eq!(state(full.clone(), 47823, "t"), InstallState::Installed);
        assert_eq!(
            state(full.clone(), 47823, "other"),
            InstallState::NeedsUpdate
        );
        let mut partial = full;
        remove_samlu(&mut partial, "Stop");
        assert_eq!(state(partial, 47823, "t"), InstallState::NeedsUpdate);
    }

    #[cfg(not(windows))]
    #[test]
    fn command_without_fail_needs_update() {
        let full = merged(parse(""), 47823, "t").unwrap();
        let old = full.to_string().replace("--silent --fail ", "--silent ");
        assert_eq!(state(parse(&old), 47823, "t"), InstallState::NeedsUpdate);
    }

    #[test]
    fn uninstall_removes_only_samlu() {
        let start = "model = \"gpt-5\"\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo mine\"\n";
        let out = removed(parse(&merged_text(start, "t"))).to_string();
        assert!(out.contains("echo mine"));
        assert!(out.contains("model = \"gpt-5\""));
        assert!(!out.contains("/hooks/codex/"));
        assert!(!out.contains("PermissionRequest"));
        // A config with only Samlu's hooks goes back to having no hooks table.
        let bare = removed(parse(&merged_text("a = 1\n", "t"))).to_string();
        assert!(!bare.contains("hooks"));
    }

    #[test]
    fn inline_foreign_hooks_survive_a_merge() {
        let start =
            "[hooks]\nStop = [{ hooks = [{ type = \"command\", command = \"echo inline\" }] }]\n";
        let out = merged_text(start, "t");
        assert!(out.contains("echo inline"));
        assert_eq!(state(parse(&out), 47823, "t"), InstallState::Installed);
    }

    #[test]
    fn apply_backs_up_and_preview_matches_what_is_written() {
        let dir = std::env::temp_dir().join(format!("samlu-codex-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "model = \"gpt-5\"\n").unwrap();
        let data = dir.join("data");
        let previewed = preview_at(&config, 47823, "t").unwrap();
        apply_at(&config, &data, 47823, "t").unwrap();
        assert_eq!(
            std::fs::read_dir(data.join("hook-backups"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), previewed);
        uninstall_at(&config, &data).unwrap();
        assert_eq!(
            std::fs::read_to_string(&config).unwrap().trim(),
            "model = \"gpt-5\""
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn permission_command_prints_body_and_never_fails() {
        let command = curl_command(47823, "t", "permission");
        assert!(command.contains("--max-time 125"));
        assert!(command.contains("--silent --fail "));
        assert!(command.ends_with("|| true"));
        assert!(command.contains("2>/dev/null"));
        assert!(!command.contains(">/dev/null 2>&1"));
        assert!(command.contains("-H \"X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE\""));
    }

    #[cfg(not(windows))]
    #[test]
    fn other_commands_discard_output() {
        for route in [
            "session-start",
            "session-end",
            "prompt",
            "post-tool",
            "stop",
        ] {
            let command = curl_command(47823, "t", route);
            assert!(command.contains("--max-time 3 "), "{route}");
            assert!(command.contains(">/dev/null 2>&1 || true"), "{route}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_commands_use_cmd_syntax() {
        let permission = curl_command(47823, "t", "permission");
        assert!(permission.starts_with("curl.exe "));
        assert!(permission.contains("--silent --fail "));
        assert!(permission.contains("2>NUL || exit /b 0"));
        assert!(!permission.contains(">NUL 2>&1"));
        assert!(permission.contains("%WARP_FOCUS_URL%"));
        assert!(curl_command(47823, "t", "stop").contains(">NUL 2>&1 || exit /b 0"));
    }
}

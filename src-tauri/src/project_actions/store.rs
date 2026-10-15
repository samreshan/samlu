//! `project-actions.json`: the actions, key bindings and preferences, kept in
//! memory and written back on every change.

use super::model::{
    normalize_chord, ActionKind, ActionsConfig, ProjectAction, RunMode, TaskStep, CHORDS,
    CONFIG_VERSION,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "project-actions.json";

pub struct ActionsStore {
    path: PathBuf,
    data: Mutex<ActionsConfig>,
}

impl ActionsStore {
    /// Loads `project-actions.json` from `app_data_dir`. A missing file is
    /// seeded with `seed()` and written; an unreadable one is renamed to
    /// `project-actions.json.bak` and seeded.
    pub fn load(app_data_dir: &Path, seed: impl FnOnce() -> ActionsConfig) -> Self {
        let path = app_data_dir.join(FILE_NAME);
        let loaded = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str::<ActionsConfig>(&contents).ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => None,
        };
        let fresh = loaded.is_none();
        if fresh && path.exists() {
            // Kept rather than overwritten, so a hand edit gone wrong can be
            // recovered.
            let _ = std::fs::rename(&path, path.with_extension("json.bak"));
        }
        let store = Self {
            data: Mutex::new(sanitize(loaded.unwrap_or_else(seed))),
            path,
        };
        if fresh {
            store.write(&store.data.lock().unwrap());
        }
        store
    }

    pub fn snapshot(&self) -> ActionsConfig {
        self.data.lock().unwrap().clone()
    }

    /// Inserts, or replaces the action with the same id. An empty id gets a
    /// new `custom-<hex>` id. Returns the action as saved.
    pub fn upsert(&self, action: ProjectAction) -> Result<ProjectAction, String> {
        let mut action = cleaned(action)?;
        self.update(move |config| {
            if action.id.is_empty() {
                action.id = new_id(|id| config.action(id).is_some());
            }
            match config
                .actions
                .iter_mut()
                .find(|existing| existing.id == action.id)
            {
                Some(existing) => *existing = action.clone(),
                None => config.actions.push(action.clone()),
            }
            Ok(action)
        })
    }

    /// Removes the action and every binding to it.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.update(|config| {
            let before = config.actions.len();
            config.actions.retain(|action| action.id != id);
            if config.actions.len() == before {
                return Err("That action no longer exists.".to_string());
            }
            config.bindings.retain(|_, bound| bound != id);
            Ok(())
        })
    }

    /// Puts the actions in the order of `ids`. Actions not named keep their
    /// relative order after the named ones; unknown ids are ignored.
    pub fn reorder(&self, ids: &[String]) -> Result<(), String> {
        self.update(|config| {
            let mut rest = std::mem::take(&mut config.actions);
            let mut ordered = Vec::with_capacity(rest.len());
            for id in ids {
                if let Some(position) = rest.iter().position(|action| &action.id == id) {
                    ordered.push(rest.remove(position));
                }
            }
            ordered.append(&mut rest);
            config.actions = ordered;
            Ok(())
        })
    }

    /// Binds `chord` (any spelling `normalize_chord` accepts) to an action,
    /// or clears it with `None`.
    pub fn bind(&self, chord: &str, action_id: Option<&str>) -> Result<(), String> {
        let chord = normalize_chord(chord)
            .filter(|chord| CHORDS.contains(&chord.as_str()))
            .ok_or_else(|| "Samlu can bind Return with one modifier: ↩ ⌘↩ ⌥↩ ⌃↩ ⇧↩.".to_string())?;
        self.update(move |config| {
            match action_id {
                Some(id) if config.action(id).is_none() => {
                    return Err("That action no longer exists.".to_string());
                }
                Some(id) => {
                    config.bindings.insert(chord, id.to_string());
                }
                None => {
                    config.bindings.remove(&chord);
                }
            }
            Ok(())
        })
    }

    pub fn set_preferences(
        &self,
        terminal: String,
        script_mode: RunMode,
        notify_task_finished: bool,
        notify_task_url: bool,
    ) -> Result<(), String> {
        self.update(move |config| {
            config.terminal = terminal.trim().to_string();
            config.script_mode = script_mode;
            config.notify_task_finished = notify_task_finished;
            config.notify_task_url = notify_task_url;
            Ok(())
        })
    }

    /// Replaces everything with `seed()` and writes it.
    pub fn reset(&self, seed: impl FnOnce() -> ActionsConfig) {
        let mut data = self.data.lock().unwrap();
        *data = sanitize(seed());
        self.write(&data);
    }

    /// Applies `change` to a copy and keeps it only if it succeeds, so a
    /// refused edit never leaves half a change behind. The lock is held
    /// through the write, which keeps writes in the order they were made.
    fn update<T>(
        &self,
        change: impl FnOnce(&mut ActionsConfig) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut data = self.data.lock().unwrap();
        let mut next = data.clone();
        let value = change(&mut next)?;
        *data = next;
        self.write(&data);
        Ok(value)
    }

    /// Writes beside the real file and renames it into place, so a crash
    /// mid-write never leaves half a configuration. A failed write keeps the
    /// change in memory, the same as the other launcher stores.
    fn write(&self, config: &ActionsConfig) {
        let Ok(json) = serde_json::to_string_pretty(config) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, json).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

/// What a file on disk can't be trusted to get right: one action per id,
/// and keys that name a real chord and an action that exists.
fn sanitize(mut config: ActionsConfig) -> ActionsConfig {
    let mut ids = HashSet::new();
    let mut actions = Vec::with_capacity(config.actions.len());
    for mut action in std::mem::take(&mut config.actions) {
        if action.id.trim().is_empty() {
            action.id = new_id(|id| ids.contains(id));
        }
        if ids.insert(action.id.clone()) {
            actions.push(action);
        }
    }

    // A key already written the canonical way wins over an alias of it.
    let (canonical, aliased): (Vec<_>, Vec<_>) = std::mem::take(&mut config.bindings)
        .into_iter()
        .partition(|(chord, _)| CHORDS.contains(&chord.as_str()));
    let mut bindings = BTreeMap::new();
    for (chord, id) in canonical.into_iter().chain(aliased) {
        let Some(chord) = normalize_chord(&chord).filter(|chord| CHORDS.contains(&chord.as_str()))
        else {
            continue;
        };
        if ids.contains(&id) && !bindings.contains_key(&chord) {
            bindings.insert(chord, id);
        }
    }

    config.actions = actions;
    config.bindings = bindings;
    config.version = CONFIG_VERSION;
    config
}

/// Trims every field and checks the action can run.
fn cleaned(action: ProjectAction) -> Result<ProjectAction, String> {
    let title = action.title.trim().to_string();
    if title.is_empty() {
        return Err("Give the action a name.".to_string());
    }
    let kind = match action.kind {
        ActionKind::Open {
            bundle_id,
            app_path,
        } => {
            let (bundle_id, app_path) = (bundle_id.trim().to_string(), app_path.trim().to_string());
            if bundle_id.is_empty() && app_path.is_empty() {
                return Err("Choose an app.".to_string());
            }
            ActionKind::Open {
                bundle_id,
                app_path,
            }
        }
        ActionKind::Url { url } => {
            let url = url.trim().to_string();
            if !url.contains("://") {
                return Err("Enter a full link, such as https://… or vscode://…".to_string());
            }
            ActionKind::Url { url }
        }
        ActionKind::Terminal {
            command,
            terminal,
            cwd,
        } => {
            let command = command.trim().to_string();
            if command.is_empty() {
                return Err("Enter a command.".to_string());
            }
            ActionKind::Terminal {
                command,
                terminal: terminal.trim().to_string(),
                cwd: cwd.trim().to_string(),
            }
        }
        ActionKind::Task {
            steps,
            continue_on_error,
        } => {
            if steps.is_empty() {
                return Err("Add at least one step.".to_string());
            }
            let steps = steps
                .into_iter()
                .enumerate()
                .map(|(index, step)| {
                    let command = step.command.trim().to_string();
                    if command.is_empty() {
                        return Err(format!("Step {} needs a command.", index + 1));
                    }
                    Ok(TaskStep {
                        title: step.title.trim().to_string(),
                        command,
                        cwd: step.cwd.trim().to_string(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            ActionKind::Task {
                steps,
                continue_on_error,
            }
        }
        ActionKind::Script { scripts, mode } => {
            // Settings sends a list, but a hand-written "dev, start" works too.
            let scripts: Vec<String> = scripts
                .iter()
                .flat_map(|names| names.split(','))
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect();
            if scripts.is_empty() {
                return Err("Name at least one script, such as dev.".to_string());
            }
            ActionKind::Script { scripts, mode }
        }
        other @ (ActionKind::Reveal | ActionKind::CopyPath) => other,
    };
    Ok(ProjectAction {
        id: action.id.trim().to_string(),
        title,
        kind,
    })
}

/// `custom-<hex nanoseconds>`, nudged forward until `taken` says it is free.
fn new_id(taken: impl Fn(&str) -> bool) -> String {
    let mut stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    loop {
        let id = format!("custom-{stamp:x}");
        if !taken(&id) {
            return id;
        }
        stamp += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seeded() -> ActionsConfig {
        ActionsConfig {
            actions: vec![
                ProjectAction {
                    id: "open-finder".into(),
                    title: "Open in Finder".into(),
                    kind: ActionKind::Open {
                        bundle_id: "com.apple.finder".into(),
                        app_path: String::new(),
                    },
                },
                ProjectAction {
                    id: "reveal".into(),
                    title: "Show in Finder".into(),
                    kind: ActionKind::Reveal,
                },
            ],
            bindings: [
                ("enter".to_string(), "open-finder".to_string()),
                ("cmd+enter".to_string(), "reveal".to_string()),
            ]
            .into_iter()
            .collect(),
            ..ActionsConfig::default()
        }
    }

    fn terminal(title: &str, command: &str) -> ProjectAction {
        ProjectAction {
            id: String::new(),
            title: title.into(),
            kind: ActionKind::Terminal {
                command: command.into(),
                terminal: String::new(),
                cwd: String::new(),
            },
        }
    }

    #[test]
    fn a_missing_file_is_seeded_and_written() {
        let dir = temp_dir("seed");
        let store = ActionsStore::load(&dir, seeded);
        assert_eq!(store.snapshot(), seeded());
        let written: ActionsConfig =
            serde_json::from_str(&std::fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(written, seeded());
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_reseeded() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        let store = ActionsStore::load(&dir, seeded);
        assert_eq!(store.snapshot(), seeded());
        assert_eq!(
            std::fs::read_to_string(dir.join("project-actions.json.bak")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn changes_survive_a_reload() {
        let dir = temp_dir("reload");
        let saved = ActionsStore::load(&dir, seeded)
            .upsert(terminal("Tunnel", "ctun start"))
            .unwrap();
        let reloaded = ActionsStore::load(&dir, || panic!("must not reseed"));
        assert_eq!(reloaded.snapshot().action(&saved.id), Some(&saved));
    }

    #[test]
    fn new_actions_get_ids_and_trimmed_fields() {
        let store = ActionsStore::load(&temp_dir("upsert"), seeded);
        let first = store.upsert(terminal(" Tunnel ", " ctun start ")).unwrap();
        let second = store.upsert(terminal("Again", "ctun start")).unwrap();
        assert!(first.id.starts_with("custom-"));
        assert_ne!(first.id, second.id);
        assert_eq!(first.title, "Tunnel");
        assert_eq!(
            first.kind,
            ActionKind::Terminal {
                command: "ctun start".into(),
                terminal: String::new(),
                cwd: String::new(),
            }
        );
    }

    #[test]
    fn saving_an_existing_id_replaces_it_in_place() {
        let store = ActionsStore::load(&temp_dir("replace"), seeded);
        let mut renamed = store.snapshot().action("reveal").unwrap().clone();
        renamed.title = "Reveal".into();
        store.upsert(renamed).unwrap();
        let config = store.snapshot();
        assert_eq!(config.actions.len(), 2);
        assert_eq!(config.actions[1].title, "Reveal");
    }

    #[test]
    fn invalid_actions_are_refused_with_a_reason() {
        let store = ActionsStore::load(&temp_dir("invalid"), seeded);
        let refuse = |action: ProjectAction| store.upsert(action).unwrap_err();
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: " ".into(),
                kind: ActionKind::Reveal
            }),
            "Give the action a name."
        );
        assert_eq!(refuse(terminal("Empty", "  ")), "Enter a command.");
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "No steps".into(),
                kind: ActionKind::Task {
                    steps: vec![],
                    continue_on_error: false
                }
            }),
            "Add at least one step."
        );
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "Blank".into(),
                kind: ActionKind::Task {
                    steps: vec![
                        TaskStep {
                            title: "One".into(),
                            command: "true".into(),
                            cwd: String::new()
                        },
                        TaskStep {
                            title: "Two".into(),
                            command: " ".into(),
                            cwd: String::new()
                        },
                    ],
                    continue_on_error: false,
                },
            }),
            "Step 2 needs a command."
        );
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "Nothing".into(),
                kind: ActionKind::Script {
                    scripts: vec![" ".into()],
                    mode: RunMode::Task
                }
            }),
            "Name at least one script, such as dev."
        );
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "Link".into(),
                kind: ActionKind::Url {
                    url: "localhost:3000".into()
                }
            }),
            "Enter a full link, such as https://… or vscode://…"
        );
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "App".into(),
                kind: ActionKind::Open {
                    bundle_id: String::new(),
                    app_path: String::new()
                }
            }),
            "Choose an app."
        );
    }

    #[test]
    fn script_names_may_arrive_comma_separated() {
        let store = ActionsStore::load(&temp_dir("scripts"), seeded);
        let saved = store
            .upsert(ProjectAction {
                id: String::new(),
                title: "Run".into(),
                kind: ActionKind::Script {
                    scripts: vec!["dev, start".into(), " run ".into()],
                    mode: RunMode::Terminal,
                },
            })
            .unwrap();
        assert_eq!(
            saved.kind,
            ActionKind::Script {
                scripts: vec!["dev".into(), "start".into(), "run".into()],
                mode: RunMode::Terminal,
            }
        );
    }

    #[test]
    fn deleting_an_action_clears_its_keys() {
        let store = ActionsStore::load(&temp_dir("delete"), seeded);
        store.delete("reveal").unwrap();
        let config = store.snapshot();
        assert!(config.action("reveal").is_none());
        assert!(!config.bindings.contains_key("cmd+enter"));
        assert_eq!(
            store.delete("reveal").unwrap_err(),
            "That action no longer exists."
        );
    }

    #[test]
    fn keys_bind_by_any_spelling_to_real_actions_only() {
        let store = ActionsStore::load(&temp_dir("bind"), seeded);
        store.bind("Option+Return", Some("reveal")).unwrap();
        assert_eq!(
            store
                .snapshot()
                .bindings
                .get("alt+enter")
                .map(String::as_str),
            Some("reveal")
        );
        assert!(store.bind("alt+enter", Some("missing")).is_err());
        assert!(store.bind("hyper+enter", Some("reveal")).is_err());
        assert!(store.bind("cmd+shift+enter", Some("reveal")).is_err());
        store.bind("enter", None).unwrap();
        assert!(!store.snapshot().bindings.contains_key("enter"));
    }

    #[test]
    fn reorder_puts_named_actions_first() {
        let store = ActionsStore::load(&temp_dir("reorder"), seeded);
        let custom = store.upsert(terminal("Tunnel", "ctun start")).unwrap();
        store
            .reorder(&[custom.id.clone(), "missing".into()])
            .unwrap();
        let ids: Vec<String> = store
            .snapshot()
            .actions
            .into_iter()
            .map(|action| action.id)
            .collect();
        assert_eq!(
            ids,
            vec![custom.id, "open-finder".to_string(), "reveal".to_string()]
        );
    }

    #[test]
    fn loading_cleans_up_stale_bindings_and_duplicate_ids() {
        let dir = temp_dir("sanitize");
        std::fs::write(
            dir.join(FILE_NAME),
            r#"{"actions":[{"id":"reveal","title":"A","kind":"reveal"},{"id":"reveal","title":"B","kind":"copy_path"}],
                "bindings":{"enter":"gone","cmd+enter":"reveal","hyper+enter":"reveal"}}"#,
        )
        .unwrap();
        let config = ActionsStore::load(&dir, || panic!("must not reseed")).snapshot();
        assert_eq!(config.actions.len(), 1);
        assert_eq!(config.actions[0].title, "A");
        assert_eq!(config.bindings.len(), 1);
        assert_eq!(
            config.bindings.get("cmd+enter").map(String::as_str),
            Some("reveal")
        );
    }

    #[test]
    fn preferences_and_reset_are_saved() {
        let dir = temp_dir("prefs");
        let store = ActionsStore::load(&dir, seeded);
        store
            .set_preferences("warp".into(), RunMode::Task, false, true)
            .unwrap();
        let config = ActionsStore::load(&dir, || panic!()).snapshot();
        assert_eq!(
            (
                config.terminal.as_str(),
                config.script_mode,
                config.notify_task_finished
            ),
            ("warp", RunMode::Task, false)
        );
        store.reset(seeded);
        assert_eq!(ActionsStore::load(&dir, || panic!()).snapshot(), seeded());
    }
}

//! Shared shapes for project actions: what Samlu can do with a project, the
//! keys that run those actions from the launcher, and the tracked tasks they
//! start. Every file in this module, the launcher and the Settings page speak
//! these types, so they are the contract between them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const CONFIG_VERSION: u32 = 1;

/// The keys a project result answers to, in the order Settings lists them.
/// Modifiers are written in macOS order (⌃⌥⇧⌘) before `enter`.
pub const CHORDS: [&str; 5] = [
    "enter",
    "cmd+enter",
    "alt+enter",
    "ctrl+enter",
    "shift+enter",
];

/// Terminals Samlu can run a command in, in the order the default is picked
/// from: the first one installed wins.
pub const COMMAND_TERMINALS: [&str; 4] = ["warp", "ghostty", "iterm", "terminal"];

/// A project the launcher found: its folder name and absolute path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRef {
    pub name: String,
    pub path: String,
}

/// One thing Samlu can do with a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAction {
    pub id: String,
    pub title: String,
    #[serde(flatten)]
    pub kind: ActionKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActionKind {
    /// Opens the folder with an app: `open -b <bundle_id>`, or the app at
    /// `app_path` when the bundle id is empty or unknown.
    Open {
        bundle_id: String,
        #[serde(default)]
        app_path: String,
    },
    /// Selects the folder in Finder.
    Reveal,
    /// Copies the folder's path.
    CopyPath,
    /// Opens a URL. `{path}` and `{name}` are percent-encoded into it.
    Url { url: String },
    /// Runs a command in a terminal window, in the project folder (or `cwd`
    /// inside it). An empty `terminal` means the default one.
    Terminal {
        command: String,
        #[serde(default)]
        terminal: String,
        #[serde(default)]
        cwd: String,
    },
    /// Runs steps one after another in the background, tracked by Samlu.
    Task {
        steps: Vec<TaskStep>,
        #[serde(default)]
        continue_on_error: bool,
    },
    /// Runs the project's own script: the first detected script whose name
    /// is in `scripts`, in that order.
    Script {
        scripts: Vec<String>,
        #[serde(default)]
        mode: RunMode,
    },
}

impl ActionKind {
    /// The `kind` tag as the pages see it.
    pub fn name(&self) -> &'static str {
        match self {
            ActionKind::Open { .. } => "open",
            ActionKind::Reveal => "reveal",
            ActionKind::CopyPath => "copy_path",
            ActionKind::Url { .. } => "url",
            ActionKind::Terminal { .. } => "terminal",
            ActionKind::Task { .. } => "task",
            ActionKind::Script { .. } => "script",
        }
    }
}

impl ProjectAction {
    /// Whether running this starts a tracked task, which the launcher stays
    /// open to show.
    pub fn opens_task(&self) -> bool {
        matches!(
            self.kind,
            ActionKind::Task { .. }
                | ActionKind::Script {
                    mode: RunMode::Task,
                    ..
                }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStep {
    #[serde(default)]
    pub title: String,
    pub command: String,
    /// Folder relative to the project; empty for the project itself.
    #[serde(default)]
    pub cwd: String,
}

/// How a script runs: in a terminal window, or as a tracked task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Terminal,
    Task,
}

/// Everything in `project-actions.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionsConfig {
    pub version: u32,
    pub actions: Vec<ProjectAction>,
    /// Chord (one of `CHORDS`) to action id. A chord left out opens the
    /// launcher's action panel.
    pub bindings: BTreeMap<String, String>,
    /// Terminal id for commands; empty means `Catalog::default_terminal`.
    pub terminal: String,
    /// How the action panel runs a detected script on Return.
    pub script_mode: RunMode,
    pub notify_task_finished: bool,
    pub notify_task_url: bool,
}

impl Default for ActionsConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            actions: Vec::new(),
            bindings: BTreeMap::new(),
            terminal: String::new(),
            script_mode: RunMode::Terminal,
            notify_task_finished: true,
            notify_task_url: true,
        }
    }
}

impl ActionsConfig {
    pub fn action(&self, id: &str) -> Option<&ProjectAction> {
        self.actions.iter().find(|action| action.id == id)
    }

    /// The action `chord` runs, if it is bound to one that still exists.
    pub fn bound(&self, chord: &str) -> Option<&ProjectAction> {
        self.bindings.get(chord).and_then(|id| self.action(id))
    }

    /// The chords bound to `action_id`, in `CHORDS` order.
    pub fn chords_for(&self, action_id: &str) -> Vec<String> {
        CHORDS
            .iter()
            .filter(|chord| self.bindings.get(**chord).map(String::as_str) == Some(action_id))
            .map(|chord| chord.to_string())
            .collect()
    }
}

/// Writes a chord the way `CHORDS` does: lowercase, modifiers in ⌃⌥⇧⌘ order,
/// then `enter`. Accepts the spellings people and keyboards use ("Cmd",
/// "Command", "Meta", "Option", "Return", "⌘"). `None` for anything else.
pub fn normalize_chord(raw: &str) -> Option<String> {
    let (mut ctrl, mut alt, mut shift, mut cmd, mut enter) = (false, false, false, false, false);
    for part in raw.split('+').map(|part| part.trim().to_lowercase()) {
        let flag = match part.as_str() {
            "ctrl" | "control" | "⌃" => &mut ctrl,
            "alt" | "option" | "opt" | "⌥" => &mut alt,
            "shift" | "⇧" => &mut shift,
            "cmd" | "command" | "meta" | "super" | "⌘" => &mut cmd,
            "enter" | "return" | "↩" => &mut enter,
            _ => return None,
        };
        if *flag {
            return None;
        }
        *flag = true;
    }
    if !enter {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for (held, name) in [(ctrl, "ctrl"), (alt, "alt"), (shift, "shift"), (cmd, "cmd")] {
        if held {
            parts.push(name);
        }
    }
    parts.push("enter");
    Some(parts.join("+"))
}

/// "cmd+enter" → "⌘↩", for messages.
pub fn chord_label(chord: &str) -> String {
    chord
        .split('+')
        .map(|part| match part {
            "ctrl" => "⌃",
            "alt" => "⌥",
            "shift" => "⇧",
            "cmd" => "⌘",
            "enter" => "↩",
            other => other,
        })
        .collect()
}

/* ------------------------------------------------------------- catalog */

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppGroup {
    Finder,
    Editor,
    Agent,
    Terminal,
    Git,
}

/// An installed app Samlu has a preset for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogApp {
    /// Id of the preset action that opens a project with it ("open-vscode").
    pub id: String,
    pub name: String,
    pub bundle_id: String,
    pub app_path: String,
    pub group: AppGroup,
}

/// An agent or developer CLI found on the login PATH.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCli {
    /// Id of the preset action that runs it ("cli-claude").
    pub id: String,
    pub name: String,
    pub command: String,
    pub path: String,
}

/// An installed terminal app.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalApp {
    /// "terminal", "warp", "iterm", "ghostty", "wezterm", "kitty",
    /// "alacritty".
    pub id: String,
    pub name: String,
    pub bundle_id: String,
    pub app_path: String,
    /// Whether Samlu can run a command in it (listed in
    /// `COMMAND_TERMINALS`); the others can only be opened at a folder.
    pub runs_commands: bool,
}

/// What is installed that Samlu knows how to use.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub apps: Vec<CatalogApp>,
    pub clis: Vec<CatalogCli>,
    pub terminals: Vec<TerminalApp>,
    /// Actions Settings offers to add: one per installed app and CLI, plus
    /// the generic ones (Show in Finder, Copy Path, Run Project).
    pub presets: Vec<ProjectAction>,
    /// The terminal id an empty `terminal` resolves to: the first installed
    /// of `COMMAND_TERMINALS`, or "terminal".
    pub default_terminal: String,
}

impl Catalog {
    pub fn terminal(&self, id: &str) -> Option<&TerminalApp> {
        self.terminals.iter().find(|terminal| terminal.id == id)
    }
}

/* ------------------------------------------------------------- scripts */

/// A script the project defines, found by `scripts::detect`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedScript {
    /// `<cwd>:<source>:<name>`, e.g. "src-tauri:Cargo.toml:run" or
    /// ":package.json:dev".
    pub id: String,
    /// The script's own name: "dev", "test", "run".
    pub name: String,
    /// The shell command that runs it: "bun run dev", "cargo run".
    pub command: String,
    /// Folder relative to the project; empty for the project itself.
    pub cwd: String,
    /// The file it came from: "package.json", "Cargo.toml", "Makefile",
    /// "justfile", "go.mod", "Package.swift", "deno.json",
    /// "pyproject.toml", "manage.py", "compose.yaml".
    pub source: String,
}

/* --------------------------------------------------------------- tasks */

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Succeeded,
    Failed,
    Stopped,
}

/// A tracked task as the pages list it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub project: ProjectRef,
    pub status: TaskStatus,
    /// 0-based index of the step running now, or of the last one that ran.
    pub step: usize,
    pub step_count: usize,
    pub step_title: String,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// Exit code of the last step that finished.
    pub exit_code: Option<i32>,
    /// URLs seen in the output, first seen first, at most 10.
    pub urls: Vec<String>,
    /// The latest non-empty output line.
    pub last_line: String,
    pub log_path: String,
}

/// A task with its recent output, for the launcher's detail view.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    #[serde(flatten)]
    pub summary: TaskSummary,
    pub lines: Vec<String>,
}

/// What the runner is asked to run. Commands are already expanded and
/// folders already absolute.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskSpec {
    pub title: String,
    pub project: ProjectRef,
    pub steps: Vec<ResolvedStep>,
    pub continue_on_error: bool,
    /// Added on top of the login environment.
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedStep {
    pub title: String,
    pub command: String,
    pub cwd: PathBuf,
}

/// Payload of the `tasks://log` event.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogEvent {
    pub id: String,
    pub lines: Vec<String>,
}

/* ------------------------------------------------- views for the pages */

/// What a key on a project result does, for the launcher footer.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingView {
    pub chord: String,
    pub action_id: String,
    pub title: String,
    pub kind: String,
    pub opens_task: bool,
}

/// An action as the launcher's action panel lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub id: String,
    pub title: String,
    /// The command, URL, app or script names.
    pub subtitle: String,
    pub kind: String,
    /// Bundle path for the row's app icon when the action opens an app.
    pub app_path: Option<String>,
    /// Keys bound to it, in `CHORDS` order.
    pub chords: Vec<String>,
    pub opens_task: bool,
}

/// The launcher's action panel for one project.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPanel {
    pub project: ProjectRef,
    pub actions: Vec<ActionView>,
    pub scripts: Vec<DetectedScript>,
    pub script_mode: RunMode,
    /// This project's tasks, running first.
    pub tasks: Vec<TaskSummary>,
}

/// What running an action did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    /// Set when a tracked task started; the launcher shows it.
    pub task_id: Option<String>,
}

/// Settings › Projects.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub config: ActionsConfig,
    pub catalog: Catalog,
}

/// An app picked with "Choose App…".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChosenApp {
    pub name: String,
    pub bundle_id: String,
    pub app_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_round_trip_as_the_pages_write_them() {
        let json = r#"[
            {"id":"open-vscode","title":"Open in VS Code","kind":"open","bundleId":"com.microsoft.VSCode","appPath":"/Applications/Visual Studio Code.app"},
            {"id":"reveal","title":"Show in Finder","kind":"reveal"},
            {"id":"copy-path","title":"Copy Path","kind":"copy_path"},
            {"id":"u","title":"Docs","kind":"url","url":"https://example.com/{name}"},
            {"id":"cli-claude","title":"Claude Code","kind":"terminal","command":"claude"},
            {"id":"t","title":"Checks","kind":"task","continueOnError":true,"steps":[{"title":"Test","command":"cargo test","cwd":"src-tauri"}]},
            {"id":"run-project","title":"Run Project","kind":"script","scripts":["dev","run"],"mode":"task"}
        ]"#;
        let actions: Vec<ProjectAction> = serde_json::from_str(json).unwrap();
        assert_eq!(
            actions[0].kind,
            ActionKind::Open {
                bundle_id: "com.microsoft.VSCode".into(),
                app_path: "/Applications/Visual Studio Code.app".into(),
            }
        );
        assert_eq!(actions[2].kind, ActionKind::CopyPath);
        assert_eq!(
            actions[4].kind,
            ActionKind::Terminal {
                command: "claude".into(),
                terminal: String::new(),
                cwd: String::new(),
            }
        );
        assert!(actions[5].opens_task());
        assert!(actions[6].opens_task());
        assert!(!actions[4].opens_task());

        let written = serde_json::to_value(&actions[5]).unwrap();
        assert_eq!(written["kind"], "task");
        assert_eq!(written["continueOnError"], true);
        assert_eq!(written["steps"][0]["cwd"], "src-tauri");
        let again: Vec<ProjectAction> =
            serde_json::from_str(&serde_json::to_string(&actions).unwrap()).unwrap();
        assert_eq!(again, actions);
    }

    #[test]
    fn a_partial_config_takes_defaults() {
        let config: ActionsConfig = serde_json::from_str(r#"{"terminal":"warp"}"#).unwrap();
        assert_eq!(config.terminal, "warp");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.script_mode, RunMode::Terminal);
        assert!(config.notify_task_finished && config.notify_task_url);
    }

    #[test]
    fn bindings_resolve_to_existing_actions_only() {
        let mut config = ActionsConfig {
            actions: vec![ProjectAction {
                id: "reveal".into(),
                title: "Show in Finder".into(),
                kind: ActionKind::Reveal,
            }],
            ..ActionsConfig::default()
        };
        config.bindings.insert("cmd+enter".into(), "reveal".into());
        config.bindings.insert("alt+enter".into(), "gone".into());
        assert_eq!(config.bound("cmd+enter").unwrap().id, "reveal");
        assert!(config.bound("alt+enter").is_none());
        assert!(config.bound("enter").is_none());
        assert_eq!(config.chords_for("reveal"), vec!["cmd+enter".to_string()]);
    }

    #[test]
    fn chords_are_normalized() {
        assert_eq!(normalize_chord("enter").as_deref(), Some("enter"));
        assert_eq!(normalize_chord("Cmd+Return").as_deref(), Some("cmd+enter"));
        assert_eq!(normalize_chord("meta+enter").as_deref(), Some("cmd+enter"));
        assert_eq!(
            normalize_chord("Option+Enter").as_deref(),
            Some("alt+enter")
        );
        assert_eq!(
            normalize_chord("cmd+shift+enter").as_deref(),
            Some("shift+cmd+enter")
        );
        assert_eq!(normalize_chord("⌃+↩").as_deref(), Some("ctrl+enter"));
        assert_eq!(normalize_chord("cmd"), None);
        assert_eq!(normalize_chord("cmd+cmd+enter"), None);
        assert_eq!(normalize_chord("hyper+enter"), None);
        for chord in CHORDS {
            assert_eq!(normalize_chord(chord).as_deref(), Some(chord));
        }
    }

    #[test]
    fn chord_labels_use_key_symbols() {
        assert_eq!(chord_label("enter"), "↩");
        assert_eq!(chord_label("cmd+enter"), "⌘↩");
        assert_eq!(chord_label("shift+cmd+enter"), "⇧⌘↩");
    }
}

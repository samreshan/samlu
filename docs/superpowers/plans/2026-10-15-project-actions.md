# Project Actions and Tasks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This plan is executed as four parallel workstreams (Tasks A–D) on top of a finished contract (Task 0), then integrated (Task E) and verified (Task F).

**Goal:** Every key on a launcher project result runs a configurable action (open in an app, agent CLI in a terminal, ctun-style command, the project's own script, or a tracked multi-step task), with a per-project action panel, a `!` tasks view, and a Settings › Projects pane.

**Architecture:** A new Rust module `src-tauri/src/project_actions/` whose submodules have no Tauri dependency (store, catalog, scripts, shell, terminal, runner) and are glued to Tauri in `mod.rs` (already written). The launcher (`public/launcher/*`) and Settings (`public/index.html`, `settings.js`, `settings.css`) talk to it through Tauri commands and two events.

**Tech Stack:** Rust 2021 (Tauri 2, serde, serde_json, plist, toml_edit, dirs, libc), vanilla JS/CSS/HTML (no build step), macOS `open`, `osascript`, Terminal `.command` files, Warp launch configurations.

**Spec:** `docs/superpowers/specs/2026-10-15-project-actions-design.md`

## Global Constraints

- macOS only; minimum macOS 11. No new crates beyond those in `src-tauri/Cargo.toml` (`libc = "0.2"` was added in Task 0).
- The contract in `src-tauri/src/project_actions/model.rs` and the public signatures in the scaffolded files are fixed. Do not rename, re-type or remove them; add private helpers freely. If the contract truly cannot work, stop and report instead of changing it.
- `mod.rs`, `model.rs`, `lib.rs`, `launcher/*.rs` belong to the integrator. Agents edit only the files their task owns.
- CI runs `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` in `src-tauri`. Every pub item must be used (dead code fails clippy in the app crate).
- Keys are `enter`, `cmd+enter`, `alt+enter`, `ctrl+enter`, `shift+enter`; modifiers in ⌃⌥⇧⌘ order (`ctrl`, `alt`, `shift`, `cmd`).
- First-run defaults keep today's behaviour: Return = `open-finder`, ⌘↩ = `reveal`.
- `{path}`/`{name}` expand to single-quoted shell words in commands and percent-encoded text in URLs. Never splice a path into a shell string unquoted.
- Tasks run with the login environment resolved from `$SHELL -ilc` (Homebrew and `~/.local/bin` are only on PATH via `.zshrc` on this Mac), plus `SAMLU_PROJECT_PATH` and `SAMLU_PROJECT_NAME`.
- UI copy follows `docs/SAMLU_DESIGN_SYSTEM.md`: sentence case, short, no exclamation marks; at most one primary (yellow) button per pane; routine saves confirm with the inline "Saved" check; toasts only for errors.
- Build cache: `CARGO_TARGET_DIR=/Users/samreshan/Programming/samlu-companion/.claude/worktrees/bold-williams-ab1940/src-tauri/target` for the app crate. The disk has about 2.5 GB free: never `cargo clean`, never build release, never create another target directory beyond the scratch one named in Tasks A and B.

## Review Focus

1. **Paths with spaces, quotes, `$` or non-ASCII** (`/Users/me/My "App" $dev`) in `{path}` — the command must still receive exactly one argument. Pinned in Task B (shell tests).
2. **Samlu launched from Finder has a bare PATH, and `.zshrc` can be slow or noisy** (this Mac prints a Java error) — `ctun`, `cloudflared`, `npm` must still resolve, and resolution must give up after about five seconds. Pinned in Task B (env parsing with noise, timeout fallback).
3. **Long-running, chatty tasks** (a dev server printing thousands of lines, `npm` spawning `node`) — memory stays bounded, events are batched, and Stop kills grandchildren too. Pinned in Task B (cap test, process-group test).
4. **Stale configuration** — an action deleted while bound, a key bound to an id that no longer exists, an app uninstalled, a corrupt JSON file. Pinned in Task A (store tests) and Task E (open_with error path).
5. **Projects with no manifest, several manifests, or manifests only in a subfolder** (this repo keeps `Cargo.toml` in `src-tauri/`) — detection must find them, skip dependency folders, and never panic on malformed files. Pinned in Task A (scripts tests).

---

## File Structure

| File | Owner | Responsibility |
|---|---|---|
| `src-tauri/src/project_actions/model.rs` | Task 0 (done) | Shared serde types and chord helpers |
| `src-tauri/src/project_actions/mod.rs` | Task 0 (done) / E | Tauri commands, dispatch, events, notifications, history |
| `src-tauri/src/project_actions/store.rs` | Task A | `project-actions.json` load/seed/save/edit/bind/reset |
| `src-tauri/src/project_actions/catalog.rs` | Task A | Presets; installed app and CLI detection; first-run config |
| `src-tauri/src/project_actions/scripts.rs` | Task A | Script detection and picking |
| `src-tauri/src/project_actions/shell.rs` | Task B | Login environment, quoting, template expansion |
| `src-tauri/src/project_actions/terminal.rs` | Task B | Run a command in Terminal, Warp, iTerm, Ghostty |
| `src-tauri/src/project_actions/runner.rs` | Task B | Tracked task runner |
| `public/launcher/launcher.js`, `launcher.css`, `launcher.html` | Task C | Keys, action panel, tasks list and detail |
| `public/index.html`, `public/settings.js`, `public/settings.css` | Task D | Settings › Projects pane |
| `src-tauri/src/lib.rs`, `Cargo.toml`, `launcher/mod.rs` | Task 0 (done) / E | Registration, shutdown, `open_projects_settings` |
| `README.md`, `PRODUCT.md`, `docs/SAMLU_DESIGN_SYSTEM.md` | Task E | Documentation |

## Tauri API (already implemented in `mod.rs`)

All arguments are camelCase from JavaScript. Every command returns a rejected promise with a user-facing string on error.

| Command | Arguments | Returns |
|---|---|---|
| `launcher_project_bindings` | — | `BindingView[]` `{ chord, actionId, title, kind, opensTask }` for bound keys only |
| `launcher_run_project_chord` | `{ path, chord, title?, subtitle? }` | `RunOutcome` `{ taskId: string \| null }` |
| `launcher_project_actions` | `{ path, title? }` | `ProjectPanel` `{ project: {name, path}, actions: ActionView[], scripts: DetectedScript[], scriptMode, tasks: TaskSummary[] }` |
| `launcher_run_project_action` | `{ path, actionId, title?, subtitle? }` | `RunOutcome` |
| `launcher_run_script` | `{ path, scriptId, mode?: "terminal"\|"task", title?, subtitle? }` | `RunOutcome` |
| `tasks_list` | — | `TaskSummary[]` running first |
| `task_detail` | `{ id }` | `TaskDetail \| null` (summary fields plus `lines: string[]`, last 400) |
| `task_stop` / `task_open_log` | `{ id }` | — |
| `task_rerun` | `{ id }` | `TaskSummary` (the new task) |
| `task_url` | `{ id, url, copy: boolean }` | — (opens or copies a URL the task printed) |
| `tasks_clear_finished` | — | — |
| `get_project_actions` | — | `{ config: ActionsConfig, catalog: Catalog }` |
| `save_project_action` | `{ action: ProjectAction }` | `ActionsConfig` (empty `id` creates) |
| `delete_project_action` | `{ id }` | `ActionsConfig` |
| `reorder_project_actions` | `{ ids: string[] }` | `ActionsConfig` |
| `set_project_action_binding` | `{ chord, actionId: string \| null }` | `ActionsConfig` |
| `set_project_action_preferences` | `{ terminal, scriptMode, notifyTaskFinished, notifyTaskUrl }` | `ActionsConfig` |
| `reset_project_actions` | — | `ActionsConfig` |
| `choose_application` | — | `{ name, bundleId, appPath } \| null` |
| `launcher_activate` | existing; `{ id, kind: "action", target: "open_projects_settings" }` opens Settings › Projects | — |

Events (broadcast to every window): `tasks://changed` → `TaskSummary`; `tasks://log` → `{ id, lines: string[] }`.

JSON shapes (camelCase):

```json
ActionView     { "id": "cli-claude", "title": "Claude Code", "subtitle": "claude", "kind": "terminal", "appPath": null, "chords": ["alt+enter"], "opensTask": false }
DetectedScript { "id": "src-tauri:Cargo.toml:run", "name": "run", "command": "cargo run", "cwd": "src-tauri", "source": "Cargo.toml" }
TaskSummary    { "id": "t-18f3a…", "title": "Checks", "project": { "name": "samlu-companion", "path": "/Users/…" }, "status": "running"|"succeeded"|"failed"|"stopped",
                 "step": 1, "stepCount": 3, "stepTitle": "Test", "startedAt": 1792000000000, "finishedAt": null, "exitCode": null,
                 "urls": ["https://abc.trycloudflare.com"], "lastLine": "Compiling samlu…", "logPath": "/Users/…/tasks/t-18f3a.log" }
ProjectAction  { "id": "", "title": "Tunnel with ctun", "kind": "terminal", "command": "ctun start", "terminal": "", "cwd": "" }
               kinds: open {bundleId, appPath} · reveal · copy_path · url {url} · terminal {command, terminal, cwd}
                      · task {steps: [{title, command, cwd}], continueOnError} · script {scripts: string[], mode}
ActionsConfig  { "version": 1, "actions": [...], "bindings": { "enter": "open-finder", "cmd+enter": "reveal" },
                 "terminal": "", "scriptMode": "terminal", "notifyTaskFinished": true, "notifyTaskUrl": true }
Catalog        { "apps": [{ "id": "open-vscode", "name": "VS Code", "bundleId": "com.microsoft.VSCode", "appPath": "/Applications/Visual Studio Code.app", "group": "editor" }],
                 "clis": [{ "id": "cli-claude", "name": "Claude Code", "command": "claude", "path": "/Users/…/.local/bin/claude" }],
                 "terminals": [{ "id": "warp", "name": "Warp", "bundleId": "dev.warp.Warp-Stable", "appPath": "/Applications/Warp.app", "runsCommands": true }],
                 "presets": [ProjectAction…], "defaultTerminal": "warp" }
```

---

### Task 0: Contract scaffold (done by the integrator)

**Files:** Created `src-tauri/src/project_actions/{model,mod,store,catalog,scripts,shell,terminal,runner}.rs` (stubs for A and B), modified `src-tauri/src/lib.rs` (module, state, 21 commands, shutdown on exit), `src-tauri/Cargo.toml` (`libc`), `src-tauri/src/launcher/mod.rs` (`open_projects_settings`).

- [x] Contract types with serde round-trip, defaults, binding and chord tests in `model.rs`
- [x] Glue and commands in `mod.rs` against stub bodies
- [x] `cargo check --all-targets --offline` passes with the shared target directory
- [x] Scratch crates for Tasks A and B at `$SCRATCH/crates/actions-data` and `$SCRATCH/crates/actions-exec`, mounting the real files by `#[path]`

`$SCRATCH` = `/private/tmp/claude-501/-Users-samreshan-Programming-samlu-companion/2dd0d66d-9cad-47d4-91ac-d71b73b64922/scratchpad`

---

### Task A: Configuration store, catalog, and script detection

**Files:**
- Modify (replace stub bodies): `src-tauri/src/project_actions/store.rs`, `catalog.rs`, `scripts.rs`
- Test: `#[cfg(test)] mod tests` at the bottom of each file
- Build harness: `$SCRATCH/crates/actions-data` (do not edit its `lib.rs`; add nothing to its `Cargo.toml`)

**Interfaces:**
- Consumes: `super::model::{ActionsConfig, ProjectAction, ActionKind, TaskStep, RunMode, Catalog, CatalogApp, CatalogCli, TerminalApp, AppGroup, DetectedScript, CHORDS, COMMAND_TERMINALS, CONFIG_VERSION, normalize_chord}`
- Produces (exact):
  - `store::FILE_NAME: &str = "project-actions.json"`
  - `ActionsStore::load(app_data_dir: &Path, seed: impl FnOnce() -> ActionsConfig) -> Self`
  - `ActionsStore::snapshot(&self) -> ActionsConfig`
  - `ActionsStore::upsert(&self, ProjectAction) -> Result<ProjectAction, String>`
  - `ActionsStore::delete(&self, &str) -> Result<(), String>`
  - `ActionsStore::reorder(&self, &[String]) -> Result<(), String>`
  - `ActionsStore::bind(&self, chord: &str, action_id: Option<&str>) -> Result<(), String>`
  - `ActionsStore::set_preferences(&self, terminal: String, script_mode: RunMode, notify_task_finished: bool, notify_task_url: bool) -> Result<(), String>`
  - `ActionsStore::reset(&self, seed: impl FnOnce() -> ActionsConfig)`
  - `catalog::detect(path_dirs: &[PathBuf]) -> Catalog`
  - `catalog::default_config(&Catalog) -> ActionsConfig`
  - `catalog::bundle_info(&Path) -> Option<(String /*bundle id*/, String /*display name*/)>`
  - `scripts::detect(root: &Path) -> Vec<DetectedScript>`
  - `scripts::pick<'a>(&'a [DetectedScript], names: &[String]) -> Option<&'a DetectedScript>`

**Behaviour, store:**
- `load`: missing file → `seed()`, write it. Unparseable file → rename to `project-actions.json.bak` (replacing an older `.bak`), `seed()`, write. Parsed file → sanitise: drop duplicate action ids (keep the first), drop bindings whose key is not in `CHORDS` after `normalize_chord`, or whose action no longer exists; set `version` to `CONFIG_VERSION`.
- Every mutation writes pretty JSON atomically (write `project-actions.json.tmp`, then rename). Create the folder if needed. A failed write is logged with `eprintln!`-free `log`-free silence like `snippets.rs`, but the in-memory change stays.
- `upsert` trims `title`, every string field, step fields and script names (dropping empty names). Errors, as written: empty title → "Give the action a name."; `open` with empty `bundle_id` and `app_path` → "Choose an app."; `url` without `://` → "Enter a full link, such as https://… or vscode://…"; `terminal` with empty command → "Enter a command."; `task` with no steps → "Add at least one step."; a step with an empty command → "Step N needs a command." (N is 1-based); `script` with no names → "Name at least one script, such as dev.". Empty id → `custom-<hex of nanoseconds since epoch>` (unique even when called twice in a row). Unknown non-empty id → appended (presets keep their preset ids when added from Settings).
- `delete` unknown id → "That action no longer exists."; also removes bindings to it.
- `bind`: chord through `normalize_chord`; must then be in `CHORDS`, else "Samlu can bind Return with one modifier: ↩ ⌘↩ ⌥↩ ⌃↩ ⇧↩."; `Some(id)` must exist, else "That action no longer exists."; `None` removes the binding.
- `reorder`: named ids first in the given order, then the rest in their previous order; unknown ids ignored.

**Behaviour, catalog:**
- Scan top-level `*.app` bundles in `/Applications`, `/Applications/Utilities`, `/System/Applications`, `/System/Applications/Utilities`, `~/Applications`, plus `/System/Library/CoreServices/Finder.app`; read `Contents/Info.plist` with the `plist` crate (`CFBundleIdentifier`; name from `CFBundleDisplayName`, else `CFBundleName`, else the file stem). `detect` builds the `(bundle_id, app_path)` list and a CLI lookup over `path_dirs` (first regular file with any execute bit), then calls a private `build(installed: &[(String, String)], find_cli: &dyn Fn(&str) -> Option<PathBuf>) -> Catalog`, which the tests drive.
- Known apps (bundle id → slug, display name, group). Preset id = `open-<slug>`, title `Open in <name>`, `ActionKind::Open { bundle_id, app_path }`:

| Bundle id | Slug | Name | Group |
|---|---|---|---|
| com.apple.finder | finder | Finder | finder |
| com.microsoft.VSCode | vscode | VS Code | editor |
| com.microsoft.VSCodeInsiders | vscode-insiders | VS Code Insiders | editor |
| com.todesktop.230313mzl4w4u92 | cursor | Cursor | editor |
| com.exafunction.windsurf | windsurf | Windsurf | editor |
| dev.zed.Zed | zed | Zed | editor |
| com.google.antigravity | antigravity | Antigravity | editor |
| com.google.antigravity-ide | antigravity-ide | Antigravity IDE | editor |
| com.apple.dt.Xcode | xcode | Xcode | editor |
| com.sublimetext.4 | sublime | Sublime Text | editor |
| com.panic.Nova | nova | Nova | editor |
| com.jetbrains.intellij | intellij | IntelliJ IDEA | editor |
| com.jetbrains.intellij.ce | intellij-ce | IntelliJ IDEA CE | editor |
| com.jetbrains.WebStorm | webstorm | WebStorm | editor |
| com.jetbrains.pycharm | pycharm | PyCharm | editor |
| com.jetbrains.pycharm.ce | pycharm-ce | PyCharm CE | editor |
| com.jetbrains.rustrover | rustrover | RustRover | editor |
| com.jetbrains.goland | goland | GoLand | editor |
| com.google.android.studio | android-studio | Android Studio | editor |
| com.anthropic.claudefordesktop | claude | Claude | agent |
| com.openai.codex | codex | Codex | agent |
| com.apple.Terminal | terminal | Terminal | terminal |
| dev.warp.Warp-Stable | warp | Warp | terminal |
| com.googlecode.iterm2 | iterm | iTerm | terminal |
| com.mitchellh.ghostty | ghostty | Ghostty | terminal |
| com.github.wez.wezterm | wezterm | WezTerm | terminal |
| net.kovidgoyal.kitty | kitty | kitty | terminal |
| org.alacritty | alacritty | Alacritty | terminal |
| com.github.GitHubClient | github-desktop | GitHub Desktop | git |
| com.DanPristupov.Fork | fork | Fork | git |
| com.fournova.Tower3 | tower | Tower | git |
| com.torusknot.SourceTreeNotMAS | sourcetree | Sourcetree | git |

  The name is the table's name, not the bundle's (this Mac's Codex app is `ChatGPT.app`). Finder is always present even when not in `installed`.
- Known CLIs (command → name). Preset id `cli-<command>`, title = name, `ActionKind::Terminal { command, terminal: "", cwd: "" }`: `claude` Claude Code, `codex` Codex CLI, `gemini` Gemini CLI, `agy` Antigravity CLI, `opencode` OpenCode, `aider` Aider, `cursor-agent` Cursor Agent, `amp` Amp, `copilot` Copilot CLI, `goose` Goose, `lazygit` Lazygit.
- `terminals`: installed terminal-group apps with `id` = slug and `runs_commands = COMMAND_TERMINALS.contains(slug)`. `default_terminal` = first of `COMMAND_TERMINALS` installed, else `"terminal"`.
- `presets` order: `open-finder`, `reveal` ("Show in Finder"), editors (table order), agent apps, CLIs (table order), `run-project` ("Run Project", `Script { scripts: ["dev","start","run","serve"], mode: Terminal }`), terminals and git apps (open at folder), `copy-path` ("Copy Path").
- `default_config(catalog)`: `ActionsConfig { actions: catalog.presets.clone(), bindings: { "enter": "open-finder", "cmd+enter": "reveal" }, ..Default::default() }`.

**Behaviour, scripts:**
- Folders: the root, then immediate subfolders sorted by name, skipping names that start with `.` or `_`, and `node_modules`, `target`, `dist`, `build`, `out`, `vendor`, `Pods`, `DerivedData`, `venv`, `env`; at most 12 subfolders. `cwd` is the subfolder name (empty for root).
- Per folder, in this source order: `package.json` (`scripts` in file order — this crate's `serde_json` has no `preserve_order`, so deserialize `scripts` through a small custom `MapAccess` visitor into `Vec<(String, String)>`; skip non-string values; package manager from the nearest lockfile in the folder or the project root: `bun.lockb`/`bun.lock` → `bun run <name>`, `pnpm-lock.yaml` → `pnpm run <name>`, `yarn.lock` → `yarn run <name>`, otherwise `npm run <name>`); `Cargo.toml` containing a `[package]` table (parse with `toml_edit`) → `run`, `test`, `build` as `cargo <name>`; `Makefile`/`makefile`/`GNUmakefile` → targets; `justfile`/`Justfile`/`.justfile` → recipes as `just <name>`; `go.mod` → `go run .`, `go test ./...`, `go build ./...` named run, test, build; `Package.swift` → `swift run`/`swift test`/`swift build`; `deno.json` → `tasks` as `deno task <name>`; `pyproject.toml` → `[project.scripts]` and `[tool.poetry.scripts]` names, command `uv run <name>` when `uv.lock` exists, `poetry run <name>` when `poetry.lock` exists, else `<name>`; `manage.py` → runserver as `python3 manage.py runserver`; `compose.yaml`/`compose.yml`/`docker-compose.yml`/`docker-compose.yaml` → up, down as `docker compose up` / `docker compose down`.
- Makefile targets: lines matching `^[A-Za-z0-9][A-Za-z0-9_.-]*( [A-Za-z0-9][A-Za-z0-9_.-]*)*\s*:` not followed by `=`; skip `.PHONY`-style (starting with `.`) and anything with `%` or `$`; each name on a multi-target line counts; first occurrence wins; at most 30.
- justfile recipes: a non-indented line, optionally starting with `@`, whose first word is the recipe name (letters, digits, `_`, `-`) and which has a `:` that is not part of `:=` after the name (parameters may contain `=`, as in `build target='debug':`); skip assignments (`name :=`), `set`, `alias`, `import`, `export`, `mod`, comments, and names starting with `_`; at most 30.
- There is no regex crate in the dependency set: parse Makefile and justfile lines by hand; the patterns above describe what to accept.
- At most 30 scripts per manifest and 120 in total; ids `<cwd>:<source>:<name>`; duplicates by id dropped. Unreadable or malformed files are skipped silently.
- `pick`: for each name in `names` order, the first script with that name, root folder before subfolders (detection order already puts the root first).

- [ ] **Step 1: Write the failing tests** (add more as you go; these are required)

`store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::super::model::{ActionKind, TaskStep};
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seeded() -> ActionsConfig {
        let mut config = ActionsConfig::default();
        config.actions = vec![
            ProjectAction {
                id: "open-finder".into(),
                title: "Open in Finder".into(),
                kind: ActionKind::Open { bundle_id: "com.apple.finder".into(), app_path: String::new() },
            },
            ProjectAction { id: "reveal".into(), title: "Show in Finder".into(), kind: ActionKind::Reveal },
        ];
        config.bindings.insert("enter".into(), "open-finder".into());
        config.bindings.insert("cmd+enter".into(), "reveal".into());
        config
    }

    fn terminal(title: &str, command: &str) -> ProjectAction {
        ProjectAction {
            id: String::new(),
            title: title.into(),
            kind: ActionKind::Terminal { command: command.into(), terminal: String::new(), cwd: String::new() },
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
        assert_eq!(std::fs::read_to_string(dir.join("project-actions.json.bak")).unwrap(), "{ not json");
    }

    #[test]
    fn changes_survive_a_reload() {
        let dir = temp_dir("reload");
        let saved = ActionsStore::load(&dir, seeded).upsert(terminal("Tunnel", "ctun start")).unwrap();
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
            ActionKind::Terminal { command: "ctun start".into(), terminal: String::new(), cwd: String::new() }
        );
    }

    #[test]
    fn invalid_actions_are_refused_with_a_reason() {
        let store = ActionsStore::load(&temp_dir("invalid"), seeded);
        let refuse = |action: ProjectAction| store.upsert(action).unwrap_err();
        assert_eq!(refuse(ProjectAction { id: String::new(), title: " ".into(), kind: ActionKind::Reveal }), "Give the action a name.");
        assert_eq!(refuse(terminal("Empty", "  ")), "Enter a command.");
        assert_eq!(
            refuse(ProjectAction { id: String::new(), title: "No steps".into(), kind: ActionKind::Task { steps: vec![], continue_on_error: false } }),
            "Add at least one step."
        );
        assert_eq!(
            refuse(ProjectAction {
                id: String::new(),
                title: "Blank".into(),
                kind: ActionKind::Task {
                    steps: vec![
                        TaskStep { title: "One".into(), command: "true".into(), cwd: String::new() },
                        TaskStep { title: "Two".into(), command: " ".into(), cwd: String::new() },
                    ],
                    continue_on_error: false,
                },
            }),
            "Step 2 needs a command."
        );
        assert_eq!(
            refuse(ProjectAction { id: String::new(), title: "Nothing".into(), kind: ActionKind::Script { scripts: vec![" ".into()], mode: RunMode::Task } }),
            "Name at least one script, such as dev."
        );
        assert_eq!(
            refuse(ProjectAction { id: String::new(), title: "Link".into(), kind: ActionKind::Url { url: "localhost:3000".into() } }),
            "Enter a full link, such as https://… or vscode://…"
        );
        assert_eq!(
            refuse(ProjectAction { id: String::new(), title: "App".into(), kind: ActionKind::Open { bundle_id: String::new(), app_path: String::new() } }),
            "Choose an app."
        );
    }

    #[test]
    fn deleting_an_action_clears_its_keys() {
        let store = ActionsStore::load(&temp_dir("delete"), seeded);
        store.delete("reveal").unwrap();
        let config = store.snapshot();
        assert!(config.action("reveal").is_none());
        assert!(!config.bindings.contains_key("cmd+enter"));
        assert_eq!(store.delete("reveal").unwrap_err(), "That action no longer exists.");
    }

    #[test]
    fn keys_bind_by_any_spelling_to_real_actions_only() {
        let store = ActionsStore::load(&temp_dir("bind"), seeded);
        store.bind("Option+Return", Some("reveal")).unwrap();
        assert_eq!(store.snapshot().bindings.get("alt+enter").map(String::as_str), Some("reveal"));
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
        store.reorder(&[custom.id.clone(), "missing".into()]).unwrap();
        let ids: Vec<String> = store.snapshot().actions.into_iter().map(|action| action.id).collect();
        assert_eq!(ids, vec![custom.id, "open-finder".to_string(), "reveal".to_string()]);
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
        assert_eq!(config.bindings.get("cmd+enter").map(String::as_str), Some("reveal"));
    }

    #[test]
    fn preferences_and_reset_are_saved() {
        let dir = temp_dir("prefs");
        let store = ActionsStore::load(&dir, seeded);
        store.set_preferences("warp".into(), RunMode::Task, false, true).unwrap();
        let config = ActionsStore::load(&dir, || panic!()).snapshot();
        assert_eq!((config.terminal.as_str(), config.script_mode, config.notify_task_finished), ("warp", RunMode::Task, false));
        store.reset(seeded);
        assert_eq!(ActionsStore::load(&dir, || panic!()).snapshot(), seeded());
    }
}
```

`catalog.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::super::model::ActionKind;
    use super::*;

    const FINDER: &str = "/System/Library/CoreServices/Finder.app";

    fn installed(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(id, path)| (id.to_string(), path.to_string())).collect()
    }

    #[test]
    fn installed_apps_and_clis_become_presets() {
        let apps = installed(&[
            ("com.apple.finder", FINDER),
            ("com.microsoft.VSCode", "/Applications/Visual Studio Code.app"),
            ("dev.warp.Warp-Stable", "/Applications/Warp.app"),
            ("com.apple.Terminal", "/System/Applications/Utilities/Terminal.app"),
            ("com.openai.codex", "/Applications/ChatGPT.app"),
            ("org.example.unknown", "/Applications/Unknown.app"),
        ]);
        let catalog = build(&apps, &|command| {
            (command == "claude").then(|| PathBuf::from("/Users/me/.local/bin/claude"))
        });
        let ids: Vec<&str> = catalog.presets.iter().map(|preset| preset.id.as_str()).collect();
        assert_eq!(&ids[..2], ["open-finder", "reveal"]);
        for id in ["open-vscode", "open-codex", "cli-claude", "run-project", "open-warp", "copy-path"] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        assert!(!ids.iter().any(|id| id.contains("unknown")));
        assert!(!ids.contains(&"cli-codex"));
        assert_eq!(catalog.default_terminal, "warp");
        assert!(catalog.terminal("warp").unwrap().runs_commands);
        let codex = catalog.apps.iter().find(|app| app.bundle_id == "com.openai.codex").unwrap();
        assert_eq!((codex.name.as_str(), codex.app_path.as_str()), ("Codex", "/Applications/ChatGPT.app"));
        let claude = catalog.presets.iter().find(|preset| preset.id == "cli-claude").unwrap();
        assert_eq!(claude.title, "Claude Code");
        assert_eq!(
            claude.kind,
            ActionKind::Terminal { command: "claude".into(), terminal: String::new(), cwd: String::new() }
        );
        assert_eq!(catalog.clis[0].path, "/Users/me/.local/bin/claude");
    }

    #[test]
    fn finder_is_always_offered_and_terminal_is_the_fallback() {
        let catalog = build(&[], &|_| None);
        assert_eq!(catalog.presets[0].id, "open-finder");
        assert_eq!(catalog.default_terminal, "terminal");
    }

    #[test]
    fn first_run_keeps_todays_keys() {
        let catalog = build(&installed(&[("com.apple.finder", FINDER)]), &|_| None);
        let config = default_config(&catalog);
        assert_eq!(config.bindings.get("enter").map(String::as_str), Some("open-finder"));
        assert_eq!(config.bindings.get("cmd+enter").map(String::as_str), Some("reveal"));
        assert_eq!(config.actions, catalog.presets);
    }

    #[test]
    fn bundle_info_reads_a_real_app() {
        let (id, name) = bundle_info(Path::new(FINDER)).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("com.apple.finder", "Finder"));
        assert!(bundle_info(Path::new("/nonexistent.app")).is_none());
    }
}
```

`scripts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("samlu-scripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, contents) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn pairs(scripts: &[DetectedScript]) -> Vec<(String, String)> {
        scripts.iter().map(|script| (script.id.clone(), script.command.clone())).collect()
    }

    #[test]
    fn package_scripts_keep_file_order_and_follow_the_lockfile() {
        let root = project("bun", &[
            ("package.json", r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest","bad":3}}"#),
            ("bun.lock", ""),
        ]);
        assert_eq!(pairs(&detect(&root)), vec![
            (":package.json:dev".to_string(), "bun run dev".to_string()),
            (":package.json:build".to_string(), "bun run build".to_string()),
            (":package.json:test".to_string(), "bun run test".to_string()),
        ]);
        for (lockfile, runner) in [("pnpm-lock.yaml", "pnpm run dev"), ("yarn.lock", "yarn run dev"), ("package-lock.json", "npm run dev")] {
            let root = project(lockfile, &[("package.json", r#"{"scripts":{"dev":"x"}}"#), (lockfile, "")]);
            assert_eq!(detect(&root)[0].command, runner);
        }
    }

    #[test]
    fn manifests_in_subfolders_are_found_and_dependency_folders_skipped() {
        let root = project("tauri", &[
            ("src-tauri/Cargo.toml", "[package]\nname = \"samlu\"\n"),
            ("node_modules/left-pad/package.json", r#"{"scripts":{"dev":"x"}}"#),
            (".hidden/package.json", r#"{"scripts":{"dev":"x"}}"#),
            ("workspace/Cargo.toml", "[workspace]\nmembers = []\n"),
        ]);
        let scripts = detect(&root);
        assert_eq!(pairs(&scripts), vec![
            ("src-tauri:Cargo.toml:run".to_string(), "cargo run".to_string()),
            ("src-tauri:Cargo.toml:test".to_string(), "cargo test".to_string()),
            ("src-tauri:Cargo.toml:build".to_string(), "cargo build".to_string()),
        ]);
        assert_eq!(scripts[0].cwd, "src-tauri");
        assert_eq!(scripts[0].source, "Cargo.toml");
    }

    #[test]
    fn makefile_targets_skip_variables_patterns_and_specials() {
        let root = project("make", &[(
            "Makefile",
            "CC := gcc\nbuild:\n\tcc main.c\n.PHONY: build test\ntest: build\n\t./t\n%.o: %.c\n\tcc\nall clean:\n\techo\nbuild:\n",
        )]);
        let names: Vec<String> = detect(&root).into_iter().map(|script| script.command).collect();
        assert_eq!(names, ["make build", "make test", "make all", "make clean"]);
    }

    #[test]
    fn justfile_recipes_skip_settings_aliases_and_private_ones() {
        let root = project("just", &[(
            "justfile",
            "set shell := [\"zsh\", \"-c\"]\ndefault:\n  just --list\nbuild target='debug':\n  cargo build\n@test *args:\n  cargo test\nalias b := build\n_hidden:\n  true\n",
        )]);
        let names: Vec<String> = detect(&root).into_iter().map(|script| script.command).collect();
        assert_eq!(names, ["just default", "just build", "just test"]);
    }

    #[test]
    fn other_ecosystems_are_recognised() {
        let root = project("mixed", &[
            ("go.mod", "module x\n"),
            ("deno.json", r#"{"tasks":{"start":"deno run main.ts"}}"#),
            ("pyproject.toml", "[project.scripts]\nserve = \"app:main\"\n"),
            ("uv.lock", ""),
            ("compose.yaml", "services: {}\n"),
        ]);
        let commands: Vec<String> = detect(&root).into_iter().map(|script| script.command).collect();
        for expected in ["go run .", "go test ./...", "deno task start", "uv run serve", "docker compose up"] {
            assert!(commands.contains(&expected.to_string()), "{expected} missing from {commands:?}");
        }
    }

    #[test]
    fn empty_and_malformed_projects_have_no_scripts() {
        assert!(detect(&project("empty", &[])).is_empty());
        assert!(detect(&project("broken", &[("package.json", "{"), ("Cargo.toml", "[[[")])).is_empty());
    }

    #[test]
    fn pick_follows_name_order_then_prefers_the_root() {
        let root = project("pick", &[
            ("package.json", r#"{"scripts":{"dev":"vite","run":"node ."}}"#),
            ("api/Cargo.toml", "[package]\nname = \"api\"\n"),
        ]);
        let scripts = detect(&root);
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(pick(&scripts, &names(&["dev", "run"])).unwrap().id, ":package.json:dev");
        assert_eq!(pick(&scripts, &names(&["run"])).unwrap().id, ":package.json:run");
        assert_eq!(pick(&scripts, &names(&["test"])).unwrap().id, "api:Cargo.toml:test");
        assert!(pick(&scripts, &names(&["serve"])).is_none());
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cd $SCRATCH/crates/actions-data && CARGO_TARGET_DIR=$SCRATCH/target cargo test --offline`
Expected: the new tests FAIL against the stubs (model tests pass).

- [ ] **Step 3: Implement** `store.rs`, then `catalog.rs`, then `scripts.rs`, running the tests after each file.

- [ ] **Step 4: All green, lint-clean**

Run: `cd $SCRATCH/crates/actions-data && CARGO_TARGET_DIR=$SCRATCH/target cargo test --offline && CARGO_TARGET_DIR=$SCRATCH/target cargo clippy --all-targets --offline -- -D warnings && rustfmt --edition 2021 --check <your three files>`
Expected: PASS, no warnings, no diff.

- [ ] **Step 5: Report** what you built, test counts, any contract concern. Do not commit; the integrator commits.

---

### Task B: Shell environment, terminal launching, and the task runner

**Files:**
- Modify (replace stub bodies): `src-tauri/src/project_actions/shell.rs`, `terminal.rs`, `runner.rs`
- Test: `#[cfg(test)] mod tests` in each file
- Build harness: `$SCRATCH/crates/actions-exec`

**Interfaces:**
- Consumes: `super::model::{ProjectRef, TerminalApp, TaskSpec, ResolvedStep, TaskSummary, TaskDetail, TaskStatus}`
- Produces (exact):
  - `shell::ShellEnv { pub shell: PathBuf, pub vars: Vec<(String, String)> }`, `ShellEnv::path_dirs(&self) -> Vec<PathBuf>`
  - `shell::login_env() -> &'static ShellEnv`
  - `shell::quote(&str) -> String`, `shell::expand_command(&str, &ProjectRef) -> String`, `shell::expand_url(&str, &ProjectRef) -> String`
  - `terminal::run_in_terminal(terminal: &TerminalApp, command: &str, cwd: &Path, title: &str, work_dir: &Path) -> Result<(), String>`
  - `runner::RunnerEvent { Changed(TaskSummary), Log { id, lines }, Url { task, url }, Finished(TaskSummary) }`, `runner::EventSink`, `runner::EnvSource`
  - `TaskRunner::new(log_dir: PathBuf, env: EnvSource, sink: EventSink) -> Self`, `start`, `list`, `detail(id, max_lines)`, `stop`, `rerun`, `clear_finished`, `stop_all`
  - `runner::is_local_url(&str) -> bool`

**Behaviour, shell:**
- `login_env`: `OnceLock`. Shell = `$SHELL` if it names an executable, else `/bin/zsh`. Run `<shell> -ilc 'printf "\n__SAMLU_ENV__\n"; /usr/bin/env -0'` with stdin null, stderr discarded, its own process group; give up after 5 s (kill the group) and use the fallback. Parse with a private `parse_env_output(output: &[u8], marker: &str) -> Option<Vec<(String, String)>>` (bytes after the last marker line, split on NUL, `NAME=value`, values may contain newlines; drop `PWD`, `OLDPWD`, `SHLVL`, `_`). Fallback: Samlu's own environment with PATH extended by `/opt/homebrew/bin`, `/opt/homebrew/sbin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin`, `/usr/bin`, `/bin`, `/usr/sbin`, `/sbin` (deduplicated, existing order first).
- `quote`: unchanged if non-empty and every char is ASCII alphanumeric or one of `-_./:@%+=,`; otherwise `'…'` with `'` written as `'\''`; empty → `''`.
- `expand_command`: first collapse `"{path}"`, `'{path}'`, `"{name}"`, `'{name}'` to the bare placeholder, then replace `{path}`/`{name}` with `quote(value)`. Other braces untouched.
- `expand_url`: `{path}`/`{name}` percent-encoded per RFC 3986 unreserved set (`A-Za-z0-9-._~`), keeping `/` in `{path}`.

**Behaviour, terminal:**
- `work_dir` is created if missing. Supported `terminal.id`: `terminal`, `iterm` (a `.command` script opened with `/usr/bin/open -b <bundle_id> <script>`), `warp` (a launch configuration in `~/.warp/launch_configurations/samlu-<unique>.yaml` opened with `/usr/bin/open "warp://launch/<file name>"`; verify on this Mac which form Warp accepts and use that), `ghostty` (`/usr/bin/open -na <app_path> --args --working-directory=<cwd> -e <shell> -ilc <quoted command>; exec <shell> -il` or whatever form Ghostty's docs specify — best effort, not testable here). Anything else → `Err("Samlu can't run commands in <name> yet.")`.
- The `.command` script (private `command_script(shell: &Path, command: &str, cwd: &Path, title: &str) -> String`): `#!` + shell; `rm -f -- "$0"` (self-deleting); `cd -- <quoted cwd> || exit 1`; set the window title with `printf '\033]0;%s\007' <quoted title>`; run `<shell> -ilc <quoted command>` so aliases and `.zshrc` PATH apply; finish with `exec <shell> -il` so the window stays at a prompt in the folder. Written with mode 0755 under `work_dir`.
- The Warp YAML (private `warp_launch_config(name: &str, command: &str, cwd: &Path) -> String`): one window, one tab titled `name`, `layout: { cwd: <abs path>, commands: [ { exec: <command> } ] }`; every value YAML double-quoted with `\` and `"` escaped. Delete `samlu-*.yaml` files older than one hour in that folder on each run.
- Live check (allowed, keep it to one window per terminal): run `echo "Samlu terminal test — you can close this window"` in Terminal and in Warp through `run_in_terminal`, using a `#[test] #[ignore]` test you run explicitly with `-- --ignored`, and confirm the window opened in the right folder by having the command also `pwd > /tmp/samlu-terminal-check-<terminal>.txt` and reading that file.

**Behaviour, runner:**
- Ids `t-<hex millis><hex counter>`. Each run keeps its `TaskSpec`, a summary, a ring buffer of the last 2,000 output lines, and appends every line to `log_dir/<id>.log` (folder created; at most 30 runs kept — when a 31st starts, the oldest finished run is forgotten and its log deleted).
- A step runs `<env.shell> -c <command>` (non-interactive; the env already carries PATH) with `env_clear()` + the `EnvSource` vars + `spec.env`, `current_dir(step.cwd)`, stdin null, stdout and stderr piped, `process_group(0)` (`std::os::unix::process::CommandExt`). Before each step write a header line `▸ <title>` (also to the log). Read stdout and stderr on separate threads; each line goes through private `clean_line` (strip ANSI CSI/OSC escape sequences, keep the text after the last `\r`) and is recorded unless empty after trimming.
- URLs: private `find_urls(line) -> Vec<String>` finds `http://` and `https://` runs up to whitespace, `"`, `'`, `<`, `>`, `)`, `]`, `` ` ``; trailing `.,;:!?` trimmed. Each new URL (max 10 per run) is appended to `summary.urls` and emits `Url { task, url }`.
- `is_local_url`: host is `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]` or ends with `.localhost`.
- Events: `Changed` when a run starts and when each step starts; `Log { id, lines }` batched — flush at most every 100 ms and at step end; `Finished` once. Never call the sink while holding a lock.
- Exit: a step's non-zero exit (or death by signal, `exit_code` = 128 + signal) fails the run unless `continue_on_error`; with `continue_on_error` every step runs and the run is `Failed` if any step failed (exit code of the last failing step), else `Succeeded`. `step`/`step_title` point at the failing step for a failed run.
- `stop(id)`: `libc::killpg(pgid, SIGTERM)`, wait up to 3 s, then `SIGKILL`; remaining steps are skipped; status `Stopped`. Stopping a finished task is `Ok(())`. Unknown id → `Err("That task is gone.")`.
- `list`: running first, then by `started_at` descending. `detail(id, max_lines)`: summary + last `max_lines` lines. `rerun(id)`: starts the stored spec again (new id). `clear_finished`: forget non-running runs (keep their log files). `stop_all`: stop every running run, waiting at most 3 s in total before SIGKILL.

- [ ] **Step 1: Write the failing tests** (add more as needed; these are required)

`shell.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectRef {
        ProjectRef { name: "My \"App\"".into(), path: "/Users/me/My \"App\" $dev".into() }
    }

    #[test]
    fn quoting_leaves_plain_words_alone() {
        assert_eq!(quote("/Users/me/code/app-1.0"), "/Users/me/code/app-1.0");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn placeholders_become_exactly_one_shell_word() {
        let project = project();
        assert_eq!(expand_command("ctun -l {path} start", &project), r#"ctun -l '/Users/me/My "App" $dev' start"#);
        assert_eq!(expand_command("code \"{path}\"", &project), r#"code '/Users/me/My "App" $dev'"#);
        assert_eq!(expand_command("echo '{name}' {other}", &project), r#"echo 'My "App"' {other}"#);
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(expand_command("printf '%s|' {path} {name}", &project))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "/Users/me/My \"App\" $dev|My \"App\"|");
    }

    #[test]
    fn urls_get_percent_encoded_placeholders() {
        let project = ProjectRef { name: "My App".into(), path: "/Users/me/My App".into() };
        assert_eq!(
            expand_url("https://x.dev/open?dir={path}&n={name}", &project),
            "https://x.dev/open?dir=/Users/me/My%20App&n=My%20App"
        );
    }

    #[test]
    fn env_output_is_read_after_the_marker() {
        let raw = b"The operation couldn't be completed. Unable to locate a Java Runtime.\n__SAMLU_ENV__\nPATH=/a:/b\0HOME=/h\0MULTI=x\ny\0PWD=/tmp\0";
        let vars = parse_env_output(raw, "__SAMLU_ENV__").unwrap();
        assert_eq!(vars, vec![
            ("PATH".to_string(), "/a:/b".to_string()),
            ("HOME".to_string(), "/h".to_string()),
            ("MULTI".to_string(), "x\ny".to_string()),
        ]);
        assert!(parse_env_output(b"no marker here", "__SAMLU_ENV__").is_none());
    }

    #[test]
    fn the_login_environment_has_a_path() {
        let env = login_env();
        assert!(!env.path_dirs().is_empty());
        assert!(env.shell.is_absolute());
    }
}
```

`runner.rs` (helpers shown once; use them in every test):

```rust
#[cfg(test)]
mod tests {
    use super::super::model::{ProjectRef, ResolvedStep, TaskSpec, TaskStatus};
    use super::super::shell::ShellEnv;
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runner(name: &str) -> (Arc<TaskRunner>, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let env: EnvSource = Arc::new(|| ShellEnv { shell: "/bin/sh".into(), vars: std::env::vars().collect() });
        let sink: EventSink = Arc::new(move |event| {
            let name = match event {
                RunnerEvent::Changed(_) => "changed".to_string(),
                RunnerEvent::Log { .. } => "log".to_string(),
                RunnerEvent::Url { url, .. } => format!("url {url}"),
                RunnerEvent::Finished(task) => format!("finished {:?}", task.status),
            };
            seen.lock().unwrap().push(name);
        });
        (Arc::new(TaskRunner::new(folder(name).join("logs"), env, sink)), events)
    }

    fn spec(name: &str, commands: &[&str], continue_on_error: bool) -> TaskSpec {
        let cwd = folder(&format!("{name}-cwd"));
        TaskSpec {
            title: name.into(),
            project: ProjectRef { name: "demo".into(), path: cwd.to_string_lossy().to_string() },
            steps: commands
                .iter()
                .map(|command| ResolvedStep { title: command.to_string(), command: command.to_string(), cwd: cwd.clone() })
                .collect(),
            continue_on_error,
            env: vec![("SAMLU_PROJECT_NAME".into(), "demo".into())],
        }
    }

    fn finish(runner: &TaskRunner, id: &str) -> TaskDetail {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let detail = runner.detail(id, 5_000).expect("task exists");
            if detail.summary.status != TaskStatus::Running {
                return detail;
            }
            assert!(Instant::now() < deadline, "task never finished");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn output_exit_codes_and_the_log_file_are_recorded() {
        let (runner, events) = runner("basic");
        let task = runner.start(spec("basic", &["echo hello \"$SAMLU_PROJECT_NAME\"", "echo oops >&2; exit 3"], false)).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        let detail = finish(&runner, &task.id);
        assert_eq!(detail.summary.status, TaskStatus::Failed);
        assert_eq!(detail.summary.exit_code, Some(3));
        assert_eq!(detail.summary.step, 1);
        assert!(detail.lines.contains(&"hello demo".to_string()));
        assert!(detail.lines.contains(&"oops".to_string()));
        let log = std::fs::read_to_string(&detail.summary.log_path).unwrap();
        assert!(log.contains("hello demo") && log.contains("oops"));
        let events = events.lock().unwrap();
        assert_eq!(events.first().map(String::as_str), Some("changed"));
        assert_eq!(events.iter().filter(|event| event.starts_with("finished")).count(), 1);
        assert!(events.iter().any(|event| event == "log"));
    }

    #[test]
    fn a_failed_step_stops_the_task_unless_told_to_continue() {
        let (runner, _) = runner("continue");
        let stopped = finish(&runner, &runner.start(spec("halt", &["false", "echo after"], false)).unwrap().id);
        assert_eq!(stopped.summary.status, TaskStatus::Failed);
        assert!(!stopped.lines.contains(&"after".to_string()));
        let kept_going = finish(&runner, &runner.start(spec("go", &["false", "echo after"], true)).unwrap().id);
        assert_eq!(kept_going.summary.status, TaskStatus::Failed);
        assert!(kept_going.lines.contains(&"after".to_string()));
        let fine = finish(&runner, &runner.start(spec("fine", &["true", "echo done"], false)).unwrap().id);
        assert_eq!(fine.summary.status, TaskStatus::Succeeded);
        assert_eq!(fine.summary.exit_code, Some(0));
    }

    #[test]
    fn stop_kills_the_whole_process_group() {
        let (runner, events) = runner("stop");
        let spec = spec("stop", &["sleep 30 & echo $! > child.pid; wait"], false);
        let pid_file = spec.steps[0].cwd.join("child.pid");
        let task = runner.start(spec).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&pid_file).map(|pid| pid.trim().is_empty()).unwrap_or(true) {
            assert!(Instant::now() < deadline, "child never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let child: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        runner.stop(&task.id).unwrap();
        assert_eq!(finish(&runner, &task.id).summary.status, TaskStatus::Stopped);
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(unsafe { libc::kill(child, 0) }, 0, "grandchild {child} survived");
        assert!(events.lock().unwrap().contains(&"finished Stopped".to_string()));
        assert!(runner.stop(&task.id).is_ok());
        assert_eq!(runner.stop("nope").unwrap_err(), "That task is gone.");
    }

    #[test]
    fn urls_are_collected_once_each() {
        let (runner, events) = runner("urls");
        let task = runner.start(spec("urls", &[
            "echo 'Your quick Tunnel: https://abc-def.trycloudflare.com.'; echo again https://abc-def.trycloudflare.com; echo '  Local: http://localhost:3000/'",
        ], false)).unwrap();
        let detail = finish(&runner, &task.id);
        assert_eq!(detail.summary.urls, vec!["https://abc-def.trycloudflare.com".to_string(), "http://localhost:3000/".to_string()]);
        assert_eq!(events.lock().unwrap().iter().filter(|event| event.starts_with("url ")).count(), 2);
    }

    #[test]
    fn lines_are_cleaned_of_escapes_and_progress() {
        assert_eq!(clean_line("\u{1b}[32m✓ ready\u{1b}[0m"), "✓ ready");
        assert_eq!(clean_line("10%\r50%\r100%"), "100%");
        assert_eq!(clean_line("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(find_urls("see (https://x.dev/a?b=1), then http://localhost:5173."), vec!["https://x.dev/a?b=1".to_string(), "http://localhost:5173".to_string()]);
    }

    #[test]
    fn local_urls_are_recognised() {
        for url in ["http://localhost:3000", "http://127.0.0.1:8080/x", "http://0.0.0.0:5173", "http://[::1]:3000", "http://app.localhost:1355"] {
            assert!(is_local_url(url), "{url}");
        }
        assert!(!is_local_url("https://abc.trycloudflare.com"));
    }

    #[test]
    fn output_is_capped_and_keeps_the_end() {
        let (runner, _) = runner("cap");
        let detail = finish(&runner, &runner.start(spec("cap", &["i=0; while [ $i -lt 5000 ]; do echo line $i; i=$((i+1)); done"], false)).unwrap().id);
        assert!(detail.lines.len() <= 2_000);
        assert_eq!(detail.lines.last().map(String::as_str), Some("line 4999"));
        assert_eq!(detail.summary.last_line, "line 4999");
    }

    #[test]
    fn rerun_list_and_clear() {
        let (runner, _) = runner("rerun");
        let first = runner.start(spec("again", &["echo once"], false)).unwrap();
        finish(&runner, &first.id);
        let second = runner.rerun(&first.id).unwrap();
        assert_ne!(second.id, first.id);
        let slow = runner.start(spec("slow", &["sleep 5"], false)).unwrap();
        assert_eq!(runner.list()[0].id, slow.id);
        runner.clear_finished();
        finish(&runner, &second.id);
        runner.clear_finished();
        assert_eq!(runner.list().iter().map(|task| task.id.clone()).collect::<Vec<_>>(), vec![slow.id.clone()]);
        runner.stop_all();
        assert_eq!(finish(&runner, &slow.id).summary.status, TaskStatus::Stopped);
    }

    #[test]
    fn the_sink_may_call_back_into_the_runner() {
        let slot: Arc<Mutex<Option<Arc<TaskRunner>>>> = Arc::new(Mutex::new(None));
        let inner = slot.clone();
        let env: EnvSource = Arc::new(|| ShellEnv { shell: "/bin/sh".into(), vars: std::env::vars().collect() });
        let sink: EventSink = Arc::new(move |_event| {
            if let Some(runner) = inner.lock().unwrap().clone() {
                let _ = runner.list();
            }
        });
        let runner = Arc::new(TaskRunner::new(folder("reentrant").join("logs"), env, sink));
        *slot.lock().unwrap() = Some(runner.clone());
        let task = runner.start(spec("reentrant", &["echo hi"], false)).unwrap();
        assert_eq!(finish(&runner, &task.id).summary.status, TaskStatus::Succeeded);
    }
}
```

`terminal.rs`: tests for `command_script` (shebang is the shell, `rm -f -- "$0"` present, `cd -- '/Users/me/My App'` for a path with a space, the command appears exactly once as a quoted `-ilc` argument, last line `exec <shell> -il`) and `warp_launch_config` (contains the absolute cwd and the command double-quoted with `"` and `\` escaped; parses back the same values with a tiny line-based check), plus the `#[ignore]` live checks above.

- [ ] **Step 2: Run them and see them fail**

Run: `cd $SCRATCH/crates/actions-exec && CARGO_TARGET_DIR=$SCRATCH/target cargo test --offline`
Expected: the new tests FAIL against the stubs.

- [ ] **Step 3: Implement** `shell.rs`, then `runner.rs`, then `terminal.rs`, testing after each.

- [ ] **Step 4: All green, lint-clean**

Run: `cd $SCRATCH/crates/actions-exec && CARGO_TARGET_DIR=$SCRATCH/target cargo test --offline && CARGO_TARGET_DIR=$SCRATCH/target cargo clippy --all-targets --offline -- -D warnings && rustfmt --edition 2021 --check <your three files>`, then the live terminal checks with `-- --ignored`.
Expected: PASS, no warnings; one test window each in Terminal and Warp opened in the right folder.

- [ ] **Step 5: Report** test counts, which Warp/Ghostty forms you verified, any contract concern. Do not commit.

---

### Task C: Launcher — keys, action panel, tasks

**Files:** Modify `public/launcher/launcher.js`, `public/launcher/launcher.css`, `public/launcher/launcher.html` (bump the `?v=4` cache busters to `?v=5`). Nothing else.

**Interfaces:** Consumes the Tauri API table above. Produces no API.

**Behaviour:**
- Views: `results` (today's list), `panel` (one project's actions), `tasks` (the `!` list, which is just `results` fed from `tasks_list`), `task` (one task's detail). Keep a small stack so Escape returns to where you came from, restoring the previous query, results and selection.
- Modifier tracking: replace `commandHeld` with `held = { ctrl, alt, shift, cmd }` from `keydown`/`keyup` on `window` (reset on `present`). `chordFrom(event)` returns `["ctrl","alt","shift","cmd"]` held (from `ctrlKey`, `altKey`, `shiftKey`, `metaKey`) + `"enter"`, joined with `+`.
- Bindings: on `present()`, `launcher_project_bindings` into a `Map` by chord.
- Project result, footer: the bound title for the chord being held, with its keys (`⌃⌥⇧⌘` symbols then `↩`); "Show Actions" when unbound. Also show `Actions ⇥` at the right of the footer whenever a project is selected (and in the panel, nothing extra).
- Project result, Return/click with modifiers: unbound → open the panel. Bound and `opensTask` → keep the launcher open, `launcher_run_project_chord({ path: target, chord, title, subtitle })`, then show the `task` view for `taskId`. Bound otherwise → show the footer busy label ("Opening…"), invoke, `hide()` on success; on error stay open and show the message in a notice line above the list (`role="status"`, cleared on the next keystroke).
- Non-project results keep today's behaviour exactly (⌘↩ = `launcher_reveal` for apps/files/content).
- Tab or ⌘K with a project selected → panel. Tab elsewhere does nothing (as today).
- Panel: `launcher_project_actions({ path, title })`. Search field shows a scope chip with the project name before the input (placeholder "Search actions"); typing filters rows case-insensitively by title, subtitle, name or command. Sections: "Actions" (row icon: the app icon via `launcher_app_icon(appPath)` when `appPath`, else a glyph per kind — `terminal` uses the existing terminal icon, `task`/`script` a play triangle, `url` a link, `copy_path` the clipboard icon, `reveal` the folder icon; right side: chord keycaps, or the kind label when unbound), "Scripts" (title = `name`, subtitle = `command`, right side = `cwd ? cwd + " · " + source : source`), "Running" (this project's tasks; right side = status word). Return runs: action → `launcher_run_project_action`; script → `launcher_run_script` with `mode` = `scriptMode` (⌘↩ = the other mode); task row → `task` view. Anything that `opensTask` (or a script run as `task`) shows the `task` view; the rest hide on success. Footer: "Run ↩" ("Open ↩" for `open`, "Copy ↩" for `copy_path`; for scripts, holding ⌘ shows "Run in Background ⌘↩" or "Run in Terminal ⌘↩") and "Back ⎋". Escape, ⇧Tab, or ⌫ on an empty field → back.
- `!` prefix → `tasks_list`, filtered by the text after `!` (title or project name); handled in `scheduleSearch` like `?` (never sent to `launcher_search`). Rows: title, subtitle `project · step N of M — lastLine` while running, `project · Finished in 1m 12s` / `Failed at <stepTitle> (exit 3)` / `Stopped`; right side status word. Return → `task` view. Empty state: "No tasks yet" / "Run an action that starts a task, or a script in the background." Legend gets `<kbd>!</kbd>Tasks`.
- Empty query: running tasks (from `tasks_list`) appear first under a "Running" section, before "Recent".
- Task view: `task_detail({ id })`. Shows title · project, status (word plus a dot; never color alone), elapsed time ticking each second while running, "Step 2 of 3 · Test", URL chips (click = `task_url({ id, url, copy: false })`), and the log in a monospace block that follows the end unless the reader has scrolled up (keep at most 1,000 lines in the DOM). Keys: Return = open first URL; ⌘↩ = copy first URL (`copy: true`, then a brief "Copied" in the footer); ⌘. = `task_stop`; ⌘R = `task_rerun` then show the new task; ⌘O = `task_open_log`; Escape = back. Footer lists the two or three that apply plus "Back ⎋". The panel uses its full height (`MAX_BODY_H`) in this view.
- Live updates: `listen("tasks://changed")` refreshes the `!` list, the panel's Running section, the empty-query Running section, and the task view header; `listen("tasks://log")` appends lines in the task view for the matching id.
- Motion and style follow `launcher.css` conventions: no animation on selection or list changes; tokens from `presence.css`; works in light and dark; reduced-motion safe.

- [ ] **Step 1:** Read `launcher.js`, `launcher.css`, `launcher.html`, `docs/SAMLU_DESIGN_SYSTEM.md` (Launcher motion), and the API table.
- [ ] **Step 2:** Implement the views, keys, footer and events above.
- [ ] **Step 3:** `node --check public/launcher/launcher.js` passes; no `console.log` left; every `invoke` has a `catch`.
- [ ] **Step 4:** Report what changed, any API gap you hit (do not edit Rust), and anything you could not verify. Do not commit; do not open the browser pane (the integrator verifies visually).

---

### Task D: Settings › Projects

**Files:** Modify `public/index.html`, `public/settings.js`, `public/settings.css`. Nothing else.

**Interfaces:** Consumes the settings rows of the Tauri API table. Produces no API.

**Behaviour:**
- Sidebar: a "Projects" nav item (folder-with-play icon in the existing 16×16 stroke style) between Launcher and About; `PANES` gains `"projects"`; `loadPane("projects")` loads it; the existing `settings://tab` listener reaches it with payload `"projects"`.
- Pane `#pane-projects`, `data-title="Projects"`, groups in this order:
  1. **Keys** (`Saved` indicator id `projects-saved`): rows for ↩ Return, ⌘↩ Command-Return, ⌥↩ Option-Return, ⌃↩ Control-Return, ⇧↩ Shift-Return (label with keycaps rendered like other shortcut labels). Each control is a `<select data-chord="…">`: first option value `""` "Show Actions", then every action in config order. Change → `set_project_action_binding({ chord, actionId: value || null })`, re-render from the returned config, `showSaved("projects-saved")`. Footnote: "Tab on a project shows every action and the project's scripts."
  2. **Actions**: modeled on the Snippets group (list beside an editor). Group head has an "Add Action" `<select>` (first option "Add Action…" as a placeholder) with optgroups: "Apps" (catalog presets of kind `open` not already added), "Agents and tools" (presets of kind `terminal` not added), "Project" (presets `reveal`, `copy_path`, `script` not added), "Custom" (`new:open` "Open with an App…", `new:terminal` "Command in a Terminal…", `new:task` "Background Task…", `new:script` "Project Script…", `new:url` "Link…"). Picking a preset saves it at once (`save_project_action({ action: preset })`) and selects it; picking a custom kind opens the editor with a blank draft (id `""`). The select resets to its placeholder after each pick. A "Restore Defaults…" text button (confirmAction "Restore default actions?" / "Your actions and keys go back to what Samlu found installed." / "Restore") → `reset_project_actions`.
     - List rows: kind glyph, title, a short subtitle (app name, command, steps, scripts or link), bound keycaps on the right. Selected row highlighted like snippets.
     - Editor: Title; Kind (`<select>`: Open in App, Show in Finder, Copy Path, Link, Terminal Command, Background Task, Project Script) — changing it swaps the fields below, keeping the title. Fields: **open** → App `<select>` of `catalog.apps` (value = bundleId; the action's current app is kept as an option even if not installed, labelled "<name> (not installed)") plus a final "Choose App…" option that calls `choose_application()` and adopts its `{ name, bundleId, appPath }` (cancel restores the previous choice). **url** → Link (monospace). **terminal** → Command (monospace, required), Terminal `<select>` (`""` = "Default (<name of catalog.defaultTerminal>)", then terminals with `runsCommands`), Folder (optional, placeholder "Project folder"). **task** → ordered steps, each with Title, Command (monospace), Folder, and Remove; "Add Step"; switch "Keep going when a step fails". **script** → Scripts (comma-separated, placeholder "dev, start, run"), Run in segmented `<select data-variant="segmented">` Terminal / Background. **reveal, copy_path** → a one-line note.
     - Help note under the editor: "{path} and {name} stand for the project. Commands start in the project folder."
     - Editor buttons: Delete… (confirmAction), Move Up, Move Down (→ `reorder_project_actions`), and the pane's single primary button "Save Action". Save sends the whole action (`save_project_action`), shows the returned config, keeps the saved action selected, `showSaved`. Backend errors → `toast(message, true)`.
  3. **Running**: Terminal for commands (`<select>`: `""` "Default (<name>)" + terminals with `runsCommands`), Scripts run in (segmented: Terminal / Background), switch "Notify when a task finishes", switch "Notify when a task shares a link" (detail: "Like a tunnel URL. Local addresses are skipped."). Any change → `set_project_action_preferences({ terminal, scriptMode, notifyTaskFinished, notifyTaskUrl })` + `showSaved`. Footnote listing what was found: "Found <CLI names> · <terminal names>." or "No agent CLIs found on your PATH."
  4. **Folders**: move the existing "Projects" group (indexed and excluded folders, Reindex, its footnote and the `project-count` pill) out of the Launcher pane into this pane unchanged — same element ids — and retitle it "Folders". Make sure its data loads when the Projects pane opens (factor the folder loading out of `loadLauncherSettings` into its own function called by both panes as needed).
- Launcher pane, Search Tools list: replace the `⌘↩ Show in Finder` row with `⇥ Project actions` / `Tab on a project`, and add `! Tasks` / `! dev server`.
- Every `<select>` you create dynamically must be enhanced the way `samlu-select.js` expects (read it: `window.SamluSelect.enhance(select)` or its MutationObserver behaviour).
- Styling uses the existing row/group/field/button classes and tokens; new CSS only for the steps editor, the actions list glyphs and keycaps. Light and dark both.

- [ ] **Step 1:** Read `index.html` (General/Launcher panes), `settings.js` (helpers, snippets editor, shortcut keycaps, `loadPane`), `settings.css`, `samlu-select.js`, `docs/SAMLU_DESIGN_SYSTEM.md` (Control center layout).
- [ ] **Step 2:** Implement the pane, the moved Folders group and the Search Tools edits.
- [ ] **Step 3:** `node --check public/settings.js` passes; no `console.log`; every `invoke` handles errors.
- [ ] **Step 4:** Report what changed, any API gap (do not edit Rust), and what you could not verify. Do not commit; do not open the browser pane.

---

### Task E: Integration (integrator)

**Files:** `src-tauri/src/project_actions/mod.rs`, `src-tauri/src/lib.rs`, docs.

- [ ] **Step 1:** Review each agent's diff against this plan and the spec; run the scratch crate tests again.
- [ ] **Step 2:** `cd src-tauri && CARGO_TARGET_DIR=<shared> cargo fmt --all --check && cargo clippy --all-targets --offline -- -D warnings && cargo test --offline` — fix integration errors in `mod.rs`; send contract-level problems back to the owning agent.
- [ ] **Step 3:** Add a `mod.rs` test for `open_with` refusing an unknown bundle id and a missing app path (Review Focus 4).
- [ ] **Step 4:** Update `README.md` (Developer launcher section), `PRODUCT.md` (launcher surface), `docs/SAMLU_DESIGN_SYSTEM.md` (launcher action panel and tasks), and the spec's API table (`launcher_run_project_chord`, `task_url`).

### Task F: Verification (integrator)

- [ ] **Step 1:** Serve `public/` with a scratch mock of `window.__TAURI__` (invoke handlers returning the JSON shapes above, events firing) and check the launcher views and the Projects pane in the browser pane, light and dark, with screenshots.
- [ ] **Step 2:** Run real actions through the scratch crates or the app: an `open` action, a terminal action in Warp and Terminal, a script detected in this repo, and a task with a URL.
- [ ] **Step 3:** Report results; offer to commit.

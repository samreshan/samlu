# Project Actions and Tasks — design

Date: 2026-10-15
Status: Approved 2026-10-15

## Context

The launcher finds projects (Git repositories under the indexed folders, plus
folders agents recently worked in). Today a project result does two fixed
things: Return opens the folder in Finder and Command-Return shows it in
Finder (`launcher.js` → `launcher_activate` / `launcher_reveal`).

The request: make the launcher the one place to act on an agentic project.
Every key on a project result is configurable — Return, ⌘↩, ⌥↩, ⌃↩ (and ⇧↩) —
and an action can open the project in any app (Finder, VS Code, Antigravity,
Antigravity IDE, the Claude and Codex apps), start an agent CLI in a terminal
(Claude Code, Codex, Gemini, Antigravity's `agy`), run a command such as the
user's `ctun` (Cloudflare quick tunnel plus dev-server CORS setup), run the
project's own program, or run a set of steps that Samlu tracks.

What the user said: configurable app per key; Return configurable separately;
⌥↩ and ⌃↩ for more cases; run the program the project has; actions can be a
ctun-like command, a tracked set of tasks, or opening something in an app;
highly configurable. Assumptions (open to correction): projects only, not apps
or files; ⇧↩ is offered as a fifth key; commands run in the project folder.

## Goals

1. Each key on a project result runs a configurable **action**.
2. A per-project **action panel** (Tab or ⌘K) lists every action, the
   project's detected scripts, and its running tasks, so nothing depends on
   remembering keys.
3. Action kinds: open in app, show in Finder, copy path, open URL, run in a
   terminal, run as a tracked background task, run the project's own script.
4. **Tracked tasks**: sequential steps with live output, status, URLs found in
   the output, stop and rerun, and a notification when they finish.
5. A **Projects** pane in Settings to bind keys and create, edit, reorder and
   delete actions, starting from presets for what is installed.
6. Existing users see no change until they configure something: Return still
   opens the folder in Finder and ⌘↩ still shows it in Finder.

## Non-goals (this release)

- Actions on apps, files or snippets.
- Showing task results on the Samlu Island (native notifications only).
- Persisting task history across restarts (logs are written to disk; the list
  of runs lives in memory).
- Scheduling, triggers, or running tasks without the launcher.
- Windows support for the new execution paths.

## Decisions

| Topic | Decision |
|---|---|
| Scope | Project results only |
| Keys | `enter`, `cmd+enter`, `alt+enter`, `ctrl+enter`, `shift+enter`; modifiers written in ⌃⌥⇧⌘ order (`ctrl`, `alt`, `shift`, `cmd`) |
| Unbound key | Opens the action panel instead of doing nothing |
| Defaults | ↩ = Open in Finder, ⌘↩ = Show in Finder (today's behaviour); other keys unbound |
| App identity | Bundle ID first (`open -b`), stored app path as fallback and for the icon |
| Seeding | First run seeds one action per installed preset (apps and CLIs found) plus generic ones |
| Shell environment | Resolved once from an interactive login shell (`$SHELL -ilc`); `.zshrc` is where Homebrew and `~/.local/bin` join PATH on this Mac |
| Path in commands | `{path}` / `{name}` expand to single-quoted shell words; `SAMLU_PROJECT_PATH` / `SAMLU_PROJECT_NAME` are also set; never spliced unquoted |
| Terminals | Terminal (`.command` file), Warp (launch configuration), iTerm and Ghostty best-effort; default is the first installed of Warp, Ghostty, iTerm, Terminal |
| Scripts default | Run in a terminal (a visible tab you can ⌃C); background is per action or ⌘↩ in the panel |
| Task logs | Ring buffer in memory (2,000 lines) plus `app_data/tasks/<id>.log`; 30 most recent runs kept |
| Quit | Samlu stops its running tasks (process groups) when it quits |
| Settings home | New **Projects** pane; the indexed-folders group moves there from Launcher |

## Experience

### Launcher, project result selected

- The footer names what Return does ("Open in Finder ↩"). Holding ⌘, ⌥, ⌃ or ⇧
  switches it to that key's action ("Claude Code ⌥↩"), or "Show Actions" when
  that key is unbound.
- Return / ⌘↩ / ⌥↩ / ⌃↩ / ⇧↩ run the bound action and close the launcher. An
  action that starts a tracked task keeps the launcher open on that task's
  output instead.
- Tab or ⌘K opens the **action panel** for the project.

### Action panel

- The search field filters the panel; a chip shows the project name. Escape,
  ⇧Tab, or ⌫ on an empty field goes back to the results.
- Sections: **Actions** (configured order, app icons for app actions, the key
  each one is bound to on the right), **Scripts** (detected; subtitle is the
  command, kind label is its source such as `package.json`), **Running**
  (this project's tasks).
- Return runs the selection. On a script, Return uses the default script mode
  and ⌘↩ the other one (terminal ↔ background).

### Tasks (`!` prefix)

- `!` lists running tasks, then recent ones; text after `!` filters by task or
  project name. An empty search shows running tasks at the top, under
  "Running".
- Return on a task opens its detail: title, project, status and elapsed time,
  URLs found in the output, and the live log (monospace, follows the end until
  you scroll up).
- In the detail: Return opens the first URL, ⌘↩ copies it, ⌘. stops, ⌘R runs
  again, ⌘O opens the log file, Escape goes back.

### Settings › Projects

1. **Keys**: five rows (↩ ⌘↩ ⌥↩ ⌃↩ ⇧↩), each a dropdown of actions plus "Show
   Actions".
2. **Actions**: a list and editor, like Snippets. "Add" offers presets for
   what is installed (apps, agent CLIs) and blank actions of each kind. The
   editor shows fields for the chosen kind; tasks have an ordered step list.
   Actions can be reordered, deleted, or reset to defaults.
3. **Running**: terminal used for commands, how scripts run by default,
   notify when a task finishes, notify when a task prints a public URL.
4. **Folders**: the indexed and excluded project folders (moved from
   Launcher).

The Launcher pane's Search Tools list replaces "⌘↩ Show in Finder" with "⇥
Project actions" and adds "! Tasks".

## Action model

Stored in `project-actions.json` in the app data folder, the same pattern as
`launcher-config.json` and `snippets.json` (serde defaults, save on write).

```json
{
  "version": 1,
  "actions": [
    { "id": "open-finder", "title": "Open in Finder", "kind": "open", "bundleId": "com.apple.finder", "appPath": "/System/Library/CoreServices/Finder.app" },
    { "id": "reveal", "title": "Show in Finder", "kind": "reveal" },
    { "id": "open-vscode", "title": "Open in VS Code", "kind": "open", "bundleId": "com.microsoft.VSCode", "appPath": "/Applications/Visual Studio Code.app" },
    { "id": "cli-claude", "title": "Claude Code", "kind": "terminal", "command": "claude", "terminal": "", "cwd": "" },
    { "id": "run-project", "title": "Run Project", "kind": "script", "scripts": ["dev", "start", "run", "serve"], "mode": "terminal" },
    { "id": "custom-…", "title": "Tunnel with ctun", "kind": "terminal", "command": "ctun start", "terminal": "", "cwd": "" },
    { "id": "custom-…", "title": "Checks", "kind": "task", "continueOnError": false, "steps": [
      { "title": "Format", "command": "cargo fmt --all --check", "cwd": "src-tauri" },
      { "title": "Test", "command": "cargo test", "cwd": "src-tauri" }
    ] },
    { "id": "copy-path", "title": "Copy Path", "kind": "copy_path" },
    { "id": "custom-…", "title": "Open Localhost", "kind": "url", "url": "http://localhost:3000" }
  ],
  "bindings": { "enter": "open-finder", "cmd+enter": "reveal" },
  "terminal": "",
  "scriptMode": "terminal",
  "notifyTaskFinished": true,
  "notifyTaskUrl": true
}
```

- `terminal: ""` means the default terminal; otherwise a catalog terminal id
  (`terminal`, `warp`, `iterm`, `ghostty`).
- `cwd` (terminal action, task step) is relative to the project; empty is the
  project folder.
- `scripts` on a script action lists names in priority order; the first one
  the project has runs, root folder before subfolders.
- Placeholders in commands: `{path}` and `{name}` become single-quoted shell
  words (a template that already wraps them in quotes is normalised first).
  In URLs they are percent-encoded. Commands always start in the project
  folder (or `cwd`), so most need neither. Background tasks also get
  `SAMLU_PROJECT_PATH` and `SAMLU_PROJECT_NAME`; terminal commands rely on
  the placeholders, since the terminal starts its own shell.
- Deleting an action clears any key bound to it.

## Catalog and presets

Detection reads `CFBundleIdentifier` from apps in the standard folders
(`/Applications`, `/Applications/Utilities`, `/System/Applications`,
`~/Applications`, Finder) and looks for CLIs on the resolved PATH.

| Group | Presets (bundle ID or command) |
|---|---|
| Finder | Open in Finder, Show in Finder |
| Editors | VS Code, VS Code Insiders, Cursor, Windsurf, Zed, Antigravity, Antigravity IDE, Xcode, Sublime Text, Nova, JetBrains IDEs |
| Agent apps | Claude (`com.anthropic.claudefordesktop`), Codex (`com.openai.codex`) |
| Agent CLIs | `claude`, `codex`, `gemini`, `agy`, `opencode`, `aider`, `cursor-agent`, `amp`, `copilot`, `goose`, `lazygit` |
| Terminals | Terminal, Warp, iTerm, Ghostty, WezTerm, kitty, Alacritty (open at folder) |
| Git | GitHub Desktop, Fork, Tower, Sourcetree |
| Project | Run Project (script), Copy Path |

The Claude app is opened with the folder (`open -b … <path>`). Its
`claude-cli://` deep link format is not documented, so it is not used; a user
who knows a deep link can make a URL action.

## Script detection

The project root and its immediate subfolders (skipping hidden, dependency and
build folders; at most 12) are checked for:

| File | Scripts |
|---|---|
| `package.json` | every `scripts` entry in file order, run with the package manager its lockfile names (bun, pnpm, yarn, else npm) |
| `Cargo.toml` with `[package]` | run, test, build |
| `Makefile` | targets (no pattern rules or variables) |
| `justfile` | recipes |
| `go.mod` | run, test, build |
| `Package.swift` | run, test, build |
| `deno.json` | tasks |
| `pyproject.toml` | `[project.scripts]` and `[tool.poetry.scripts]` (via uv or poetry when locked); `manage.py` adds runserver |
| `compose.yaml` / `docker-compose.yml` | up, down |

Each script has an id (`<subfolder>:<source>:<name>`), name, command, folder
and source.

## Running actions

- **open**: `open -b <bundleId> <path>`, falling back to `open -a <appPath>`.
- **reveal / copy_path / url**: Finder reveal, clipboard, `open <url>`.
- **terminal**: opens a new window or tab in the chosen terminal, in the
  project folder, runs the command through the user's interactive shell (so
  aliases work), and leaves the shell open afterwards. Terminal runs a
  self-deleting `.command` script; Warp gets a launch configuration.
- **task**: starts a tracked task (below).
- **script**: finds the script, then runs it as a terminal command or a task.

## Task runner

- Each step runs `$SHELL -c <command>` with the resolved login environment
  plus `SAMLU_PROJECT_PATH` and `SAMLU_PROJECT_NAME`, in its own process
  group, stdin closed, stdout and stderr captured line by line (ANSI escapes
  stripped, carriage-return progress collapsed).
- Steps run in order. A failing step fails the task unless
  `continueOnError`. Stop sends SIGTERM to the process group, then SIGKILL
  after three seconds, and skips the remaining steps.
- Status: running, succeeded, failed, stopped. Each run records steps, exit
  code, start and finish times, the last line, and up to 10 URLs found in its
  output.
- Events: `tasks://changed` (a run's summary) and `tasks://log` (`{ id,
  lines }`, batched about every 100 ms) to every window.
- Notifications (each switchable): a task finished or failed; a task printed
  its first public URL (not localhost).
- Quitting Samlu stops running tasks.

## Tauri API

Settings:

| Command | Returns |
|---|---|
| `get_project_actions()` | `{ config, catalog }` |
| `save_project_action(action)` | config (empty id creates) |
| `delete_project_action(id)` | config |
| `reorder_project_actions(ids)` | config |
| `set_project_action_binding(chord, actionId?)` | config |
| `set_project_action_preferences(terminal, scriptMode, notifyTaskFinished, notifyTaskUrl)` | config |
| `reset_project_actions()` | config |
| `choose_application()` | `{ name, bundleId, appPath }` or null |

Launcher:

| Command | Returns |
|---|---|
| `launcher_project_bindings()` | `[{ chord, actionId, title, kind, opensTask }]` |
| `launcher_project_actions(path)` | `{ project, actions, scripts, scriptMode, tasks }` |
| `launcher_run_project_chord(path, chord, title?, subtitle?)` | runs the action bound to a key, returns `{ taskId? }` (an async command, so `launcher_activate` and its main-thread fast path stay as they are) |
| `launcher_run_project_action(path, actionId)` | `{ taskId? }` |
| `launcher_run_script(path, scriptId, mode?)` | `{ taskId? }` |
| `tasks_list()` / `task_detail(id)` | summaries / summary with `lines` |
| `task_stop(id)` / `task_rerun(id)` / `task_open_log(id)` / `tasks_clear_finished()` | — |
| `task_url(id, url, copy)` | opens or copies a link that appeared in that task's output (only those) |

Running a project action from the launcher records the project in launcher
history, so it keeps ranking by use.

## Code layout

New Rust module `src-tauri/src/project_actions/`:

| File | Purpose |
|---|---|
| `model.rs` | Shared serde types (the contract) |
| `store.rs` | Load, seed, save, edit, bind, reset |
| `catalog.rs` | Presets, installed-app and CLI detection |
| `scripts.rs` | Script detection |
| `shell.rs` | Login environment, quoting, placeholder expansion, PATH lookup |
| `terminal.rs` | Running a command in Terminal, Warp, iTerm, Ghostty |
| `runner.rs` | Tracked task runner (no Tauri dependency; events through a callback) |
| `mod.rs` | Tauri commands and glue: dispatch, events, notifications, history |

Frontend: `public/launcher/*` (keys, panel, tasks) and `public/index.html`,
`settings.js`, `settings.css` (Projects pane).

Every module except `mod.rs` depends only on `std`, crates already in
`Cargo.toml` (plus `libc`, already in the lock file) and its siblings, so each
can be compiled and tested on its own.

## Error handling

- A missing app, script, or terminal returns a short message the launcher
  shows in place ("Visual Studio Code isn't installed. Choose another app in
  Settings › Projects."); nothing fails silently.
- Shell environment resolution has a five-second timeout and falls back to the
  app's environment plus the common tool folders.
- A corrupt `project-actions.json` is kept as `.bak` and defaults are seeded.

## Testing

- Unit: config defaults and migration, binding validation, delete clearing
  bindings, seeding from a fake catalog, quoting and expansion (spaces,
  quotes, `$`), URL encoding, every script source with fixtures, Makefile and
  justfile parsing, terminal script and launch-configuration generation, the
  runner (output capture, exit codes, stop kills the group, continue on
  error, URL detection, ANSI stripping).
- Build: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`,
  `cargo test`.
- Manual: the launcher and Settings rendered with a mocked backend in the
  browser pane; a real run of a terminal action in Warp and Terminal; a task
  run end to end.

## Build constraint

The Mac has about 1.7 GB free and a debug build needs about 4 GB. Modules are
developed in small scratch crates that compile only their own dependencies;
the full app build waits until space is freed.

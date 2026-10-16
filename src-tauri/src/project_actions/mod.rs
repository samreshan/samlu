//! Project actions: what the launcher's keys do to a project, the per-project
//! action panel, and tracked background tasks.
//!
//! The submodules hold no Tauri code — the configuration store, the catalog of
//! apps and CLIs, script detection, the shell environment, terminal launching
//! and the task runner — so each can be built and tested on its own. This
//! file is the glue: Tauri commands, dispatching an action, turning runner
//! events into window events and notifications, and launcher history.

pub mod catalog;
pub mod model;
pub mod process_group;
pub mod runner;
pub mod scripts;
pub mod shell;
pub mod store;
pub mod terminal;

use model::{
    chord_label, normalize_chord, ActionKind, ActionView, ActionsConfig, BindingView, Catalog,
    ChosenApp, DetectedScript, ProjectAction, ProjectPanel, ProjectRef, ResolvedStep, RunMode,
    RunOutcome, SettingsView, TaskDetail, TaskLogEvent, TaskSpec, TaskStatus, TaskSummary, CHORDS,
};
use runner::{RunnerEvent, TaskRunner};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use store::ActionsStore;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::NotificationExt;

use crate::launcher::history::LauncherHistory;
use crate::launcher::search::LauncherResult;

/// Lines of output the launcher's task view starts with.
const DETAIL_LINES: usize = 400;

pub struct ProjectActions {
    data_dir: PathBuf,
    /// Loaded off the main thread: seeding it detects installed apps, which
    /// needs the login shell's PATH.
    store: OnceLock<ActionsStore>,
    catalog: Mutex<Option<Catalog>>,
    runner: TaskRunner,
    /// Generated terminal scripts and launch configurations.
    terminal_dir: PathBuf,
}

impl ProjectActions {
    pub fn new(app: &AppHandle, app_data_dir: &Path) -> Arc<Self> {
        let sink_app = app.clone();
        let runner = TaskRunner::new(
            app_data_dir.join("tasks"),
            Arc::new(|| shell::login_env().clone()),
            Arc::new(move |event| on_runner_event(&sink_app, event)),
        );
        Arc::new(Self {
            data_dir: app_data_dir.to_path_buf(),
            store: OnceLock::new(),
            catalog: Mutex::new(None),
            runner,
            terminal_dir: app_data_dir.join("terminal"),
        })
    }

    /// Resolves the login environment and loads the configuration in the
    /// background, so the first launcher action never waits for either.
    pub fn warm(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || {
            let _ = this.store();
        });
    }

    /// Stops running tasks; called when Samlu quits.
    pub fn shutdown(&self) {
        self.runner.stop_all();
    }

    fn store(&self) -> &ActionsStore {
        self.store.get_or_init(|| {
            ActionsStore::load(&self.data_dir, || {
                catalog::default_config(&self.catalog(false))
            })
        })
    }

    /// What is installed. Detected once, and again whenever Settings opens.
    fn catalog(&self, refresh: bool) -> Catalog {
        let mut cached = self.catalog.lock().unwrap();
        if refresh || cached.is_none() {
            *cached = Some(catalog::detect(&shell::login_env().path_dirs()));
        }
        cached.clone().unwrap_or_default()
    }

    fn run_action(
        &self,
        app: &AppHandle,
        action: &ProjectAction,
        project: &ProjectRef,
    ) -> Result<RunOutcome, String> {
        let config = self.store().snapshot();
        match &action.kind {
            ActionKind::Open {
                bundle_id,
                app_path,
            } => open_with(bundle_id, app_path, &project.path, &action.title),
            ActionKind::Reveal => crate::reveal_path(&project.path).map(|_| RunOutcome::default()),
            ActionKind::CopyPath => app
                .clipboard()
                .write_text(project.path.clone())
                .map(|_| RunOutcome::default())
                .map_err(|error| error.to_string()),
            ActionKind::Url { url } => {
                crate::open_path(&shell::expand_url(url, project)).map(|_| RunOutcome::default())
            }
            ActionKind::Terminal {
                command,
                terminal,
                cwd,
            } => {
                let folder = folder_in(project, cwd)?;
                self.run_in_terminal(
                    &config,
                    terminal,
                    &shell::expand_command(command, project),
                    &folder,
                    &format!("{} · {}", action.title, project.name),
                )
            }
            ActionKind::Task {
                steps,
                continue_on_error,
            } => {
                let steps = steps
                    .iter()
                    .map(|step| {
                        Ok(ResolvedStep {
                            title: if step.title.trim().is_empty() {
                                step.command.clone()
                            } else {
                                step.title.clone()
                            },
                            command: shell::expand_command(&step.command, project),
                            cwd: folder_in(project, &step.cwd)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                self.start_task(&action.title, project, steps, *continue_on_error)
            }
            ActionKind::Script {
                scripts: names,
                mode,
            } => {
                let detected = scripts::detect(Path::new(&project.path));
                let script = scripts::pick(&detected, names).ok_or_else(|| {
                    format!(
                        "{} has no {} script.",
                        project.name,
                        names.join(", ").trim_end_matches(", ")
                    )
                })?;
                self.run_script(&config, project, script, *mode)
            }
        }
    }

    fn run_script(
        &self,
        config: &ActionsConfig,
        project: &ProjectRef,
        script: &DetectedScript,
        mode: RunMode,
    ) -> Result<RunOutcome, String> {
        let folder = folder_in(project, &script.cwd)?;
        match mode {
            RunMode::Terminal => self.run_in_terminal(
                config,
                "",
                &script.command,
                &folder,
                &format!("{} · {}", script.name, project.name),
            ),
            RunMode::Task => self.start_task(
                &script.name,
                project,
                vec![ResolvedStep {
                    title: script.command.clone(),
                    command: script.command.clone(),
                    cwd: folder,
                }],
                false,
            ),
        }
    }

    fn run_in_terminal(
        &self,
        config: &ActionsConfig,
        terminal_id: &str,
        command: &str,
        folder: &Path,
        title: &str,
    ) -> Result<RunOutcome, String> {
        let catalog = self.catalog(false);
        let id = [terminal_id, config.terminal.as_str()]
            .into_iter()
            .find(|id| !id.is_empty())
            .unwrap_or(catalog.default_terminal.as_str());
        let Some(terminal) = catalog.terminal(id) else {
            return Err(
                "The terminal for this action isn't installed. Choose another in Settings › Projects."
                    .to_string(),
            );
        };
        if !terminal.runs_commands {
            return Err(format!(
                "Samlu can't run commands in {} yet. Choose {} in Settings › Projects.",
                terminal.name, COMMAND_TERMINAL_NAMES
            ));
        }
        terminal::run_in_terminal(terminal, command, folder, title, &self.terminal_dir)?;
        Ok(RunOutcome::default())
    }

    fn start_task(
        &self,
        title: &str,
        project: &ProjectRef,
        steps: Vec<ResolvedStep>,
        continue_on_error: bool,
    ) -> Result<RunOutcome, String> {
        if steps.is_empty() {
            return Err(format!("“{title}” has no steps to run."));
        }
        let summary = self.runner.start(TaskSpec {
            title: title.to_string(),
            project: project.clone(),
            steps,
            continue_on_error,
            env: vec![
                ("SAMLU_PROJECT_PATH".into(), project.path.clone()),
                ("SAMLU_PROJECT_NAME".into(), project.name.clone()),
            ],
        })?;
        Ok(RunOutcome {
            task_id: Some(summary.id),
        })
    }

    fn panel(&self, project: ProjectRef) -> ProjectPanel {
        let config = self.store().snapshot();
        let actions = config
            .actions
            .iter()
            .map(|action| ActionView {
                id: action.id.clone(),
                title: action.title.clone(),
                subtitle: subtitle_for(action),
                kind: action.kind.name().to_string(),
                app_path: match &action.kind {
                    ActionKind::Open { app_path, .. }
                        if !app_path.is_empty() && Path::new(app_path).exists() =>
                    {
                        Some(app_path.clone())
                    }
                    _ => None,
                },
                chords: config.chords_for(&action.id),
                opens_task: action.opens_task(),
            })
            .collect();
        let tasks = self
            .runner
            .list()
            .into_iter()
            .filter(|task| task.project.path == project.path)
            .collect();
        ProjectPanel {
            scripts: scripts::detect(Path::new(&project.path)),
            script_mode: config.script_mode,
            actions,
            tasks,
            project,
        }
    }
}

/* ------------------------------------------------------------ dispatch */

/// The terminals an action can run commands in, for messages.
#[cfg(not(windows))]
const COMMAND_TERMINAL_NAMES: &str = "Warp, Ghostty, iTerm or Terminal";
#[cfg(windows)]
const COMMAND_TERMINAL_NAMES: &str =
    "Windows Terminal, PowerShell, Command Prompt, WezTerm or Alacritty";

/// `open -b` (or `-a`) and wait for its answer, so a missing app is reported
/// instead of failing silently.
#[cfg(not(windows))]
fn open_with(
    bundle_id: &str,
    app_path: &str,
    target: &str,
    title: &str,
) -> Result<RunOutcome, String> {
    let attempts: [(&str, &str); 2] = [("-b", bundle_id), ("-a", app_path)];
    for (flag, app) in attempts {
        if app.is_empty() || (flag == "-a" && !Path::new(app).exists()) {
            continue;
        }
        let opened = Command::new("/usr/bin/open")
            .args([flag, app, target])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if opened {
            return Ok(RunOutcome::default());
        }
    }
    Err(format!(
        "The app for “{title}” isn't installed. Choose another in Settings › Projects."
    ))
}

/// Starts the app with the folder, so a missing app is reported instead of
/// failing silently. Terminals open a window at the folder.
#[cfg(windows)]
fn open_with(
    bundle_id: &str,
    app_path: &str,
    target: &str,
    title: &str,
) -> Result<RunOutcome, String> {
    let missing =
        || format!("The app for “{title}” isn't installed. Choose another in Settings › Projects.");
    if app_path.is_empty() || !catalog::is_present(Path::new(app_path)) {
        return Err(missing());
    }
    if let Some(terminal) = catalog::terminal_id(bundle_id) {
        terminal::open_folder(terminal, app_path, Path::new(target))?;
        return Ok(RunOutcome::default());
    }
    Command::new(app_path)
        .args(catalog::open_arguments(bundle_id, target))
        .spawn()
        .map(|_| RunOutcome::default())
        .map_err(|_| missing())
}

/// Opens the default terminal in the home folder, for the launcher's Open
/// Terminal command.
#[cfg(windows)]
pub fn open_terminal(app: &AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<Arc<ProjectActions>>()
        .ok_or_else(|| "Samlu is still starting.".to_string())?;
    let catalog = state.catalog(false);
    let terminal = catalog
        .terminal(&catalog.default_terminal)
        .ok_or_else(|| "No terminal is installed.".to_string())?;
    let home = dirs::home_dir().unwrap_or_default();
    terminal::open_folder(&terminal.id, &terminal.app_path, &home)
}

/// The project folder, or `relative` inside it.
fn folder_in(project: &ProjectRef, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim().trim_matches('/');
    let folder = if relative.is_empty() {
        PathBuf::from(&project.path)
    } else {
        Path::new(&project.path).join(relative)
    };
    if folder.is_dir() {
        Ok(folder)
    } else {
        Err(format!("{} has no folder “{relative}”.", project.name))
    }
}

/// The second line of an action in the panel.
fn subtitle_for(action: &ProjectAction) -> String {
    match &action.kind {
        ActionKind::Open {
            app_path,
            bundle_id,
        } => Path::new(app_path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| bundle_id.clone()),
        ActionKind::Reveal => model::FILE_MANAGER.into(),
        ActionKind::CopyPath => "Path to the clipboard".into(),
        ActionKind::Url { url } => url.clone(),
        ActionKind::Terminal { command, .. } => command.clone(),
        ActionKind::Task { steps, .. } => steps
            .iter()
            .map(|step| {
                if step.title.trim().is_empty() {
                    step.command.as_str()
                } else {
                    step.title.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join(" → "),
        ActionKind::Script { scripts, .. } => scripts.join(" · "),
    }
}

fn project_ref(path: &str, name: Option<String>) -> Result<ProjectRef, String> {
    let folder = Path::new(path);
    if !folder.is_dir() {
        return Err(format!("{path} is no longer there."));
    }
    let name = name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            folder
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| path.to_string());
    Ok(ProjectRef {
        name,
        path: path.to_string(),
    })
}

/// A project run from the launcher keeps ranking by use, the same as opening
/// it did.
fn record_project(app: &AppHandle, project: &ProjectRef, subtitle: Option<String>) {
    if let Some(history) = app.try_state::<Arc<LauncherHistory>>() {
        history.record_selection(&LauncherResult {
            id: format!("project:{}", project.path),
            kind: "project".into(),
            title: project.name.clone(),
            subtitle: subtitle.unwrap_or_default(),
            target: project.path.clone(),
            section: None,
        });
    }
}

/* ------------------------------------------------------- runner events */

fn on_runner_event(app: &AppHandle, event: RunnerEvent) {
    match event {
        RunnerEvent::Changed(task) => {
            let _ = app.emit("tasks://changed", &task);
        }
        RunnerEvent::Log { id, lines } => {
            let _ = app.emit("tasks://log", TaskLogEvent { id, lines });
        }
        RunnerEvent::Url { task, url } => {
            let _ = app.emit("tasks://changed", &task);
            // Only the first address the world can reach is worth a
            // notification; local dev-server addresses are expected.
            let first_public = task.urls.iter().find(|url| !runner::is_local_url(url));
            if first_public != Some(&url) || !preferences(app).notify_task_url {
                return;
            }
            notify(
                app,
                &format!("{} is live", task.title),
                &format!("{} · {url}", task.project.name),
            );
        }
        RunnerEvent::Finished(task) => {
            let _ = app.emit("tasks://changed", &task);
            if !preferences(app).notify_task_finished {
                return;
            }
            let elapsed = task
                .finished_at
                .unwrap_or(task.started_at)
                .saturating_sub(task.started_at);
            match task.status {
                TaskStatus::Succeeded => notify(
                    app,
                    &format!("{} finished", task.title),
                    &format!("{} · {}", task.project.name, duration_label(elapsed)),
                ),
                TaskStatus::Failed => notify(
                    app,
                    &format!("{} failed", task.title),
                    &match task.exit_code {
                        Some(code) => format!(
                            "{} · {} exited with {code}",
                            task.project.name, task.step_title
                        ),
                        None => format!("{} · {}", task.project.name, task.step_title),
                    },
                ),
                // Stopping is something you just did yourself.
                TaskStatus::Stopped | TaskStatus::Running => {}
            }
        }
    }
}

fn preferences(app: &AppHandle) -> ActionsConfig {
    app.try_state::<Arc<ProjectActions>>()
        .map(|state| state.store().snapshot())
        .unwrap_or_default()
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        log::warn!("failed to show task notification: {error}");
    }
}

/// 72_000 → "1m 12s".
fn duration_label(millis: u64) -> String {
    let seconds = millis / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/* ------------------------------------------------- commands: launcher */

type State<'a> = tauri::State<'a, Arc<ProjectActions>>;

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| error.to_string())?
}

/// What each key does to a project result, for the launcher footer.
#[tauri::command]
pub async fn launcher_project_bindings(state: State<'_>) -> Result<Vec<BindingView>, String> {
    let state = state.inner().clone();
    blocking(move || {
        let config = state.store().snapshot();
        Ok(CHORDS
            .iter()
            .filter_map(|chord| {
                config.bound(chord).map(|action| BindingView {
                    chord: chord.to_string(),
                    action_id: action.id.clone(),
                    title: action.title.clone(),
                    kind: action.kind.name().to_string(),
                    opens_task: action.opens_task(),
                })
            })
            .collect())
    })
    .await
}

/// Runs whatever `chord` is bound to. The launcher opens the action panel
/// itself for an unbound key, so an error here means the binding vanished.
#[tauri::command]
pub async fn launcher_run_project_chord(
    app: AppHandle,
    state: State<'_>,
    path: String,
    chord: String,
    title: Option<String>,
    subtitle: Option<String>,
) -> Result<RunOutcome, String> {
    let state = state.inner().clone();
    blocking(move || {
        let project = project_ref(&path, title)?;
        let chord = normalize_chord(&chord).ok_or_else(|| format!("Unknown key: {chord}"))?;
        let config = state.store().snapshot();
        let action = config.bound(&chord).cloned().ok_or_else(|| {
            format!(
                "Nothing is set for {}. Choose an action in Settings › Projects.",
                chord_label(&chord)
            )
        })?;
        let outcome = state.run_action(&app, &action, &project)?;
        record_project(&app, &project, subtitle);
        Ok(outcome)
    })
    .await
}

/// The action panel for one project.
#[tauri::command]
pub async fn launcher_project_actions(
    state: State<'_>,
    path: String,
    title: Option<String>,
) -> Result<ProjectPanel, String> {
    let state = state.inner().clone();
    blocking(move || Ok(state.panel(project_ref(&path, title)?))).await
}

#[tauri::command]
pub async fn launcher_run_project_action(
    app: AppHandle,
    state: State<'_>,
    path: String,
    action_id: String,
    title: Option<String>,
    subtitle: Option<String>,
) -> Result<RunOutcome, String> {
    let state = state.inner().clone();
    blocking(move || {
        let project = project_ref(&path, title)?;
        let action = state
            .store()
            .snapshot()
            .action(&action_id)
            .cloned()
            .ok_or_else(|| "That action no longer exists.".to_string())?;
        let outcome = state.run_action(&app, &action, &project)?;
        record_project(&app, &project, subtitle);
        Ok(outcome)
    })
    .await
}

/// Runs a detected script, in `mode` or the configured default.
#[tauri::command]
pub async fn launcher_run_script(
    app: AppHandle,
    state: State<'_>,
    path: String,
    script_id: String,
    mode: Option<RunMode>,
    title: Option<String>,
    subtitle: Option<String>,
) -> Result<RunOutcome, String> {
    let state = state.inner().clone();
    blocking(move || {
        let project = project_ref(&path, title)?;
        let detected = scripts::detect(Path::new(&project.path));
        let script = detected
            .iter()
            .find(|script| script.id == script_id)
            .ok_or_else(|| "That script is no longer in the project.".to_string())?;
        let config = state.store().snapshot();
        let outcome = state.run_script(
            &config,
            &project,
            script,
            mode.unwrap_or(config.script_mode),
        )?;
        record_project(&app, &project, subtitle);
        Ok(outcome)
    })
    .await
}

/* ---------------------------------------------------- commands: tasks */

#[tauri::command]
pub fn tasks_list(state: State<'_>) -> Vec<TaskSummary> {
    state.runner.list()
}

#[tauri::command]
pub fn task_detail(state: State<'_>, id: String) -> Option<TaskDetail> {
    state.runner.detail(&id, DETAIL_LINES)
}

#[tauri::command]
pub async fn task_stop(state: State<'_>, id: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || state.runner.stop(&id)).await
}

#[tauri::command]
pub async fn task_rerun(state: State<'_>, id: String) -> Result<TaskSummary, String> {
    let state = state.inner().clone();
    blocking(move || state.runner.rerun(&id)).await
}

/// Opens a task's full log in the default text editor.
#[tauri::command]
pub fn task_open_log(state: State<'_>, id: String) -> Result<(), String> {
    let task = state
        .runner
        .list()
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| "That task is gone.".to_string())?;
    #[cfg(not(windows))]
    let opened = Command::new("/usr/bin/open")
        .args(["-t", &task.log_path])
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string());
    // Windows opens `.log` files in Notepad unless the user chose otherwise.
    #[cfg(windows)]
    let opened = crate::open_path(&task.log_path);
    opened
}

#[tauri::command]
pub fn tasks_clear_finished(state: State<'_>) {
    state.runner.clear_finished();
}

/// Opens, or copies, a link a task printed. Only links that appeared in that
/// task's own output are accepted, so the page cannot open arbitrary URLs.
#[tauri::command]
pub fn task_url(
    app: AppHandle,
    state: State<'_>,
    id: String,
    url: String,
    copy: bool,
) -> Result<(), String> {
    let known = state
        .runner
        .list()
        .into_iter()
        .any(|task| task.id == id && task.urls.contains(&url));
    if !known {
        return Err("That link isn't in the task's output.".to_string());
    }
    if copy {
        app.clipboard()
            .write_text(url)
            .map_err(|error| error.to_string())
    } else {
        crate::open_path(&url)
    }
}

/* ------------------------------------------------- commands: settings */

#[tauri::command]
pub async fn get_project_actions(state: State<'_>) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || {
        let catalog = state.catalog(true);
        Ok(SettingsView {
            config: state.store().snapshot(),
            catalog,
        })
    })
    .await
}

#[tauri::command]
pub async fn save_project_action(
    state: State<'_>,
    action: ProjectAction,
) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        state.store().upsert(action)?;
        Ok(state.store().snapshot())
    })
    .await
}

#[tauri::command]
pub async fn delete_project_action(state: State<'_>, id: String) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        state.store().delete(&id)?;
        Ok(state.store().snapshot())
    })
    .await
}

#[tauri::command]
pub async fn reorder_project_actions(
    state: State<'_>,
    ids: Vec<String>,
) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        state.store().reorder(&ids)?;
        Ok(state.store().snapshot())
    })
    .await
}

#[tauri::command]
pub async fn set_project_action_binding(
    state: State<'_>,
    chord: String,
    action_id: Option<String>,
) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        let action_id = action_id.filter(|id| !id.is_empty());
        state.store().bind(&chord, action_id.as_deref())?;
        Ok(state.store().snapshot())
    })
    .await
}

#[tauri::command]
pub async fn set_project_action_preferences(
    state: State<'_>,
    terminal: String,
    script_mode: RunMode,
    notify_task_finished: bool,
    notify_task_url: bool,
) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        if !terminal.is_empty() {
            let catalog = state.catalog(false);
            match catalog.terminal(&terminal) {
                Some(found) if found.runs_commands => {}
                Some(found) => {
                    return Err(format!("Samlu can't run commands in {} yet.", found.name))
                }
                None => return Err("That terminal isn't installed.".to_string()),
            }
        }
        state.store().set_preferences(
            terminal,
            script_mode,
            notify_task_finished,
            notify_task_url,
        )?;
        Ok(state.store().snapshot())
    })
    .await
}

#[tauri::command]
pub async fn reset_project_actions(state: State<'_>) -> Result<ActionsConfig, String> {
    let state = state.inner().clone();
    blocking(move || {
        let catalog = state.catalog(true);
        state.store().reset(|| catalog::default_config(&catalog));
        Ok(state.store().snapshot())
    })
    .await
}

/// The Windows file picker, for an action that opens a project in an app
/// Samlu has no preset for.
#[cfg(windows)]
#[tauri::command]
pub async fn choose_application(app: AppHandle) -> Result<Option<ChosenApp>, String> {
    use tauri_plugin_dialog::DialogExt;
    blocking(move || {
        let Some(chosen) = app
            .dialog()
            .file()
            .set_title("Choose an app to open projects with")
            .add_filter("Apps", &["exe"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = chosen
            .into_path()
            .map_err(|error| format!("That doesn't look like an app: {error}"))?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .ok_or_else(|| "That doesn't look like an app.".to_string())?;
        let app_path = path.to_string_lossy().to_string();
        Ok(Some(ChosenApp {
            name,
            // Windows apps have no bundle identifier; the path stands in.
            bundle_id: app_path.clone(),
            app_path,
        }))
    })
    .await
}

/// The macOS app picker, for an action that opens a project in an app Samlu
/// has no preset for.
#[cfg(not(windows))]
#[tauri::command]
pub async fn choose_application() -> Result<Option<ChosenApp>, String> {
    blocking(|| {
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "POSIX path of (choose application with prompt \"Choose an app to open projects with\" as alias)",
            ])
            .output()
            .map_err(|error| format!("could not open the app picker: {error}"))?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return if error.contains("-128") || error.to_lowercase().contains("cancel") {
                Ok(None)
            } else {
                Err(format!("The app picker failed: {}", error.trim()))
            };
        }
        let app_path = String::from_utf8_lossy(&output.stdout)
            .trim()
            .trim_end_matches('/')
            .to_string();
        let (bundle_id, name) = catalog::bundle_info(Path::new(&app_path))
            .ok_or_else(|| "That doesn't look like an app.".to_string())?;
        Ok(Some(ChosenApp {
            name,
            bundle_id,
            app_path,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_label(4_200), "4s");
        assert_eq!(duration_label(72_000), "1m 12s");
        assert_eq!(duration_label(3_723_000), "1h 2m");
    }

    #[test]
    fn task_subtitles_name_their_steps() {
        let action = ProjectAction {
            id: "t".into(),
            title: "Checks".into(),
            kind: ActionKind::Task {
                steps: vec![
                    model::TaskStep {
                        title: "Format".into(),
                        command: "cargo fmt --check".into(),
                        cwd: String::new(),
                    },
                    model::TaskStep {
                        title: String::new(),
                        command: "cargo test".into(),
                        cwd: String::new(),
                    },
                ],
                continue_on_error: false,
            },
        };
        assert_eq!(subtitle_for(&action), "Format → cargo test");
    }

    #[test]
    fn opening_with_a_missing_app_says_which_action() {
        let error = open_with(
            "com.example.samlu-missing-app",
            "/Applications/Samlu Missing.app",
            "/tmp",
            "Open in Nowhere",
        )
        .unwrap_err();
        assert!(error.contains("“Open in Nowhere”"), "{error}");
    }

    #[test]
    fn folders_must_exist_inside_the_project() {
        let root = std::env::temp_dir().join(format!("samlu-folder-in-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src-tauri")).unwrap();
        let project = ProjectRef {
            name: "demo".into(),
            path: root.to_string_lossy().to_string(),
        };
        assert_eq!(folder_in(&project, "").unwrap(), root);
        assert_eq!(
            folder_in(&project, "src-tauri/").unwrap(),
            root.join("src-tauri")
        );
        assert!(folder_in(&project, "missing").is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}

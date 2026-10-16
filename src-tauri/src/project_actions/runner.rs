//! Tracked background tasks: steps run one after another in their own
//! process groups, with their output, status and URLs kept for the launcher.

use super::model::{ResolvedStep, TaskDetail, TaskSpec, TaskStatus, TaskSummary};
use super::process_group::{exit_code, ProcessGroup};
use super::shell::{command_for, ShellEnv};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Output kept in memory per task; the log file has all of it.
const MAX_LINES: usize = 2_000;
/// Tasks remembered, running or not. The oldest finished one goes first.
const MAX_RUNS: usize = 30;
const MAX_URLS: usize = 10;
/// Output reaches the pages in batches at most this often, so a chatty dev
/// server can't flood them with events.
const FLUSH_EVERY: Duration = Duration::from_millis(100);
/// How long a stopped task gets to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// After a step exits, how long its output may keep arriving from anything it
/// left running before the next step starts anyway.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

pub enum RunnerEvent {
    /// A task started, moved to another step, or found a URL.
    Changed(TaskSummary),
    /// New output lines, batched.
    Log { id: String, lines: Vec<String> },
    /// A URL appeared in a task's output for the first time.
    Url { task: TaskSummary, url: String },
    /// A task succeeded, failed or was stopped.
    Finished(TaskSummary),
}

/// Receives runner events. Called from runner threads, never while the
/// runner holds a lock, so it may call back into the runner.
pub type EventSink = Arc<dyn Fn(RunnerEvent) + Send + Sync>;
/// The environment each task starts from (the login environment).
pub type EnvSource = Arc<dyn Fn() -> ShellEnv + Send + Sync>;

pub struct TaskRunner {
    log_dir: PathBuf,
    env: EnvSource,
    sink: EventSink,
    runs: Mutex<Vec<Arc<Run>>>,
    counter: AtomicU64,
}

/// One task: what it runs and how far it has got.
struct Run {
    id: String,
    spec: TaskSpec,
    state: Mutex<RunState>,
    /// Signalled when the task finishes, for `stop` to wait on.
    finished: Condvar,
}

struct RunState {
    summary: TaskSummary,
    lines: VecDeque<String>,
    /// Lines not yet sent in a `Log` event.
    pending: Vec<String>,
    log: Option<File>,
    /// The process tree of the step running now.
    group: Option<ProcessGroup>,
    stop_requested: bool,
    /// Every step has ended; only the final report is left.
    steps_done: bool,
}

impl TaskRunner {
    /// `log_dir` holds one `<id>.log` per run.
    pub fn new(log_dir: PathBuf, env: EnvSource, sink: EventSink) -> Self {
        prune_logs(&log_dir);
        Self {
            log_dir,
            env,
            sink,
            runs: Mutex::new(Vec::new()),
            counter: AtomicU64::new(0),
        }
    }

    pub fn start(&self, spec: TaskSpec) -> Result<TaskSummary, String> {
        let Some(first) = spec.steps.first() else {
            return Err(format!("“{}” has no steps to run.", spec.title));
        };
        let id = format!(
            "t-{:x}-{:x}",
            now_millis(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let _ = std::fs::create_dir_all(&self.log_dir);
        let log_path = self.log_dir.join(format!("{id}.log"));
        let summary = TaskSummary {
            id: id.clone(),
            title: spec.title.clone(),
            project: spec.project.clone(),
            status: TaskStatus::Running,
            step: 0,
            step_count: spec.steps.len(),
            step_title: first.title.clone(),
            started_at: now_millis(),
            finished_at: None,
            exit_code: None,
            urls: Vec::new(),
            last_line: String::new(),
            log_path: log_path.to_string_lossy().to_string(),
        };
        let run = Arc::new(Run {
            id,
            spec,
            state: Mutex::new(RunState {
                summary: summary.clone(),
                lines: VecDeque::new(),
                pending: Vec::new(),
                log: File::create(&log_path).ok(),
                group: None,
                stop_requested: false,
                steps_done: false,
            }),
            finished: Condvar::new(),
        });
        self.remember(run.clone());
        (self.sink)(RunnerEvent::Changed(summary.clone()));
        let env = (self.env)();
        let sink = self.sink.clone();
        std::thread::spawn(move || execute(run, env, sink));
        Ok(summary)
    }

    /// Running tasks first, then the rest, newest first.
    pub fn list(&self) -> Vec<TaskSummary> {
        let runs = self.runs.lock().unwrap().clone();
        let mut summaries: Vec<TaskSummary> = runs.iter().map(|run| run.summary()).collect();
        summaries.sort_by(|left, right| {
            let running = |task: &TaskSummary| task.status == TaskStatus::Running;
            running(right)
                .cmp(&running(left))
                .then(right.started_at.cmp(&left.started_at))
                .then(right.id.cmp(&left.id))
        });
        summaries
    }

    /// The task with its last `max_lines` lines of output.
    pub fn detail(&self, id: &str, max_lines: usize) -> Option<TaskDetail> {
        let run = self.find(id)?;
        let state = run.state.lock().unwrap();
        let skip = state.lines.len().saturating_sub(max_lines);
        Some(TaskDetail {
            summary: state.summary.clone(),
            lines: state.lines.iter().skip(skip).cloned().collect(),
        })
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        let run = self
            .find(id)
            .ok_or_else(|| "That task is gone.".to_string())?;
        run.request_stop();
        run.settle(Instant::now() + STOP_GRACE);
        Ok(())
    }

    /// Starts the same spec again as a new task.
    pub fn rerun(&self, id: &str) -> Result<TaskSummary, String> {
        let run = self
            .find(id)
            .ok_or_else(|| "That task is gone.".to_string())?;
        self.start(run.spec.clone())
    }

    /// Forgets every task that is no longer running. Their logs stay.
    pub fn clear_finished(&self) {
        self.runs.lock().unwrap().retain(|run| run.is_running());
    }

    /// Stops every running task; called when Samlu quits. They all get the
    /// same grace period, not one each.
    pub fn stop_all(&self) {
        let running: Vec<Arc<Run>> = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|run| run.is_running())
            .cloned()
            .collect();
        for run in &running {
            run.request_stop();
        }
        let deadline = Instant::now() + STOP_GRACE;
        for run in &running {
            run.settle(deadline);
        }
    }

    fn find(&self, id: &str) -> Option<Arc<Run>> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .find(|run| run.id == id)
            .cloned()
    }

    fn remember(&self, run: Arc<Run>) {
        let mut runs = self.runs.lock().unwrap();
        runs.push(run);
        while runs.len() > MAX_RUNS {
            let Some(oldest) = runs.iter().position(|run| !run.is_running()) else {
                break;
            };
            let forgotten = runs.remove(oldest);
            let _ = std::fs::remove_file(&forgotten.summary().log_path);
        }
    }
}

impl Run {
    fn summary(&self) -> TaskSummary {
        self.state.lock().unwrap().summary.clone()
    }

    fn is_running(&self) -> bool {
        self.state.lock().unwrap().summary.status == TaskStatus::Running
    }

    /// A line of the task's own output: cleaned, kept, logged and searched
    /// for URLs.
    fn output(&self, sink: &EventSink, raw: &str) {
        let line = clean_line(raw);
        if line.trim().is_empty() {
            return;
        }
        let found: Vec<(TaskSummary, String)> = {
            let mut state = self.state.lock().unwrap();
            state.summary.last_line = line.clone();
            let mut found = Vec::new();
            for url in find_urls(&line) {
                if state.summary.urls.len() < MAX_URLS && !state.summary.urls.contains(&url) {
                    state.summary.urls.push(url.clone());
                    found.push(url);
                }
            }
            state.keep(line);
            found
                .into_iter()
                .map(|url| (state.summary.clone(), url))
                .collect()
        };
        for (task, url) in found {
            sink(RunnerEvent::Url { task, url });
        }
    }

    /// A line Samlu adds, such as a step's heading.
    fn note(&self, line: String) {
        self.state.lock().unwrap().keep(line);
    }

    /// Sends whatever output has not been sent yet.
    fn flush(&self, sink: &EventSink) {
        let lines = std::mem::take(&mut self.state.lock().unwrap().pending);
        if !lines.is_empty() {
            sink(RunnerEvent::Log {
                id: self.id.clone(),
                lines,
            });
        }
    }

    /// Marks the task as stopping and asks its current step to end.
    fn request_stop(&self) {
        let mut state = self.state.lock().unwrap();
        if state.summary.status != TaskStatus::Running || state.steps_done {
            return;
        }
        state.stop_requested = true;
        if let Some(group) = &state.group {
            group.terminate();
        }
    }

    /// Waits for the task to finish until `deadline`, then kills what is
    /// left of the current step.
    fn settle(&self, deadline: Instant) {
        let state = self.state.lock().unwrap();
        let wait = deadline.saturating_duration_since(Instant::now());
        let (state, timeout) = self
            .finished
            .wait_timeout_while(state, wait, |state| {
                state.summary.status == TaskStatus::Running
            })
            .unwrap();
        if timeout.timed_out() {
            if let Some(group) = &state.group {
                group.kill();
            }
        }
    }
}

impl RunState {
    fn keep(&mut self, line: String) {
        if let Some(log) = self.log.as_mut() {
            let _ = writeln!(log, "{line}");
        }
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line.clone());
        self.pending.push(line);
    }
}

/// Runs the task's steps in order on its own thread, then reports how it
/// ended.
fn execute(run: Arc<Run>, env: ShellEnv, sink: EventSink) {
    let flusher = {
        let (run, sink) = (run.clone(), sink.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(FLUSH_EVERY);
            run.flush(&sink);
            if run.state.lock().unwrap().steps_done {
                break;
            }
        })
    };

    let mut failure: Option<(usize, i32)> = None;
    let mut last_code = None;
    for (index, step) in run.spec.steps.iter().enumerate() {
        let moved_on = {
            let mut state = run.state.lock().unwrap();
            if state.stop_requested {
                break;
            }
            state.summary.step = index;
            state.summary.step_title = step.title.clone();
            (index > 0).then(|| state.summary.clone())
        };
        if let Some(summary) = moved_on {
            sink(RunnerEvent::Changed(summary));
        }
        run.note(format!("▸ {}", step.title));
        let code = run_step(&run, &env, step, &sink);
        last_code = Some(code);
        if code != 0 {
            failure = Some((index, code));
            if !run.spec.continue_on_error {
                break;
            }
        }
    }

    // A stop that arrives once the steps are over has nothing left to stop.
    let stopped = {
        let mut state = run.state.lock().unwrap();
        state.steps_done = true;
        state.stop_requested
    };
    let _ = flusher.join();
    let summary = {
        let mut state = run.state.lock().unwrap();
        state.summary.status = if stopped {
            TaskStatus::Stopped
        } else if failure.is_some() {
            TaskStatus::Failed
        } else {
            TaskStatus::Succeeded
        };
        state.summary.exit_code = failure.map(|(_, code)| code).or(last_code);
        if let (TaskStatus::Failed, Some((index, _))) = (state.summary.status, failure) {
            state.summary.step = index;
            state.summary.step_title = run.spec.steps[index].title.clone();
        }
        state.summary.finished_at = Some(now_millis());
        state.group = None;
        state.summary.clone()
    };
    run.flush(&sink);
    run.finished.notify_all();
    sink(RunnerEvent::Finished(summary));
}

/// Runs one step in its own process group and returns its exit code, or 128
/// plus the signal that ended it.
fn run_step(run: &Arc<Run>, env: &ShellEnv, step: &ResolvedStep, sink: &EventSink) -> i32 {
    let mut command = command_for(env, &step.command);
    command
        .env_clear()
        .envs(env.vars.iter().map(|(name, value)| (name, value)))
        .envs(run.spec.env.iter().map(|(name, value)| (name, value)))
        .current_dir(&step.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ProcessGroup::prepare(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            run.note(format!("Samlu couldn't start this step: {error}"));
            return 127;
        }
    };
    {
        let mut state = run.state.lock().unwrap();
        state.group = ProcessGroup::adopt(&child);
        // Stop arrived while the step was starting.
        if state.stop_requested {
            if let Some(group) = &state.group {
                group.terminate();
            }
        }
    }

    let streams: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let readers: Vec<_> = streams
        .into_iter()
        .map(|stream| {
            let (run, sink) = (run.clone(), sink.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut line = Vec::new();
                while matches!(reader.read_until(b'\n', &mut line), Ok(read) if read > 0) {
                    run.output(&sink, &String::from_utf8_lossy(&line));
                    line.clear();
                }
            })
        })
        .collect();

    let status = child.wait();
    // Output still in the pipes arrives before the next step starts, unless
    // something the step left running is holding them open.
    let drained = Instant::now() + DRAIN_GRACE;
    while readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < drained {
        std::thread::sleep(Duration::from_millis(10));
    }
    run.state.lock().unwrap().group = None;
    run.flush(sink);
    match status {
        Ok(status) => exit_code(status),
        Err(_) => 1,
    }
}

/// Whether a URL points at this Mac (localhost, 127.0.0.1, 0.0.0.0, [::1]).
pub fn is_local_url(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if authority.starts_with('[') {
        authority
            .split_inclusive(']')
            .next()
            .unwrap_or(authority)
            .to_string()
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    }
    .to_ascii_lowercase();
    matches!(
        host.as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]"
    ) || host.ends_with(".localhost")
}

/// Removes terminal escape sequences (colors, titles, cursor moves) and keeps
/// only what a progress line finally shows: the text after its last
/// carriage return.
fn clean_line(raw: &str) -> String {
    let raw = raw.trim_end_matches(['\n', '\r']);
    let shown = raw.rsplit('\r').next().unwrap_or(raw);
    let mut cleaned = String::with_capacity(shown.len());
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c != '\u{7}' {
                cleaned.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte between @ and ~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    cleaned
}

/// http(s) URLs in a line, without the punctuation that ends a sentence.
fn find_urls(line: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = line;
    while let Some(start) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        let candidate = &rest[start..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || "\"'<>)]`".contains(c))
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let scheme = if url.starts_with("https://") { 8 } else { 7 };
        if url.len() > scheme {
            urls.push(url.to_string());
        }
        rest = &candidate[end.max(1)..];
    }
    urls
}

/// Keeps the newest `MAX_RUNS` logs from earlier sessions.
fn prune_logs(log_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in logs.into_iter().skip(MAX_RUNS) {
        let _ = std::fs::remove_file(path);
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::model::{ProjectRef, ResolvedStep, TaskSpec, TaskStatus};
    use super::super::shell::ShellEnv;
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("samlu-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_env() -> EnvSource {
        Arc::new(|| ShellEnv {
            shell: if cfg!(windows) {
                std::env::var_os("ComSpec")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into())
            } else {
                "/bin/sh".into()
            },
            vars: std::env::vars().collect(),
        })
    }

    /// The same step for the shell each platform runs commands with.
    fn sh(unix: &'static str, windows: &'static str) -> &'static str {
        if cfg!(windows) {
            windows
        } else {
            unix
        }
    }

    fn runner(name: &str) -> (Arc<TaskRunner>, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let sink: EventSink = Arc::new(move |event| {
            let name = match event {
                RunnerEvent::Changed(_) => "changed".to_string(),
                RunnerEvent::Log { .. } => "log".to_string(),
                RunnerEvent::Url { url, .. } => format!("url {url}"),
                RunnerEvent::Finished(task) => format!("finished {:?}", task.status),
            };
            seen.lock().unwrap().push(name);
        });
        (
            Arc::new(TaskRunner::new(folder(name).join("logs"), test_env(), sink)),
            events,
        )
    }

    fn spec(name: &str, commands: &[&str], continue_on_error: bool) -> TaskSpec {
        let cwd = folder(&format!("{name}-cwd"));
        TaskSpec {
            title: name.into(),
            project: ProjectRef {
                name: "demo".into(),
                path: cwd.to_string_lossy().to_string(),
            },
            steps: commands
                .iter()
                .map(|command| ResolvedStep {
                    title: command.to_string(),
                    command: command.to_string(),
                    cwd: cwd.clone(),
                })
                .collect(),
            continue_on_error,
            env: vec![("SAMLU_PROJECT_NAME".into(), "demo".into())],
        }
    }

    /// Events arrive on runner threads a moment after the state changes.
    fn wait_for_event(events: &Mutex<Vec<String>>, wanted: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !events.lock().unwrap().iter().any(|event| event == wanted) {
            assert!(Instant::now() < deadline, "never saw {wanted}");
            std::thread::sleep(Duration::from_millis(10));
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
        let task = runner
            .start(spec(
                "basic",
                &[
                    sh(
                        "echo hello \"$SAMLU_PROJECT_NAME\"",
                        "echo hello %SAMLU_PROJECT_NAME%",
                    ),
                    sh("echo oops >&2; exit 3", "(echo oops)1>&2 & exit 3"),
                ],
                false,
            ))
            .unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        let detail = finish(&runner, &task.id);
        assert_eq!(detail.summary.status, TaskStatus::Failed);
        assert_eq!(detail.summary.exit_code, Some(3));
        assert_eq!(detail.summary.step, 1);
        assert!(detail.lines.contains(&"hello demo".to_string()));
        assert!(detail.lines.contains(&"oops".to_string()));
        let log = std::fs::read_to_string(&detail.summary.log_path).unwrap();
        assert!(log.contains("hello demo") && log.contains("oops"));
        wait_for_event(&events, "finished Failed");
        let events = events.lock().unwrap();
        assert_eq!(events.first().map(String::as_str), Some("changed"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("finished"))
                .count(),
            1
        );
        assert!(events.iter().any(|event| event == "log"));
    }

    #[test]
    fn a_failed_step_stops_the_task_unless_told_to_continue() {
        let (runner, _) = runner("continue");
        let stopped = finish(
            &runner,
            &runner
                .start(spec("halt", &[sh("false", "exit 1"), "echo after"], false))
                .unwrap()
                .id,
        );
        assert_eq!(stopped.summary.status, TaskStatus::Failed);
        assert!(!stopped.lines.contains(&"after".to_string()));
        let kept_going = finish(
            &runner,
            &runner
                .start(spec("go", &[sh("false", "exit 1"), "echo after"], true))
                .unwrap()
                .id,
        );
        assert_eq!(kept_going.summary.status, TaskStatus::Failed);
        assert_eq!(kept_going.summary.step, 0);
        assert!(kept_going.lines.contains(&"after".to_string()));
        let fine = finish(
            &runner,
            &runner
                .start(spec("fine", &[sh("true", "exit 0"), "echo done"], false))
                .unwrap()
                .id,
        );
        assert_eq!(fine.summary.status, TaskStatus::Succeeded);
        assert_eq!(fine.summary.exit_code, Some(0));
    }

    #[cfg(unix)]
    #[test]
    fn stop_kills_the_whole_process_group() {
        let (runner, events) = runner("stop");
        let spec = spec("stop", &["sleep 30 & echo $! > child.pid; wait"], false);
        let pid_file = spec.steps[0].cwd.join("child.pid");
        let task = runner.start(spec).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&pid_file)
            .map(|pid| pid.trim().is_empty())
            .unwrap_or(true)
        {
            assert!(Instant::now() < deadline, "child never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let child: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        runner.stop(&task.id).unwrap();
        assert_eq!(
            finish(&runner, &task.id).summary.status,
            TaskStatus::Stopped
        );
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(
            unsafe { libc::kill(child, 0) },
            0,
            "grandchild {child} survived"
        );
        wait_for_event(&events, "finished Stopped");
        assert!(runner.stop(&task.id).is_ok());
        assert_eq!(runner.stop("nope").unwrap_err(), "That task is gone.");
    }

    #[cfg(windows)]
    #[test]
    fn stop_kills_the_whole_process_tree() {
        let (runner, events) = runner("stop");
        // A grandchild in the background writes a heartbeat every second.
        let spec = spec(
            "stop",
            &[
                r#"start /b cmd /d /c "for /l %i in (1,1,600) do ((echo %i)>beat.txt & ping -n 2 127.0.0.1 >nul)" & ping -n 60 127.0.0.1 >nul"#,
            ],
            false,
        );
        let beat = spec.steps[0].cwd.join("beat.txt");
        let read = || std::fs::read_to_string(&beat).unwrap_or_default();
        let task = runner.start(spec).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while read().trim().parse::<u32>().unwrap_or(0) < 2 {
            assert!(Instant::now() < deadline, "grandchild never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        runner.stop(&task.id).unwrap();
        assert_eq!(
            finish(&runner, &task.id).summary.status,
            TaskStatus::Stopped
        );
        std::thread::sleep(Duration::from_millis(300));
        let after_stop = read();
        std::thread::sleep(Duration::from_millis(2_500));
        assert_eq!(read(), after_stop, "the grandchild kept running");
        wait_for_event(&events, "finished Stopped");
        assert_eq!(runner.stop("nope").unwrap_err(), "That task is gone.");
    }

    #[test]
    fn urls_are_collected_once_each() {
        let (runner, events) = runner("urls");
        let task = runner
            .start(spec(
                "urls",
                &[sh(
                    "echo 'Your quick Tunnel: https://abc-def.trycloudflare.com.'; echo again https://abc-def.trycloudflare.com; echo '  Local: http://localhost:3000/'",
                    "(echo Your quick Tunnel: https://abc-def.trycloudflare.com.)&(echo again https://abc-def.trycloudflare.com)&(echo   Local: http://localhost:3000/)",
                )],
                false,
            ))
            .unwrap();
        let detail = finish(&runner, &task.id);
        wait_for_event(&events, "finished Succeeded");
        assert_eq!(
            detail.summary.urls,
            vec![
                "https://abc-def.trycloudflare.com".to_string(),
                "http://localhost:3000/".to_string()
            ]
        );
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.starts_with("url "))
                .count(),
            2
        );
    }

    #[test]
    fn lines_are_cleaned_of_escapes_and_progress() {
        assert_eq!(clean_line("\u{1b}[32m✓ ready\u{1b}[0m"), "✓ ready");
        assert_eq!(clean_line("10%\r50%\r100%"), "100%");
        assert_eq!(clean_line("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(clean_line("crlf line\r\n"), "crlf line");
        assert_eq!(
            find_urls("see (https://x.dev/a?b=1), then http://localhost:5173."),
            vec![
                "https://x.dev/a?b=1".to_string(),
                "http://localhost:5173".to_string()
            ]
        );
        assert!(find_urls("https:// and http://").is_empty());
    }

    #[test]
    fn local_urls_are_recognised() {
        for url in [
            "http://localhost:3000",
            "http://127.0.0.1:8080/x",
            "http://0.0.0.0:5173",
            "http://[::1]:3000",
            "http://app.localhost:1355",
        ] {
            assert!(is_local_url(url), "{url}");
        }
        assert!(!is_local_url("https://abc.trycloudflare.com"));
        assert!(!is_local_url("https://localhost.example.com"));
    }

    #[test]
    fn output_is_capped_and_keeps_the_end() {
        let (runner, _) = runner("cap");
        let detail = finish(
            &runner,
            &runner
                .start(spec(
                    "cap",
                    &[sh(
                        "i=0; while [ $i -lt 5000 ]; do echo line $i; i=$((i+1)); done",
                        "for /l %i in (0,1,4999) do @echo line %i",
                    )],
                    false,
                ))
                .unwrap()
                .id,
        );
        assert!(detail.lines.len() <= MAX_LINES);
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
        finish(&runner, &second.id);
        let slow = runner
            .start(spec(
                "slow",
                &[sh("sleep 5", "ping -n 6 127.0.0.1 >nul")],
                false,
            ))
            .unwrap();
        assert_eq!(runner.list()[0].id, slow.id);
        runner.clear_finished();
        assert_eq!(
            runner
                .list()
                .iter()
                .map(|task| task.id.clone())
                .collect::<Vec<_>>(),
            vec![slow.id.clone()]
        );
        runner.stop_all();
        assert_eq!(
            finish(&runner, &slow.id).summary.status,
            TaskStatus::Stopped
        );
    }

    #[test]
    fn a_step_that_cannot_start_fails_the_task() {
        let (runner, _) = runner("missing-dir");
        let mut spec = spec("missing", &["echo never"], false);
        spec.steps[0].cwd = PathBuf::from("/nonexistent/samlu/folder");
        let detail = finish(&runner, &runner.start(spec).unwrap().id);
        assert_eq!(detail.summary.status, TaskStatus::Failed);
        assert_eq!(detail.summary.exit_code, Some(127));
        assert!(detail
            .lines
            .iter()
            .any(|line| line.starts_with("Samlu couldn't start this step")));
    }

    #[test]
    fn the_sink_may_call_back_into_the_runner() {
        let slot: Arc<Mutex<Option<Arc<TaskRunner>>>> = Arc::new(Mutex::new(None));
        let inner = slot.clone();
        let sink: EventSink = Arc::new(move |_event| {
            if let Some(runner) = inner.lock().unwrap().clone() {
                let _ = runner.list();
            }
        });
        let runner = Arc::new(TaskRunner::new(
            folder("reentrant").join("logs"),
            test_env(),
            sink,
        ));
        *slot.lock().unwrap() = Some(runner.clone());
        let task = runner
            .start(spec("reentrant", &["echo hi"], false))
            .unwrap();
        assert_eq!(
            finish(&runner, &task.id).summary.status,
            TaskStatus::Succeeded
        );
        *slot.lock().unwrap() = None;
    }
}

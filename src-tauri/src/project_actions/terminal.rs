//! Running a command in a terminal window: Terminal, iTerm and Warp open a
//! self-deleting `.command` script; Ghostty runs it from its command line.
//! On Windows every terminal runs a self-deleting `.cmd` script instead.

use super::model::TerminalApp;
#[cfg(not(windows))]
use super::shell::{login_env, quote};
#[cfg(not(windows))]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Generated files that were never picked up are cleared after this long.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Opens a new window or tab of `terminal` in `cwd`, runs `command` through
/// the user's interactive shell, and leaves that shell open afterwards.
/// `work_dir` is Samlu's folder for generated scripts and configurations.
#[cfg(not(windows))]
pub fn run_in_terminal(
    terminal: &TerminalApp,
    command: &str,
    cwd: &Path,
    title: &str,
    work_dir: &Path,
) -> Result<(), String> {
    let shell = &login_env().shell;
    match terminal.id.as_str() {
        // Each opens a `.command` file it is handed in a new window or tab
        // and runs it. Warp's launch links only find configurations Warp has
        // already loaded, so a fresh one is often missed; a script never is.
        "terminal" | "iterm" | "warp" => {
            let script = write_script(work_dir, &command_script(shell, command, cwd, title))?;
            open(
                &terminal.name,
                &["-b", &terminal.bundle_id, &script.to_string_lossy()],
            )
        }
        "ghostty" => {
            let script = write_script(work_dir, &command_script(shell, command, cwd, title))?;
            open(
                &terminal.name,
                &[
                    "-na",
                    &terminal.app_path,
                    "--args",
                    &format!("--working-directory={}", cwd.display()),
                    "-e",
                    &script.to_string_lossy(),
                ],
            )
        }
        _ => Err(format!(
            "Samlu can't run commands in {} yet.",
            terminal.name
        )),
    }
}

/// The script every terminal runs. It is plain `sh` whatever the
/// user's shell is, deletes itself, moves to the folder, names the window,
/// runs the command through the user's interactive login shell (so aliases
/// and `.zshrc` apply), and then stays at a prompt in that folder.
#[cfg(not(windows))]
fn command_script(shell: &Path, command: &str, cwd: &Path, title: &str) -> String {
    let shell = quote(&shell.to_string_lossy());
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- {cwd} || exit 1\nprintf '\\033]0;%s\\007' {title}\n{shell} -ilc {command}\nexec {shell} -il\n",
        cwd = quote(&cwd.to_string_lossy()),
        title = quote(title),
        command = quote(command),
    )
}

#[cfg(not(windows))]
fn write_script(work_dir: &Path, contents: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(work_dir)
        .map_err(|error| format!("Couldn't prepare the terminal: {error}"))?;
    remove_stale(work_dir, "samlu-", ".command");
    let path = work_dir.join(format!("samlu-{}.command", unique()));
    std::fs::write(&path, contents)
        .and_then(|_| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)))
        .map_err(|error| format!("Couldn't prepare the terminal: {error}"))?;
    Ok(path)
}

/// Generated files a terminal never ran (it was closed, or refused them).
fn remove_stale(folder: &Path, prefix: &str, suffix: &str) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if name.starts_with(prefix) && name.ends_with(suffix) && stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn unique() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// `open` reports a terminal it can't reach on stderr; that becomes the
/// message the launcher shows.
#[cfg(not(windows))]
fn open(terminal: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new("/usr/bin/open")
        .args(args)
        .output()
        .map_err(|error| format!("Couldn't open {terminal}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            format!("Couldn't open {terminal}.")
        } else {
            format!("Couldn't open {terminal}: {detail}")
        })
    }
}

/* ------------------------------------------------------------- Windows */

/// `CREATE_NO_WINDOW`, for the `cmd.exe` that only starts a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Opens a new window or tab of `terminal` in `cwd`, runs `command` through
/// `cmd.exe`, the shell Samlu runs commands with on Windows, and then leaves
/// an interactive shell open there: PowerShell, or `cmd.exe` in Command
/// Prompt. `work_dir` is Samlu's folder for generated scripts.
#[cfg(windows)]
pub fn run_in_terminal(
    terminal: &TerminalApp,
    command: &str,
    cwd: &Path,
    title: &str,
    work_dir: &Path,
) -> Result<(), String> {
    if !super::model::COMMAND_TERMINALS.contains(&terminal.id.as_str()) {
        return Err(format!(
            "Samlu can't run commands in {} yet.",
            terminal.name
        ));
    }
    let in_cmd = terminal.id == "cmd";
    let stay = if in_cmd {
        "echo on".to_string()
    } else {
        format!("\"{}\" -NoLogo", interactive_shell().display())
    };
    let script = write_batch(work_dir, &batch_script(command, cwd, title, &stay))?;
    let program = format!(
        "\"{}\" /d {} \"{}\"",
        cmd_exe().display(),
        if in_cmd { "/k" } else { "/c" },
        script.display()
    );
    launch(
        &terminal.id,
        &terminal.app_path,
        &terminal.name,
        cwd,
        title,
        Some(&program),
    )
}

/// Opens `terminal` at `cwd` with its usual shell and nothing run.
#[cfg(windows)]
pub fn open_folder(id: &str, app_path: &str, cwd: &Path) -> Result<(), String> {
    let name = Path::new(app_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| id.to_string());
    let title = cwd
        .file_name()
        .map(|folder| folder.to_string_lossy().to_string())
        .unwrap_or_default();
    launch(id, app_path, &name, cwd, &title, None)
}

/// Starts `program` (a command line) in a new window or tab of the terminal,
/// or the terminal's own shell when there is none.
#[cfg(windows)]
fn launch(
    id: &str,
    app_path: &str,
    name: &str,
    cwd: &Path,
    title: &str,
    program: Option<&str>,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let folder = cwd.to_string_lossy().to_string();
    let mut command = match id {
        // Windows Terminal reads `;` as the start of another tab.
        "windows-terminal" => {
            let mut command = Command::new(app_path);
            command
                .args(["-w", "new", "new-tab", "--title"])
                .arg(window_title(title).replace(';', ","))
                .arg("-d")
                .arg(folder.replace(';', r"\;"));
            if let Some(program) = program {
                command.raw_arg(program.replace(';', r"\;"));
            }
            command
        }
        // Console programs get their window from `start`, which gives them
        // a console of their own.
        "powershell" | "cmd" => {
            let shell = match (program, id) {
                (Some(program), _) => program.to_string(),
                (None, "powershell") => format!("\"{app_path}\" -NoLogo"),
                (None, _) => format!("\"{app_path}\""),
            };
            let mut command = Command::new(cmd_exe());
            command
                .raw_arg(format!("/d /c start \"\" /d \"{folder}\" {shell}"))
                .creation_flags(CREATE_NO_WINDOW);
            command
        }
        "wezterm" => {
            let mut command = Command::new(app_path);
            command.args(["start", "--cwd", &folder]);
            if let Some(program) = program {
                command.arg("--").raw_arg(program);
            }
            command
        }
        "alacritty" => {
            let mut command = Command::new(app_path);
            command
                .args(["--working-directory", &folder, "--title"])
                .arg(window_title(title));
            if let Some(program) = program {
                command.arg("-e").raw_arg(program);
            }
            command
        }
        _ if program.is_none() => {
            let mut command = Command::new(app_path);
            command.current_dir(cwd);
            command
        }
        _ => return Err(format!("Samlu can't run commands in {name} yet.")),
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open {name}: {error}"))
}

/// The script every Windows terminal runs. It switches the console to UTF-8
/// so folder names and titles survive, moves to the folder, names the
/// window, runs the command, then deletes itself and hands the console to
/// `stay`, the interactive shell. `(goto)` ends the script before it is
/// deleted, so `cmd.exe` never looks for the rest of a missing file.
#[cfg(windows)]
fn batch_script(command: &str, cwd: &Path, title: &str, stay: &str) -> String {
    format!(
        "@echo off\r\nchcp 65001 >nul\r\ntitle {title}\r\ncd /d \"{cwd}\"\r\n{command}\r\n(goto) 2>nul & del \"%~f0\" & {stay}\r\n",
        title = window_title(title),
        cwd = cwd.display(),
    )
}

/// A window title `cmd.exe` prints as written: nothing it would read as an
/// operator or a variable.
#[cfg(windows)]
fn window_title(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if "&|<>^%\"".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(windows)]
fn write_batch(work_dir: &Path, contents: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(work_dir)
        .map_err(|error| format!("Couldn't prepare the terminal: {error}"))?;
    remove_stale(work_dir, "samlu-", ".cmd");
    let path = work_dir.join(format!("samlu-{}.cmd", unique()));
    std::fs::write(&path, contents)
        .map_err(|error| format!("Couldn't prepare the terminal: {error}"))?;
    Ok(path)
}

#[cfg(windows)]
fn cmd_exe() -> PathBuf {
    std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| {
            PathBuf::from(crate::platform::expand_variables(
                r"%SystemRoot%\System32\cmd.exe",
            ))
        })
}

/// PowerShell 7 when it is installed, Windows PowerShell otherwise.
#[cfg(windows)]
fn interactive_shell() -> PathBuf {
    let modern = PathBuf::from(crate::platform::expand_variables(
        r"%ProgramFiles%\PowerShell\7\pwsh.exe",
    ));
    if modern.is_file() {
        modern
    } else {
        PathBuf::from(crate::platform::expand_variables(
            r"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn command_scripts_run_in_the_folder_and_keep_the_shell() {
        let script = command_script(
            Path::new("/bin/zsh"),
            "claude --resume",
            Path::new("/Users/me/My App"),
            "Claude Code · My App",
        );
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[0], "#!/bin/sh");
        assert_eq!(lines[1], "rm -f -- \"$0\"");
        assert_eq!(lines[2], "cd -- '/Users/me/My App' || exit 1");
        assert_eq!(lines[3], r"printf '\033]0;%s\007' 'Claude Code · My App'");
        assert_eq!(lines[4], "/bin/zsh -ilc 'claude --resume'");
        assert_eq!(lines[5], "exec /bin/zsh -il");
    }

    #[cfg(not(windows))]
    #[test]
    fn command_scripts_survive_quotes_in_the_command() {
        let script = command_script(
            Path::new("/bin/zsh"),
            "echo 'it works' && pwd",
            Path::new("/tmp"),
            "t",
        );
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(
                script
                    .lines()
                    .nth(4)
                    .unwrap()
                    .replace("/bin/zsh -ilc", "/bin/sh -c"),
            )
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).lines().next(),
            Some("it works")
        );
    }

    #[test]
    fn unknown_terminals_are_refused() {
        let kitty = TerminalApp {
            id: "kitty".into(),
            name: "kitty".into(),
            bundle_id: "net.kovidgoyal.kitty".into(),
            app_path: "/Applications/kitty.app".into(),
            runs_commands: false,
        };
        assert_eq!(
            run_in_terminal(&kitty, "true", Path::new("/tmp"), "t", Path::new("/tmp")).unwrap_err(),
            "Samlu can't run commands in kitty yet."
        );
    }

    #[cfg(windows)]
    #[test]
    fn batch_scripts_run_in_the_folder_and_hand_over_the_shell() {
        let script = batch_script(
            "claude --resume",
            Path::new(r"C:\Users\me\My App"),
            "Claude Code · My App & co",
            r#""C:\pwsh.exe" -NoLogo"#,
        );
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[0], "@echo off");
        assert_eq!(lines[1], "chcp 65001 >nul");
        assert_eq!(lines[2], "title Claude Code · My App   co");
        assert_eq!(lines[3], r#"cd /d "C:\Users\me\My App""#);
        assert_eq!(lines[4], "claude --resume");
        assert_eq!(
            lines[5],
            r#"(goto) 2>nul & del "%~f0" & "C:\pwsh.exe" -NoLogo"#
        );
        assert!(script.ends_with("\r\n"));
    }

    #[cfg(windows)]
    #[test]
    fn batch_scripts_run_the_command_and_delete_themselves() {
        let root =
            std::env::temp_dir().join(format!("samlu terminal batch {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = write_batch(
            &root.join("work"),
            &batch_script("echo it works && cd", &root, "t", "echo handed over"),
        )
        .unwrap();
        let output = Command::new(cmd_exe())
            .args(["/d", "/c"])
            .arg(&script)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
        assert_eq!(lines.first(), Some(&"it works"), "{stdout}");
        assert_eq!(
            std::fs::canonicalize(lines[1]).unwrap(),
            std::fs::canonicalize(&root).unwrap()
        );
        assert_eq!(lines.last(), Some(&"handed over"), "{stdout}");
        assert!(!script.exists(), "the script should delete itself");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn titles_lose_characters_cmd_would_run() {
        assert_eq!(
            window_title(r#"a & b | c > %PATH% "q""#),
            "a   b   c    PATH   q"
        );
    }

    /// Opens one real window; run with `cargo test -- --ignored`.
    #[cfg(target_os = "macos")]
    fn live_check(id: &str, name: &str, bundle_id: &str, app_path: &str) {
        let root =
            std::env::temp_dir().join(format!("samlu terminal check {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let marker = PathBuf::from(format!("/tmp/samlu-terminal-check-{id}.txt"));
        let _ = std::fs::remove_file(&marker);
        let terminal = TerminalApp {
            id: id.into(),
            name: name.into(),
            bundle_id: bundle_id.into(),
            app_path: app_path.into(),
            runs_commands: true,
        };
        run_in_terminal(
            &terminal,
            &format!(
                "pwd > {} && echo 'Samlu terminal test — you can close this window'",
                marker.display()
            ),
            &root,
            "Samlu terminal test",
            &root.join("work"),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "{name} never ran the command"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_millis(200));
        let ran_in = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            std::fs::canonicalize(ran_in.trim()).unwrap(),
            std::fs::canonicalize(&root).unwrap()
        );
        assert!(!std::fs::read_dir(root.join("work"))
            .map(|mut entries| entries.any(|entry| entry
                .map(|entry| entry.file_name().to_string_lossy().ends_with(".command"))
                .unwrap_or(false)))
            .unwrap_or(false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_terminal_app() {
        live_check(
            "terminal",
            "Terminal",
            "com.apple.Terminal",
            "/System/Applications/Utilities/Terminal.app",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_warp() {
        live_check(
            "warp",
            "Warp",
            "dev.warp.Warp-Stable",
            "/Applications/Warp.app",
        );
    }
}

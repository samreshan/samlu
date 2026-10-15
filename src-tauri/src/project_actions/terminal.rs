//! Running a command in a terminal window: Terminal, iTerm and Warp open a
//! self-deleting `.command` script; Ghostty runs it from its command line.

use super::model::TerminalApp;
use super::shell::{login_env, quote};
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
fn command_script(shell: &Path, command: &str, cwd: &Path, title: &str) -> String {
    let shell = quote(&shell.to_string_lossy());
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- {cwd} || exit 1\nprintf '\\033]0;%s\\007' {title}\n{shell} -ilc {command}\nexec {shell} -il\n",
        cwd = quote(&cwd.to_string_lossy()),
        title = quote(title),
        command = quote(command),
    )
}

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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Opens one real window; run with `cargo test -- --ignored`.
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

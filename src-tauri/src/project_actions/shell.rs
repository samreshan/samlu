//! The user's shell environment, and turning command and URL templates into
//! something safe to run.
//!
//! Commands run through the login shell on macOS and through `cmd.exe` on
//! Windows, which is also what npm scripts use there.

use super::model::ProjectRef;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::Duration;

/// Printed just before the environment, so whatever `.zshrc` prints first can
/// be told apart from it.
#[cfg(unix)]
const MARKER: &str = "__SAMLU_ENV__";
/// A `.zshrc` waiting on something must not hold up the first action.
#[cfg(unix)]
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Variables that describe the shell that printed them, not the user's setup.
#[cfg(unix)]
const SHELL_LOCAL: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_"];

/// The environment commands run with: the user's login shell and the
/// variables an interactive login shell ends up with.
#[derive(Clone, Debug)]
pub struct ShellEnv {
    pub shell: PathBuf,
    pub vars: Vec<(String, String)>,
}

impl ShellEnv {
    /// The folders on `PATH`, in order.
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        self.vars
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| std::env::split_paths(value).collect())
            .unwrap_or_default()
    }
}

/// Resolved once from `$SHELL -ilc` and cached. Waits at most about five
/// seconds, then falls back to Samlu's own environment plus the usual tool
/// folders.
pub fn login_env() -> &'static ShellEnv {
    static ENV: OnceLock<ShellEnv> = OnceLock::new();
    ENV.get_or_init(|| {
        let shell = user_shell();
        let vars = resolve(&shell).unwrap_or_else(fallback_vars);
        ShellEnv { shell, vars }
    })
}

/// A process that runs `command` through the user's shell: `$SHELL -c` on
/// Unix, `cmd.exe /d /s /c` on Windows, where `/s` hands over everything
/// between the outer quotes untouched.
pub fn command_for(env: &ShellEnv, command: &str) -> Command {
    let mut process = Command::new(&env.shell);
    #[cfg(unix)]
    process.arg("-c").arg(command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.raw_arg(format!("/d /s /c \"{command}\""));
    }
    process
}

/// POSIX single-quoting. Plain words are returned unchanged.
#[cfg(unix)]
pub fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// `cmd.exe` double-quoting. Plain words are returned unchanged. Windows file
/// names can't contain `"`; a stray one is doubled, which `cmd.exe` and
/// programs read back as one quote inside the word.
#[cfg(windows)]
pub fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,\\".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("\"{}\"", word.replace('"', "\"\""))
    }
}

/// `{path}` and `{name}` become quoted shell words. Quotes a template already
/// put around a placeholder are dropped first, since the value brings its
/// own.
pub fn expand_command(template: &str, project: &ProjectRef) -> String {
    let mut template = template.to_string();
    for placeholder in ["{path}", "{name}"] {
        for mark in ['"', '\''] {
            template = template.replace(&format!("{mark}{placeholder}{mark}"), placeholder);
        }
    }
    substitute(&template, |name| match name {
        "path" => Some(quote(&project.path)),
        "name" => Some(quote(&project.name)),
        _ => None,
    })
}

/// `{path}` and `{name}` become percent-encoded (the path keeps its
/// slashes).
pub fn expand_url(template: &str, project: &ProjectRef) -> String {
    substitute(template, |name| match name {
        "path" => Some(percent_encode(&project.path, b"/")),
        "name" => Some(percent_encode(&project.name, b"")),
        _ => None,
    })
}

/// Replaces each `{placeholder}` `value_for` knows in one pass, so a value
/// that happens to contain `{name}` is never expanded a second time. Braces
/// it doesn't know stay as written.
fn substitute(template: &str, value_for: impl Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        expanded.push_str(&rest[..start]);
        let candidate = &rest[start..];
        let replacement = candidate
            .find('}')
            .and_then(|end| value_for(&candidate[1..end]).map(|value| (end, value)));
        match replacement {
            Some((end, value)) => {
                expanded.push_str(&value);
                rest = &candidate[end + 1..];
            }
            None => {
                expanded.push('{');
                rest = &candidate[1..];
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// RFC 3986: unreserved characters, and those in `keep`, stay as they are.
fn percent_encode(value: &str, keep: &[u8]) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || keep.contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(unix)]
fn user_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute() && is_executable(shell))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Runs the user's shell the way a terminal would, interactive and login,
/// and reads the environment it ends up with. On this kind of setup `.zshrc`
/// is where Homebrew and `~/.local/bin` join PATH, and a plain login shell
/// never reads it.
#[cfg(unix)]
fn resolve(shell: &Path) -> Option<Vec<(String, String)>> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(shell)
        .arg("-ilc")
        .arg(format!("printf '\\n{MARKER}\\n'; /usr/bin/env -0"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(RESOLVE_TIMEOUT);
    if output.is_err() {
        // Stops the shell and anything it started that still holds the pipe.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.wait();
    parse_env_output(&output.ok()?, MARKER)
}

/// The `NAME=value` entries `env -0` printed after `marker`. Values may hold
/// newlines; only NUL separates entries.
#[cfg(unix)]
fn parse_env_output(output: &[u8], marker: &str) -> Option<Vec<(String, String)>> {
    let needle = format!("\n{marker}\n");
    let start = output
        .windows(needle.len())
        .position(|window| window == needle.as_bytes())?
        + needle.len();
    Some(
        output[start..]
            .split(|byte| *byte == 0)
            .filter_map(|entry| {
                let entry = String::from_utf8_lossy(entry);
                let (name, value) = entry.split_once('=')?;
                (!name.is_empty() && !SHELL_LOCAL.contains(&name))
                    .then(|| (name.to_string(), value.to_string()))
            })
            .collect(),
    )
}

/// `cmd.exe`, as the system names it.
#[cfg(windows)]
fn user_shell() -> PathBuf {
    std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute() && shell.is_file())
        .unwrap_or_else(|| {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            Path::new(&root).join(r"System32\cmd.exe")
        })
}

/// Windows has no login shell to ask. Installers put new tool folders on PATH
/// in the registry, so PATH is read from there, keeping any folders Samlu's
/// own environment adds, and the rest is Samlu's environment.
#[cfg(windows)]
fn resolve(_shell: &Path) -> Option<Vec<(String, String)>> {
    let registry = crate::platform::registry_path()?;
    let mut folders: Vec<PathBuf> = std::env::split_paths(&registry).collect();
    if let Some(own) = std::env::var_os("PATH") {
        for folder in std::env::split_paths(&own) {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
    }
    Some(with_path(folders))
}

/// Samlu's environment with PATH set to `folders`. On Windows the variable
/// is spelled `Path`, and either spelling is replaced.
fn with_path(folders: Vec<PathBuf>) -> Vec<(String, String)> {
    let path = std::env::join_paths(&folders)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    std::env::vars()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("PATH"))
        .chain([("PATH".to_string(), path)])
        .collect()
}

/// Folders tools usually install into, for when PATH can't be read.
#[cfg(unix)]
fn usual_tool_folders() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join(".bun/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ]
}

#[cfg(windows)]
fn usual_tool_folders() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut folders = vec![
        home.join(".local").join("bin"),
        home.join(".cargo").join("bin"),
        home.join(".bun").join("bin"),
    ];
    if let Some(roaming) = dirs::data_dir() {
        folders.push(roaming.join("npm"));
    }
    if let Some(local) = dirs::data_local_dir() {
        folders.push(local.join("Microsoft").join("WindowsApps"));
    }
    folders
}

/// Samlu's own environment with the usual tool folders added to PATH, for
/// when the login shell can't be asked.
fn fallback_vars() -> Vec<(String, String)> {
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for folder in usual_tool_folders() {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    with_path(folders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn project() -> ProjectRef {
        ProjectRef {
            name: "My \"App\"".into(),
            path: "/Users/me/My \"App\" $dev".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn quoting_leaves_plain_words_alone() {
        assert_eq!(quote("/Users/me/code/app-1.0"), "/Users/me/code/app-1.0");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote(""), "''");
    }

    #[cfg(unix)]
    #[test]
    fn placeholders_become_exactly_one_shell_word() {
        let project = project();
        assert_eq!(
            expand_command("ctun -l {path} start", &project),
            r#"ctun -l '/Users/me/My "App" $dev' start"#
        );
        assert_eq!(
            expand_command("code \"{path}\"", &project),
            r#"code '/Users/me/My "App" $dev'"#
        );
        assert_eq!(
            expand_command("echo '{name}' {other}", &project),
            r#"echo 'My "App"' {other}"#
        );
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(expand_command("printf '%s|' {path} {name}", &project))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "/Users/me/My \"App\" $dev|My \"App\"|"
        );
    }

    #[test]
    fn a_value_is_never_expanded_twice() {
        let project = ProjectRef {
            name: "x".into(),
            path: "/tmp/{name}".into(),
        };
        assert_eq!(
            expand_command("ls {path}", &project),
            format!("ls {}", quote("/tmp/{name}"))
        );
        assert_eq!(expand_url("x://{path}", &project), "x:///tmp/%7Bname%7D");
    }

    #[test]
    fn urls_get_percent_encoded_placeholders() {
        let project = ProjectRef {
            name: "My App".into(),
            path: "/Users/me/My App".into(),
        };
        assert_eq!(
            expand_url("https://x.dev/open?dir={path}&n={name}", &project),
            "https://x.dev/open?dir=/Users/me/My%20App&n=My%20App"
        );
    }

    #[cfg(unix)]
    #[test]
    fn env_output_is_read_after_the_marker() {
        let raw = b"The operation couldn't be completed. Unable to locate a Java Runtime.\n__SAMLU_ENV__\nPATH=/a:/b\0HOME=/h\0MULTI=x\ny\0PWD=/tmp\0";
        let vars = parse_env_output(raw, "__SAMLU_ENV__").unwrap();
        assert_eq!(
            vars,
            vec![
                ("PATH".to_string(), "/a:/b".to_string()),
                ("HOME".to_string(), "/h".to_string()),
                ("MULTI".to_string(), "x\ny".to_string()),
            ]
        );
        assert!(parse_env_output(b"no marker here", "__SAMLU_ENV__").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_adds_the_usual_tool_folders() {
        let env = ShellEnv {
            shell: "/bin/zsh".into(),
            vars: fallback_vars(),
        };
        let folders = env.path_dirs();
        assert!(folders.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(folders.contains(&dirs::home_dir().unwrap().join(".local/bin")));
    }

    #[test]
    fn the_login_environment_has_a_path() {
        let env = login_env();
        assert!(!env.path_dirs().is_empty());
        assert!(env.shell.is_absolute());
    }

    #[cfg(windows)]
    #[test]
    fn cmd_quoting_leaves_plain_words_alone() {
        assert_eq!(
            quote(r"C:\Users\me\code\app-1.0"),
            r"C:\Users\me\code\app-1.0"
        );
        assert_eq!(quote("a b"), r#""a b""#);
        assert_eq!(quote(r#"say "hi""#), r#""say ""hi""""#);
        assert_eq!(quote("a&b|c"), r#""a&b|c""#);
        assert_eq!(quote(""), r#""""#);
    }

    #[cfg(windows)]
    #[test]
    fn cmd_placeholders_never_run_as_commands() {
        let project = ProjectRef {
            name: "demo".into(),
            path: r"C:\Users\me\My App & echo INJECTED".into(),
        };
        let output = command_for(login_env(), &expand_command("echo {path}", &project))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            r#""C:\Users\me\My App & echo INJECTED""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn cmd_runs_chained_commands() {
        let output = command_for(login_env(), "echo one && echo two")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
        assert_eq!(lines, ["one", "two"]);
    }

    #[cfg(windows)]
    #[test]
    fn the_fallback_adds_the_usual_tool_folders_on_windows() {
        let env = ShellEnv {
            shell: user_shell(),
            vars: fallback_vars(),
        };
        let folders = env.path_dirs();
        assert!(folders.contains(&dirs::home_dir().unwrap().join(".cargo").join("bin")));
        assert!(env.shell.ends_with("cmd.exe") || env.shell.ends_with("CMD.EXE"));
    }
}

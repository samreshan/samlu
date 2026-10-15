//! The user's shell environment, and turning command and URL templates into
//! something safe to run.

use super::model::ProjectRef;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// Printed just before the environment, so whatever `.zshrc` prints first can
/// be told apart from it.
const MARKER: &str = "__SAMLU_ENV__";
/// A `.zshrc` waiting on something must not hold up the first action.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Variables that describe the shell that printed them, not the user's setup.
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

/// POSIX single-quoting. Plain words are returned unchanged.
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

fn user_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute() && is_executable(shell))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Runs the user's shell the way a terminal would, interactive and login,
/// and reads the environment it ends up with. On this kind of setup `.zshrc`
/// is where Homebrew and `~/.local/bin` join PATH, and a plain login shell
/// never reads it.
fn resolve(shell: &Path) -> Option<Vec<(String, String)>> {
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

/// Samlu's own environment with the usual tool folders added to PATH, for
/// when the login shell can't be asked.
fn fallback_vars() -> Vec<(String, String)> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for folder in [
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
    ] {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    let path = std::env::join_paths(&folders)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    std::env::vars()
        .filter(|(name, _)| name != "PATH")
        .chain([("PATH".to_string(), path)])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectRef {
        ProjectRef {
            name: "My \"App\"".into(),
            path: "/Users/me/My \"App\" $dev".into(),
        }
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
        assert_eq!(expand_command("ls {path}", &project), "ls '/tmp/{name}'");
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
}

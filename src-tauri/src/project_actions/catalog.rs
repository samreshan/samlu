//! Apps and CLIs Samlu knows how to use with a project, matched against what
//! is installed, and the first-run configuration built from them.

use super::model::{
    ActionKind, ActionsConfig, AppGroup, Catalog, CatalogApp, CatalogCli, ProjectAction, RunMode,
    TerminalApp, COMMAND_TERMINALS, FALLBACK_TERMINAL, FILE_MANAGER,
};
#[cfg(not(windows))]
use serde::Deserialize;
#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;

/// The file manager's preset. Windows keeps the same identifier for File
/// Explorer, so saved bindings mean the same thing on both.
const FINDER_BUNDLE: &str = "com.apple.finder";
#[cfg(not(windows))]
const FINDER_PATH: &str = "/System/Library/CoreServices/Finder.app";

#[cfg(not(windows))]
fn finder_path() -> String {
    FINDER_PATH.to_string()
}

#[cfg(windows)]
fn finder_path() -> String {
    crate::platform::expand_variables(r"%SystemRoot%\explorer.exe")
}

/// Apps with a preset: bundle id, slug, the name Samlu shows, and its group.
/// The name is ours rather than the bundle's, because the same app can be
/// installed under another name (the Codex app as ChatGPT.app).
const APPS: &[(&str, &str, &str, AppGroup)] = &[
    (FINDER_BUNDLE, "finder", FILE_MANAGER, AppGroup::Finder),
    (
        "com.microsoft.VSCode",
        "vscode",
        "VS Code",
        AppGroup::Editor,
    ),
    (
        "com.microsoft.VSCodeInsiders",
        "vscode-insiders",
        "VS Code Insiders",
        AppGroup::Editor,
    ),
    (
        "com.todesktop.230313mzl4w4u92",
        "cursor",
        "Cursor",
        AppGroup::Editor,
    ),
    (
        "com.exafunction.windsurf",
        "windsurf",
        "Windsurf",
        AppGroup::Editor,
    ),
    ("dev.zed.Zed", "zed", "Zed", AppGroup::Editor),
    (
        "com.google.antigravity",
        "antigravity",
        "Antigravity",
        AppGroup::Editor,
    ),
    (
        "com.google.antigravity-ide",
        "antigravity-ide",
        "Antigravity IDE",
        AppGroup::Editor,
    ),
    ("com.apple.dt.Xcode", "xcode", "Xcode", AppGroup::Editor),
    (
        "com.sublimetext.4",
        "sublime",
        "Sublime Text",
        AppGroup::Editor,
    ),
    ("com.panic.Nova", "nova", "Nova", AppGroup::Editor),
    (
        "com.jetbrains.intellij",
        "intellij",
        "IntelliJ IDEA",
        AppGroup::Editor,
    ),
    (
        "com.jetbrains.intellij.ce",
        "intellij-ce",
        "IntelliJ IDEA CE",
        AppGroup::Editor,
    ),
    (
        "com.jetbrains.WebStorm",
        "webstorm",
        "WebStorm",
        AppGroup::Editor,
    ),
    (
        "com.jetbrains.pycharm",
        "pycharm",
        "PyCharm",
        AppGroup::Editor,
    ),
    (
        "com.jetbrains.pycharm.ce",
        "pycharm-ce",
        "PyCharm CE",
        AppGroup::Editor,
    ),
    (
        "com.jetbrains.rustrover",
        "rustrover",
        "RustRover",
        AppGroup::Editor,
    ),
    ("com.jetbrains.goland", "goland", "GoLand", AppGroup::Editor),
    (
        "com.google.android.studio",
        "android-studio",
        "Android Studio",
        AppGroup::Editor,
    ),
    (
        "com.anthropic.claudefordesktop",
        "claude",
        "Claude",
        AppGroup::Agent,
    ),
    ("com.openai.codex", "codex", "Codex", AppGroup::Agent),
    (
        "com.apple.Terminal",
        "terminal",
        "Terminal",
        AppGroup::Terminal,
    ),
    (
        "Microsoft.WindowsTerminal",
        "windows-terminal",
        "Windows Terminal",
        AppGroup::Terminal,
    ),
    (
        "Microsoft.PowerShell",
        "powershell",
        "PowerShell",
        AppGroup::Terminal,
    ),
    (
        "Microsoft.CommandPrompt",
        "cmd",
        "Command Prompt",
        AppGroup::Terminal,
    ),
    ("dev.warp.Warp-Stable", "warp", "Warp", AppGroup::Terminal),
    (
        "com.googlecode.iterm2",
        "iterm",
        "iTerm",
        AppGroup::Terminal,
    ),
    (
        "com.mitchellh.ghostty",
        "ghostty",
        "Ghostty",
        AppGroup::Terminal,
    ),
    (
        "com.github.wez.wezterm",
        "wezterm",
        "WezTerm",
        AppGroup::Terminal,
    ),
    ("net.kovidgoyal.kitty", "kitty", "kitty", AppGroup::Terminal),
    (
        "org.alacritty",
        "alacritty",
        "Alacritty",
        AppGroup::Terminal,
    ),
    (
        "com.github.GitHubClient",
        "github-desktop",
        "GitHub Desktop",
        AppGroup::Git,
    ),
    ("com.DanPristupov.Fork", "fork", "Fork", AppGroup::Git),
    ("com.fournova.Tower3", "tower", "Tower", AppGroup::Git),
    (
        "com.torusknot.SourceTreeNotMAS",
        "sourcetree",
        "Sourcetree",
        AppGroup::Git,
    ),
];

/// Command-line tools with a preset: the command and the name Samlu shows.
const CLIS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex CLI"),
    ("gemini", "Gemini CLI"),
    ("agy", "Antigravity CLI"),
    ("opencode", "OpenCode"),
    ("aider", "Aider"),
    ("cursor-agent", "Cursor Agent"),
    ("amp", "Amp"),
    ("copilot", "Copilot CLI"),
    ("goose", "Goose"),
    ("lazygit", "Lazygit"),
];

/// What "Run Project" looks for, in order.
const RUN_SCRIPTS: [&str; 4] = ["dev", "start", "run", "serve"];

/// Scans the standard application folders and `path_dirs` (the login PATH).
pub fn detect(path_dirs: &[PathBuf]) -> Catalog {
    build(&installed_apps(), &|command| {
        find_on_path(command, path_dirs)
    })
}

/// The first-run configuration: every preset in `catalog`, Return bound to
/// "open-finder" and Command-Return to "reveal" — what those keys did before
/// they could be changed.
pub fn default_config(catalog: &Catalog) -> ActionsConfig {
    let mut config = ActionsConfig {
        actions: catalog.presets.clone(),
        ..ActionsConfig::default()
    };
    config
        .bindings
        .insert("enter".to_string(), "open-finder".to_string());
    config
        .bindings
        .insert("cmd+enter".to_string(), "reveal".to_string());
    config
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct BundleInfo {
    #[serde(rename = "CFBundleIdentifier")]
    identifier: Option<String>,
    #[serde(rename = "CFBundleDisplayName")]
    display_name: Option<String>,
    #[serde(rename = "CFBundleName")]
    bundle_name: Option<String>,
}

/// `CFBundleIdentifier` and display name of an app bundle.
#[cfg(not(windows))]
pub fn bundle_info(app_path: &Path) -> Option<(String, String)> {
    let info: BundleInfo = plist::from_file(app_path.join("Contents/Info.plist")).ok()?;
    let identifier = info
        .identifier
        .map(|identifier| identifier.trim().to_string())
        .filter(|identifier| !identifier.is_empty())?;
    let name = [info.display_name, info.bundle_name]
        .into_iter()
        .flatten()
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty())
        .or_else(|| {
            app_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })?;
    Some((identifier, name))
}

/// Every app bundle directly inside the standard folders, as (bundle id,
/// path), plus Finder, which lives elsewhere.
#[cfg(not(windows))]
fn installed_apps() -> Vec<(String, String)> {
    let mut folders = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = dirs::home_dir() {
        folders.push(home.join("Applications"));
    }
    let mut apps = Vec::new();
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut bundles: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "app"))
            .collect();
        bundles.sort();
        for bundle in bundles {
            if let Some((identifier, _)) = bundle_info(&bundle) {
                apps.push((identifier, bundle.to_string_lossy().to_string()));
            }
        }
    }
    apps.push((FINDER_BUNDLE.to_string(), FINDER_PATH.to_string()));
    apps
}

/// Where preset apps install on Windows. `%NAME%` is an environment
/// variable, and a `*` in a folder name matches versioned folders, newest
/// first.
#[cfg(windows)]
const WINDOWS_LOCATIONS: &[(&str, &[&str])] = &[
    (FINDER_BUNDLE, &[r"%SystemRoot%\explorer.exe"]),
    (
        "com.microsoft.VSCode",
        &[
            r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe",
            r"%ProgramFiles%\Microsoft VS Code\Code.exe",
        ],
    ),
    (
        "com.microsoft.VSCodeInsiders",
        &[
            r"%LOCALAPPDATA%\Programs\Microsoft VS Code Insiders\Code - Insiders.exe",
            r"%ProgramFiles%\Microsoft VS Code Insiders\Code - Insiders.exe",
        ],
    ),
    (
        "com.todesktop.230313mzl4w4u92",
        &[
            r"%LOCALAPPDATA%\Programs\cursor\Cursor.exe",
            r"%ProgramFiles%\cursor\Cursor.exe",
        ],
    ),
    (
        "com.exafunction.windsurf",
        &[
            r"%LOCALAPPDATA%\Programs\Windsurf\Windsurf.exe",
            r"%ProgramFiles%\Windsurf\Windsurf.exe",
        ],
    ),
    (
        "dev.zed.Zed",
        &[
            r"%LOCALAPPDATA%\Programs\Zed\Zed.exe",
            r"%ProgramFiles%\Zed\Zed.exe",
        ],
    ),
    (
        "com.google.antigravity",
        &[r"%LOCALAPPDATA%\Programs\Antigravity\Antigravity.exe"],
    ),
    (
        "com.sublimetext.4",
        &[
            r"%ProgramFiles%\Sublime Text\sublime_text.exe",
            r"%ProgramFiles%\Sublime Text 4\sublime_text.exe",
        ],
    ),
    (
        "com.jetbrains.intellij",
        &[
            r"%LOCALAPPDATA%\Programs\IntelliJ IDEA Ultimate\bin\idea64.exe",
            r"%ProgramFiles%\JetBrains\IntelliJ IDEA 2*\bin\idea64.exe",
        ],
    ),
    (
        "com.jetbrains.intellij.ce",
        &[
            r"%LOCALAPPDATA%\Programs\IntelliJ IDEA Community Edition\bin\idea64.exe",
            r"%ProgramFiles%\JetBrains\IntelliJ IDEA Community Edition*\bin\idea64.exe",
        ],
    ),
    (
        "com.jetbrains.WebStorm",
        &[
            r"%LOCALAPPDATA%\Programs\WebStorm\bin\webstorm64.exe",
            r"%ProgramFiles%\JetBrains\WebStorm*\bin\webstorm64.exe",
        ],
    ),
    (
        "com.jetbrains.pycharm",
        &[
            r"%LOCALAPPDATA%\Programs\PyCharm Professional\bin\pycharm64.exe",
            r"%ProgramFiles%\JetBrains\PyCharm 2*\bin\pycharm64.exe",
        ],
    ),
    (
        "com.jetbrains.pycharm.ce",
        &[
            r"%LOCALAPPDATA%\Programs\PyCharm Community\bin\pycharm64.exe",
            r"%ProgramFiles%\JetBrains\PyCharm Community Edition*\bin\pycharm64.exe",
        ],
    ),
    (
        "com.jetbrains.rustrover",
        &[
            r"%LOCALAPPDATA%\Programs\RustRover\bin\rustrover64.exe",
            r"%ProgramFiles%\JetBrains\RustRover*\bin\rustrover64.exe",
        ],
    ),
    (
        "com.jetbrains.goland",
        &[
            r"%LOCALAPPDATA%\Programs\GoLand\bin\goland64.exe",
            r"%ProgramFiles%\JetBrains\GoLand*\bin\goland64.exe",
        ],
    ),
    (
        "com.google.android.studio",
        &[
            r"%ProgramFiles%\Android\Android Studio\bin\studio64.exe",
            r"%LOCALAPPDATA%\Programs\Android Studio\bin\studio64.exe",
        ],
    ),
    (
        "com.anthropic.claudefordesktop",
        &[r"%LOCALAPPDATA%\AnthropicClaude\claude.exe"],
    ),
    (
        "Microsoft.WindowsTerminal",
        &[r"%LOCALAPPDATA%\Microsoft\WindowsApps\wt.exe"],
    ),
    (
        "Microsoft.PowerShell",
        &[
            r"%ProgramFiles%\PowerShell\7\pwsh.exe",
            r"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe",
        ],
    ),
    (
        "Microsoft.CommandPrompt",
        &[r"%SystemRoot%\System32\cmd.exe"],
    ),
    (
        "dev.warp.Warp-Stable",
        &[r"%LOCALAPPDATA%\Programs\Warp\warp.exe"],
    ),
    (
        "com.github.wez.wezterm",
        &[r"%ProgramFiles%\WezTerm\wezterm-gui.exe"],
    ),
    (
        "org.alacritty",
        &[r"%ProgramFiles%\Alacritty\alacritty.exe"],
    ),
    (
        "com.github.GitHubClient",
        &[r"%LOCALAPPDATA%\GitHubDesktop\GitHubDesktop.exe"],
    ),
    ("com.DanPristupov.Fork", &[r"%LOCALAPPDATA%\Fork\Fork.exe"]),
    (
        "com.torusknot.SourceTreeNotMAS",
        &[r"%LOCALAPPDATA%\SourceTree\SourceTree.exe"],
    ),
];

/// Every preset app found where its installer puts it, as (identifier,
/// executable). File Explorer is always there.
#[cfg(windows)]
fn installed_apps() -> Vec<(String, String)> {
    WINDOWS_LOCATIONS
        .iter()
        .filter_map(|(identifier, patterns)| {
            patterns
                .iter()
                .find_map(|pattern| locate(pattern))
                .map(|path| (identifier.to_string(), path.to_string_lossy().to_string()))
        })
        .collect()
}

/// The newest file matching `pattern` (see `WINDOWS_LOCATIONS`).
#[cfg(windows)]
fn locate(pattern: &str) -> Option<PathBuf> {
    let expanded = crate::platform::expand_variables(pattern);
    if expanded.contains('%') {
        // A variable that isn't set.
        return None;
    }
    let mut candidates = vec![PathBuf::new()];
    for (index, part) in expanded.split('\\').enumerate() {
        let mut next = Vec::new();
        for base in &candidates {
            match part.split_once('*') {
                None if index == 0 => next.push(PathBuf::from(format!("{part}\\"))),
                None => next.push(base.join(part)),
                Some((prefix, suffix)) => {
                    let Ok(entries) = std::fs::read_dir(base) else {
                        continue;
                    };
                    let mut found: Vec<PathBuf> = entries
                        .flatten()
                        .filter(|entry| {
                            let name = entry.file_name().to_string_lossy().to_string();
                            name.len() >= prefix.len() + suffix.len()
                                && name.starts_with(prefix)
                                && name.ends_with(suffix)
                        })
                        .map(|entry| entry.path())
                        .collect();
                    found.sort();
                    next.extend(found.into_iter().rev());
                }
            }
        }
        candidates = next;
    }
    candidates.into_iter().find(|path| is_present(path))
}

/// A file that exists. App execution aliases, such as `wt.exe`, are links
/// that only exist to be run, so they are checked without following them.
#[cfg(windows)]
pub fn is_present(path: &std::path::Path) -> bool {
    path.is_file() || std::fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_dir())
}

/// The arguments that open `folder` in an app on Windows. Most take the
/// folder itself; agent apps are only brought up.
#[cfg(windows)]
pub fn open_arguments(bundle_id: &str, folder: &str) -> Vec<String> {
    match bundle_id {
        "com.github.GitHubClient" => vec![format!("--cli-open={folder}")],
        "com.torusknot.SourceTreeNotMAS" => vec!["-f".into(), folder.into()],
        "com.anthropic.claudefordesktop" | "com.openai.codex" => Vec::new(),
        _ => vec![folder.to_string()],
    }
}

/// The terminal id ("windows-terminal", "wezterm", ...) for an app that is
/// a terminal.
#[cfg(windows)]
pub fn terminal_id(bundle_id: &str) -> Option<&'static str> {
    APPS.iter()
        .find(|(identifier, _, _, group)| *identifier == bundle_id && *group == AppGroup::Terminal)
        .map(|(_, slug, _, _)| *slug)
}

/// The first executable called `command` in `path_dirs`.
#[cfg(not(windows))]
fn find_on_path(command: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path_dirs
        .iter()
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(command))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// The first program called `command` in `path_dirs`, with any of the
/// extensions `PATHEXT` lists (`claude.cmd`, `codex.exe`, ...).
#[cfg(windows)]
fn find_on_path(command: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    let extensions: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    path_dirs
        .iter()
        .filter(|folder| folder.is_absolute())
        .find_map(|folder| {
            extensions
                .iter()
                .map(|extension| folder.join(format!("{command}{extension}")))
                .find(|candidate| is_present(candidate))
        })
}

/// The catalog for `installed` apps (bundle id, path) and whatever `find_cli`
/// can locate. Finder is always there, installed list or not.
fn build(installed: &[(String, String)], find_cli: &dyn Fn(&str) -> Option<PathBuf>) -> Catalog {
    let path_for = |bundle_id: &str| {
        installed
            .iter()
            .find(|(identifier, _)| identifier == bundle_id)
            .map(|(_, path)| path.clone())
            .or_else(|| (bundle_id == FINDER_BUNDLE).then(finder_path))
    };
    let apps: Vec<CatalogApp> = APPS
        .iter()
        .filter_map(|(bundle_id, slug, name, group)| {
            path_for(bundle_id).map(|app_path| CatalogApp {
                id: format!("open-{slug}"),
                name: name.to_string(),
                bundle_id: bundle_id.to_string(),
                app_path,
                group: *group,
            })
        })
        .collect();
    let clis: Vec<CatalogCli> = CLIS
        .iter()
        .filter_map(|(command, name)| {
            find_cli(command).map(|path| CatalogCli {
                id: format!("cli-{command}"),
                name: name.to_string(),
                command: command.to_string(),
                path: path.to_string_lossy().to_string(),
            })
        })
        .collect();
    let terminals: Vec<TerminalApp> = apps
        .iter()
        .filter(|app| app.group == AppGroup::Terminal)
        .map(|app| {
            let id = app.id.trim_start_matches("open-").to_string();
            TerminalApp {
                runs_commands: COMMAND_TERMINALS.contains(&id.as_str()),
                id,
                name: app.name.clone(),
                bundle_id: app.bundle_id.clone(),
                app_path: app.app_path.clone(),
            }
        })
        .collect();
    let default_terminal = COMMAND_TERMINALS
        .iter()
        .find(|id| terminals.iter().any(|terminal| terminal.id == **id))
        .unwrap_or(&FALLBACK_TERMINAL)
        .to_string();

    let opens_with = |group: AppGroup| {
        apps.iter()
            .filter(move |app| app.group == group)
            .map(|app| ProjectAction {
                id: app.id.clone(),
                title: format!("Open in {}", app.name),
                kind: ActionKind::Open {
                    bundle_id: app.bundle_id.clone(),
                    app_path: app.app_path.clone(),
                },
            })
    };
    let mut presets: Vec<ProjectAction> = opens_with(AppGroup::Finder).collect();
    presets.push(ProjectAction {
        id: "reveal".into(),
        title: format!("Show in {FILE_MANAGER}"),
        kind: ActionKind::Reveal,
    });
    presets.extend(opens_with(AppGroup::Editor));
    presets.extend(opens_with(AppGroup::Agent));
    presets.extend(clis.iter().map(|cli| ProjectAction {
        id: cli.id.clone(),
        title: cli.name.clone(),
        kind: ActionKind::Terminal {
            command: cli.command.clone(),
            terminal: String::new(),
            cwd: String::new(),
        },
    }));
    presets.push(ProjectAction {
        id: "run-project".into(),
        title: "Run Project".into(),
        kind: ActionKind::Script {
            scripts: RUN_SCRIPTS.iter().map(|name| name.to_string()).collect(),
            mode: RunMode::Terminal,
        },
    });
    presets.extend(opens_with(AppGroup::Terminal));
    presets.extend(opens_with(AppGroup::Git));
    presets.push(ProjectAction {
        id: "copy-path".into(),
        title: "Copy Path".into(),
        kind: ActionKind::CopyPath,
    });

    Catalog {
        apps,
        clis,
        terminals,
        presets,
        default_terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINDER: &str = "/System/Library/CoreServices/Finder.app";

    fn installed(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, path)| (id.to_string(), path.to_string()))
            .collect()
    }

    #[cfg(not(windows))]
    #[test]
    fn installed_apps_and_clis_become_presets() {
        let apps = installed(&[
            ("com.apple.finder", FINDER),
            (
                "com.microsoft.VSCode",
                "/Applications/Visual Studio Code.app",
            ),
            ("dev.warp.Warp-Stable", "/Applications/Warp.app"),
            (
                "com.apple.Terminal",
                "/System/Applications/Utilities/Terminal.app",
            ),
            ("com.openai.codex", "/Applications/ChatGPT.app"),
            ("org.example.unknown", "/Applications/Unknown.app"),
        ]);
        let catalog = build(&apps, &|command| {
            (command == "claude").then(|| PathBuf::from("/Users/me/.local/bin/claude"))
        });
        let ids: Vec<&str> = catalog
            .presets
            .iter()
            .map(|preset| preset.id.as_str())
            .collect();
        assert_eq!(&ids[..2], ["open-finder", "reveal"]);
        for id in [
            "open-vscode",
            "open-codex",
            "cli-claude",
            "run-project",
            "open-warp",
            "copy-path",
        ] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        assert!(!ids.iter().any(|id| id.contains("unknown")));
        assert!(!ids.contains(&"cli-codex"));
        assert_eq!(catalog.default_terminal, "warp");
        assert!(catalog.terminal("warp").unwrap().runs_commands);
        let codex = catalog
            .apps
            .iter()
            .find(|app| app.bundle_id == "com.openai.codex")
            .unwrap();
        assert_eq!(
            (codex.name.as_str(), codex.app_path.as_str()),
            ("Codex", "/Applications/ChatGPT.app")
        );
        let claude = catalog
            .presets
            .iter()
            .find(|preset| preset.id == "cli-claude")
            .unwrap();
        assert_eq!(claude.title, "Claude Code");
        assert_eq!(
            claude.kind,
            ActionKind::Terminal {
                command: "claude".into(),
                terminal: String::new(),
                cwd: String::new(),
            }
        );
        assert_eq!(catalog.clis[0].path, "/Users/me/.local/bin/claude");
    }

    #[cfg(not(windows))]
    #[test]
    fn presets_follow_the_documented_order() {
        let apps = installed(&[
            ("com.DanPristupov.Fork", "/Applications/Fork.app"),
            ("com.apple.Terminal", "/Applications/Terminal.app"),
            ("com.anthropic.claudefordesktop", "/Applications/Claude.app"),
            ("com.google.antigravity", "/Applications/Antigravity.app"),
        ]);
        let catalog = build(&apps, &|command| {
            (command == "codex").then(|| PathBuf::from("/opt/homebrew/bin/codex"))
        });
        let ids: Vec<&str> = catalog
            .presets
            .iter()
            .map(|preset| preset.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "open-finder",
                "reveal",
                "open-antigravity",
                "open-claude",
                "cli-codex",
                "run-project",
                "open-terminal",
                "open-fork",
                "copy-path",
            ]
        );
        assert_eq!(catalog.default_terminal, "terminal");
        assert!(catalog.terminal("terminal").unwrap().runs_commands);
    }

    #[test]
    fn terminals_without_command_support_are_marked() {
        let catalog = build(
            &installed(&[("net.kovidgoyal.kitty", "/Applications/kitty.app")]),
            &|_| None,
        );
        let kitty = catalog.terminal("kitty").unwrap();
        assert!(!kitty.runs_commands);
        assert_eq!(catalog.default_terminal, FALLBACK_TERMINAL);
    }

    #[test]
    fn finder_is_always_offered_and_terminal_is_the_fallback() {
        let catalog = build(&[], &|_| None);
        assert_eq!(catalog.presets[0].id, "open-finder");
        assert_eq!(catalog.default_terminal, FALLBACK_TERMINAL);
    }

    #[test]
    fn first_run_keeps_todays_keys() {
        let catalog = build(&installed(&[("com.apple.finder", FINDER)]), &|_| None);
        let config = default_config(&catalog);
        assert_eq!(
            config.bindings.get("enter").map(String::as_str),
            Some("open-finder")
        );
        assert_eq!(
            config.bindings.get("cmd+enter").map(String::as_str),
            Some("reveal")
        );
        assert_eq!(config.actions, catalog.presets);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_info_reads_a_real_app() {
        let (id, name) = bundle_info(Path::new(FINDER)).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("com.apple.finder", "Finder"));
        assert!(bundle_info(Path::new("/nonexistent.app")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn executables_are_found_on_the_path_in_order() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("samlu-catalog-path-{}", std::process::id()));
        let (first, second) = (root.join("first"), root.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("tool"), "").unwrap();
        std::fs::write(second.join("tool"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(second.join("tool"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let dirs = [PathBuf::from("relative"), first.clone(), second.clone()];
        assert_eq!(find_on_path("tool", &dirs), Some(second.join("tool")));
        assert_eq!(find_on_path("missing", &dirs), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_programs_are_found_by_their_extensions() {
        let root = std::env::temp_dir().join(format!("samlu-catalog-path-{}", std::process::id()));
        let (first, second) = (root.join("first"), root.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("tool.txt"), "").unwrap();
        std::fs::write(second.join("tool.cmd"), "@echo off\r\n").unwrap();
        let dirs = [PathBuf::from("relative"), first.clone(), second.clone()];
        assert_eq!(find_on_path("tool", &dirs), Some(second.join("tool.cmd")));
        assert_eq!(find_on_path("missing", &dirs), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn versioned_install_folders_pick_the_newest() {
        let root = std::env::temp_dir().join(format!("samlu-catalog-glob-{}", std::process::id()));
        for version in [
            "Tool 2024.3",
            "Tool 2025.1",
            "Tool Community Edition 2026.1",
        ] {
            let bin = root.join(version).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("tool64.exe"), "").unwrap();
        }
        let pattern = format!(r"{}\Tool 2*\bin\tool64.exe", root.display());
        assert_eq!(
            locate(&pattern),
            Some(root.join("Tool 2025.1").join("bin").join("tool64.exe"))
        );
        assert_eq!(locate(r"%SAMLU_SURELY_UNSET%\tool.exe"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminals_run_commands_and_explorer_is_the_file_manager() {
        let apps = installed(&[
            ("com.apple.finder", r"C:\Windows\explorer.exe"),
            (
                "Microsoft.PowerShell",
                r"C:\Program Files\PowerShell\7\pwsh.exe",
            ),
            (
                "Microsoft.WindowsTerminal",
                r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\wt.exe",
            ),
            (
                "dev.warp.Warp-Stable",
                r"C:\Users\me\AppData\Local\Programs\Warp\warp.exe",
            ),
        ]);
        let catalog = build(&apps, &|_| None);
        assert_eq!(catalog.default_terminal, "windows-terminal");
        assert!(catalog.terminal("powershell").unwrap().runs_commands);
        assert!(!catalog.terminal("warp").unwrap().runs_commands);
        let reveal = catalog
            .presets
            .iter()
            .find(|preset| preset.id == "reveal")
            .unwrap();
        assert_eq!(reveal.title, "Show in File Explorer");
        assert_eq!(catalog.presets[0].title, "Open in File Explorer");
        assert_eq!(terminal_id("Microsoft.CommandPrompt"), Some("cmd"));
        assert_eq!(terminal_id("com.microsoft.VSCode"), None);
        assert_eq!(
            open_arguments("com.torusknot.SourceTreeNotMAS", r"C:\code\app"),
            ["-f", r"C:\code\app"]
        );
        assert_eq!(
            open_arguments("com.microsoft.VSCode", r"C:\code\app"),
            [r"C:\code\app"]
        );
    }
}

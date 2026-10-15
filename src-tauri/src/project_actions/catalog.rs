//! Apps and CLIs Samlu knows how to use with a project, matched against what
//! is installed, and the first-run configuration built from them.

use super::model::{
    ActionKind, ActionsConfig, AppGroup, Catalog, CatalogApp, CatalogCli, ProjectAction, RunMode,
    TerminalApp, COMMAND_TERMINALS,
};
use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const FINDER_BUNDLE: &str = "com.apple.finder";
const FINDER_PATH: &str = "/System/Library/CoreServices/Finder.app";

/// Apps with a preset: bundle id, slug, the name Samlu shows, and its group.
/// The name is ours rather than the bundle's, because the same app can be
/// installed under another name (the Codex app as ChatGPT.app).
const APPS: &[(&str, &str, &str, AppGroup)] = &[
    (FINDER_BUNDLE, "finder", "Finder", AppGroup::Finder),
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

/// The first executable called `command` in `path_dirs`.
fn find_on_path(command: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
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

/// The catalog for `installed` apps (bundle id, path) and whatever `find_cli`
/// can locate. Finder is always there, installed list or not.
fn build(installed: &[(String, String)], find_cli: &dyn Fn(&str) -> Option<PathBuf>) -> Catalog {
    let path_for = |bundle_id: &str| {
        installed
            .iter()
            .find(|(identifier, _)| identifier == bundle_id)
            .map(|(_, path)| path.clone())
            .or_else(|| (bundle_id == FINDER_BUNDLE).then(|| FINDER_PATH.to_string()))
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
        .unwrap_or(&"terminal")
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
        title: "Show in Finder".into(),
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
        assert_eq!(catalog.default_terminal, "terminal");
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

    #[test]
    fn bundle_info_reads_a_real_app() {
        let (id, name) = bundle_info(Path::new(FINDER)).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("com.apple.finder", "Finder"));
        assert!(bundle_info(Path::new("/nonexistent.app")).is_none());
    }

    #[test]
    fn executables_are_found_on_the_path_in_order() {
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
}

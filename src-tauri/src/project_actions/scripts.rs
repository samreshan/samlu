//! Scripts a project defines, found in its manifest files: `package.json`,
//! `Cargo.toml`, Makefiles, justfiles, `go.mod`, `Package.swift`,
//! `deno.json`, `pyproject.toml`, `manage.py` and Compose files.

use super::model::DetectedScript;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// Subfolders searched below the project root.
const MAX_SUBFOLDERS: usize = 12;
const MAX_PER_MANIFEST: usize = 30;
const MAX_SCRIPTS: usize = 120;
/// Manifests are small; anything bigger is not one worth parsing.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Folders that hold dependencies or build output, never a project's own
/// scripts.
const SKIPPED_FOLDERS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "Pods",
    "DerivedData",
    "venv",
    "env",
];

/// Scripts the project at `root` can run: the root folder first, then its
/// immediate subfolders.
pub fn detect(root: &Path) -> Vec<DetectedScript> {
    let mut scripts = Vec::new();
    for (cwd, folder) in folders(root) {
        detect_in(root, &folder, &cwd, &mut scripts);
        if scripts.len() >= MAX_SCRIPTS {
            break;
        }
    }
    let mut seen = HashSet::new();
    scripts.retain(|script| seen.insert(script.id.clone()));
    scripts.truncate(MAX_SCRIPTS);
    scripts
}

/// The first script whose name is in `names`, trying names in order and
/// preferring the root folder for each name.
pub fn pick<'a>(scripts: &'a [DetectedScript], names: &[String]) -> Option<&'a DetectedScript> {
    // Detection lists the root folder first, so the first match by name is
    // already the root's when it has one.
    names
        .iter()
        .find_map(|name| scripts.iter().find(|script| &script.name == name))
}

/// The root, as "", then its subfolders by name, skipping hidden, private
/// and dependency folders.
fn folders(root: &Path) -> Vec<(String, PathBuf)> {
    let mut folders = vec![(String::new(), root.to_path_buf())];
    let Ok(entries) = std::fs::read_dir(root) else {
        return folders;
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let skipped = name.starts_with('.')
                || name.starts_with('_')
                || SKIPPED_FOLDERS.contains(&name.as_str());
            (!skipped).then(|| (name, entry.path()))
        })
        .collect();
    children.sort_by_key(|(name, _)| name.to_lowercase());
    folders.extend(children.into_iter().take(MAX_SUBFOLDERS));
    folders
}

fn detect_in(root: &Path, folder: &Path, cwd: &str, scripts: &mut Vec<DetectedScript>) {
    let mut add = |source: &str, found: Vec<(String, String)>| {
        for (name, command) in found.into_iter().take(MAX_PER_MANIFEST) {
            scripts.push(DetectedScript {
                id: format!("{cwd}:{source}:{name}"),
                name,
                command,
                cwd: cwd.to_string(),
                source: source.to_string(),
            });
        }
    };
    let fixed = |names: &[&str], command: &dyn Fn(&str) -> String| -> Vec<(String, String)> {
        names
            .iter()
            .map(|name| (name.to_string(), command(name)))
            .collect()
    };

    if let Some(names) = package_scripts(&folder.join("package.json")) {
        let runner = package_runner(folder, root);
        add("package.json", with_prefix(runner, names));
    }
    if is_cargo_package(&folder.join("Cargo.toml")) {
        add(
            "Cargo.toml",
            fixed(&["run", "test", "build"], &|name| format!("cargo {name}")),
        );
    }
    if let Some(file) = first_file(folder, &["Makefile", "makefile", "GNUmakefile"]) {
        add("Makefile", with_prefix("make", make_targets(&read(&file))));
    }
    if let Some(file) = first_file(folder, &["justfile", "Justfile", ".justfile"]) {
        add("justfile", with_prefix("just", just_recipes(&read(&file))));
    }
    if folder.join("go.mod").is_file() {
        add(
            "go.mod",
            vec![
                ("run".into(), "go run .".into()),
                ("test".into(), "go test ./...".into()),
                ("build".into(), "go build ./...".into()),
            ],
        );
    }
    if folder.join("Package.swift").is_file() {
        add(
            "Package.swift",
            fixed(&["run", "test", "build"], &|name| format!("swift {name}")),
        );
    }
    if let Some(names) = deno_tasks(&folder.join("deno.json")) {
        add("deno.json", with_prefix("deno task", names));
    }
    if let Some(names) = python_scripts(&folder.join("pyproject.toml")) {
        let lockfile = |name: &str| folder.join(name).is_file() || root.join(name).is_file();
        let runner = if lockfile("uv.lock") {
            "uv run"
        } else if lockfile("poetry.lock") {
            "poetry run"
        } else {
            ""
        };
        add("pyproject.toml", with_prefix(runner, names));
    }
    if folder.join("manage.py").is_file() {
        add(
            "manage.py",
            vec![("runserver".into(), "python3 manage.py runserver".into())],
        );
    }
    let compose = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ];
    if let Some(file) = first_file(folder, &compose) {
        let source = file
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        add(
            &source,
            fixed(&["up", "down"], &|name| format!("docker compose {name}")),
        );
    }
}

/// Pairs each name with `<prefix> <name>`, the name quoted when it isn't a
/// plain word, so an odd script name still reaches the tool as one argument.
fn with_prefix(prefix: &str, names: Vec<String>) -> Vec<(String, String)> {
    names
        .into_iter()
        .map(|name| {
            let command = if prefix.is_empty() {
                word(&name)
            } else {
                format!("{prefix} {}", word(&name))
            };
            (name, command)
        })
        .collect()
}

fn word(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if plain {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', r"'\''"))
    }
}

fn first_file(folder: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| folder.join(name))
        .find(|path| path.is_file())
}

/// A manifest's text, or nothing when it is missing, unreadable or too big
/// to be a manifest.
fn read(path: &Path) -> String {
    let small = std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= MAX_MANIFEST_BYTES);
    if !small {
        return String::new();
    }
    std::fs::read_to_string(path).unwrap_or_default()
}

/* ------------------------------------------------------------- package.json */

/// An object's entries in the order the file lists them. This crate's
/// `serde_json` sorts object keys, so `package.json` scripts are read entry
/// by entry to keep the order the author chose.
#[derive(Default)]
struct OrderedEntries(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for OrderedEntries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = OrderedEntries;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, serde_json::Value>()? {
                    entries.push(entry);
                }
                Ok(OrderedEntries(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

#[derive(Deserialize)]
struct PackageJson {
    #[serde(default)]
    scripts: OrderedEntries,
}

#[derive(Deserialize)]
struct DenoJson {
    #[serde(default)]
    tasks: OrderedEntries,
}

fn package_scripts(path: &Path) -> Option<Vec<String>> {
    let package: PackageJson = serde_json::from_str(&read(path)).ok()?;
    Some(
        package
            .scripts
            .0
            .into_iter()
            .filter(|(_, command)| command.is_string())
            .map(|(name, _)| name)
            .collect(),
    )
}

/// The package manager the nearest lockfile names, in the folder or the
/// project root.
fn package_runner(folder: &Path, root: &Path) -> &'static str {
    for dir in [folder, root] {
        if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
            return "bun run";
        }
        if dir.join("pnpm-lock.yaml").is_file() {
            return "pnpm run";
        }
        if dir.join("yarn.lock").is_file() {
            return "yarn run";
        }
        if dir.join("package-lock.json").is_file() {
            return "npm run";
        }
    }
    "npm run"
}

/// Deno tasks are a command string or an object with one.
fn deno_tasks(path: &Path) -> Option<Vec<String>> {
    let deno: DenoJson = serde_json::from_str(&read(path)).ok()?;
    Some(
        deno.tasks
            .0
            .into_iter()
            .filter(|(_, task)| task.is_string() || task.is_object())
            .map(|(name, _)| name)
            .collect(),
    )
}

/* ------------------------------------------------------------------ TOML */

/// A workspace-only `Cargo.toml` has nothing of its own to run.
fn is_cargo_package(path: &Path) -> bool {
    read(path)
        .parse::<toml_edit::DocumentMut>()
        .is_ok_and(|document| {
            document
                .get("package")
                .is_some_and(|item| item.is_table_like())
        })
}

fn python_scripts(path: &Path) -> Option<Vec<String>> {
    let document = read(path).parse::<toml_edit::DocumentMut>().ok()?;
    let tables = [
        document
            .get("project")
            .and_then(|project| project.get("scripts")),
        document
            .get("tool")
            .and_then(|tool| tool.get("poetry"))
            .and_then(|poetry| poetry.get("scripts")),
    ];
    let mut names: Vec<String> = Vec::new();
    for table in tables
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_table_like())
    {
        for (name, _) in table.iter() {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
    }
    (!names.is_empty()).then_some(names)
}

/* --------------------------------------------------- Makefile and justfile */

/// Explicit targets: a line at the start of the line naming plain words
/// before a `:` that isn't an assignment (`:=`, `::=`). Special targets
/// (`.PHONY`), pattern rules (`%.o`) and anything with variables are
/// skipped, as is the body of a `define`.
fn make_targets(contents: &str) -> Vec<String> {
    let plain = |name: &str| {
        name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    let mut targets: Vec<String> = Vec::new();
    let mut in_define = false;
    for line in contents.lines() {
        if in_define {
            in_define = !line.trim_start().starts_with("endef");
            continue;
        }
        if line.starts_with("define ") {
            in_define = true;
            continue;
        }
        if line.is_empty() || line.starts_with(['\t', ' ', '#']) {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let after = &line[colon + 1..];
        if after.starts_with('=') || after.starts_with(":=") {
            continue;
        }
        let names: Vec<&str> = line[..colon].split_whitespace().collect();
        if names.is_empty() || !names.iter().all(|name| plain(name)) {
            continue;
        }
        for name in names {
            if !targets.iter().any(|target| target == name) {
                targets.push(name.to_string());
            }
        }
        if targets.len() >= MAX_PER_MANIFEST {
            break;
        }
    }
    targets.truncate(MAX_PER_MANIFEST);
    targets
}

/// Recipes: an unindented line, optionally starting with `@`, whose first
/// word is the name and which has a `:` that is not part of `:=`. Parameters
/// may carry defaults (`build target='debug':`). Settings, aliases, imports,
/// assignments and private (`_`) recipes are skipped.
fn just_recipes(contents: &str) -> Vec<String> {
    let mut recipes: Vec<String> = Vec::new();
    for line in contents.lines() {
        if line.is_empty() || line.starts_with([' ', '\t', '#', '[']) {
            continue;
        }
        let line = line.strip_prefix('@').unwrap_or(line);
        let end = line
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(line.len());
        let (name, rest) = line.split_at(end);
        let keyword = matches!(name, "set" | "alias" | "import" | "export" | "mod");
        if name.is_empty()
            || keyword
            || !name.starts_with(|c: char| c.is_ascii_alphanumeric())
            || rest.trim_start().starts_with(":=")
        {
            continue;
        }
        let bytes = rest.as_bytes();
        let header_ends = bytes
            .iter()
            .enumerate()
            .any(|(index, &byte)| byte == b':' && bytes.get(index + 1) != Some(&b'='));
        if header_ends && !recipes.iter().any(|recipe| recipe == name) {
            recipes.push(name.to_string());
        }
        if recipes.len() >= MAX_PER_MANIFEST {
            break;
        }
    }
    recipes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("samlu-scripts-{name}-{}", std::process::id()));
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
        scripts
            .iter()
            .map(|script| (script.id.clone(), script.command.clone()))
            .collect()
    }

    #[test]
    fn package_scripts_keep_file_order_and_follow_the_lockfile() {
        let root = project(
            "bun",
            &[
                (
                    "package.json",
                    r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest","bad":3}}"#,
                ),
                ("bun.lock", ""),
            ],
        );
        assert_eq!(
            pairs(&detect(&root)),
            vec![
                (":package.json:dev".to_string(), "bun run dev".to_string()),
                (
                    ":package.json:build".to_string(),
                    "bun run build".to_string()
                ),
                (":package.json:test".to_string(), "bun run test".to_string()),
            ]
        );
        for (lockfile, runner) in [
            ("pnpm-lock.yaml", "pnpm run dev"),
            ("yarn.lock", "yarn run dev"),
            ("package-lock.json", "npm run dev"),
        ] {
            let root = project(
                lockfile,
                &[
                    ("package.json", r#"{"scripts":{"dev":"x"}}"#),
                    (lockfile, ""),
                ],
            );
            assert_eq!(detect(&root)[0].command, runner);
        }
    }

    #[test]
    fn a_monorepo_package_uses_the_root_lockfile() {
        let root = project(
            "monorepo",
            &[
                ("pnpm-lock.yaml", ""),
                ("web/package.json", r#"{"scripts":{"dev":"next dev"}}"#),
            ],
        );
        let scripts = detect(&root);
        assert_eq!(scripts[0].command, "pnpm run dev");
        assert_eq!(scripts[0].cwd, "web");
    }

    #[test]
    fn manifests_in_subfolders_are_found_and_dependency_folders_skipped() {
        let root = project(
            "tauri",
            &[
                ("src-tauri/Cargo.toml", "[package]\nname = \"samlu\"\n"),
                (
                    "node_modules/left-pad/package.json",
                    r#"{"scripts":{"dev":"x"}}"#,
                ),
                (".hidden/package.json", r#"{"scripts":{"dev":"x"}}"#),
                ("workspace/Cargo.toml", "[workspace]\nmembers = []\n"),
            ],
        );
        let scripts = detect(&root);
        assert_eq!(
            pairs(&scripts),
            vec![
                (
                    "src-tauri:Cargo.toml:run".to_string(),
                    "cargo run".to_string()
                ),
                (
                    "src-tauri:Cargo.toml:test".to_string(),
                    "cargo test".to_string()
                ),
                (
                    "src-tauri:Cargo.toml:build".to_string(),
                    "cargo build".to_string()
                ),
            ]
        );
        assert_eq!(scripts[0].cwd, "src-tauri");
        assert_eq!(scripts[0].source, "Cargo.toml");
    }

    #[test]
    fn makefile_targets_skip_variables_patterns_and_specials() {
        let root = project(
            "make",
            &[(
                "Makefile",
                "CC := gcc\nbuild:\n\tcc main.c\n.PHONY: build test\ntest: build\n\t./t\n%.o: %.c\n\tcc\nall clean:\n\techo\nbuild:\n",
            )],
        );
        let names: Vec<String> = detect(&root)
            .into_iter()
            .map(|script| script.command)
            .collect();
        assert_eq!(names, ["make build", "make test", "make all", "make clean"]);
    }

    #[test]
    fn makefile_definitions_and_assignments_are_not_targets() {
        let targets = make_targets(
            "define HELP\nusage: make it\nendef\nVERSION ::= 1\nexport PATH := x:y\nURL = http://x\nrelease: test\n",
        );
        assert_eq!(targets, ["release"]);
    }

    #[test]
    fn justfile_recipes_skip_settings_aliases_and_private_ones() {
        let root = project(
            "just",
            &[(
                "justfile",
                "set shell := [\"zsh\", \"-c\"]\ndefault:\n  just --list\nbuild target='debug':\n  cargo build\n@test *args:\n  cargo test\nalias b := build\n_hidden:\n  true\n",
            )],
        );
        let names: Vec<String> = detect(&root)
            .into_iter()
            .map(|script| script.command)
            .collect();
        assert_eq!(names, ["just default", "just build", "just test"]);
    }

    #[test]
    fn justfile_assignments_are_not_recipes() {
        assert_eq!(
            just_recipes("version := 'a:b'\n[private]\nlint:\n  true\n"),
            ["lint"]
        );
    }

    #[test]
    fn other_ecosystems_are_recognised() {
        let root = project(
            "mixed",
            &[
                ("go.mod", "module x\n"),
                ("deno.json", r#"{"tasks":{"start":"deno run main.ts"}}"#),
                (
                    "pyproject.toml",
                    "[project.scripts]\nserve = \"app:main\"\n",
                ),
                ("uv.lock", ""),
                ("compose.yaml", "services: {}\n"),
            ],
        );
        let commands: Vec<String> = detect(&root)
            .into_iter()
            .map(|script| script.command)
            .collect();
        for expected in [
            "go run .",
            "go test ./...",
            "deno task start",
            "uv run serve",
            "docker compose up",
        ] {
            assert!(
                commands.contains(&expected.to_string()),
                "{expected} missing from {commands:?}"
            );
        }
    }

    #[test]
    fn odd_script_names_stay_one_word() {
        let root = project(
            "odd",
            &[(
                "package.json",
                r#"{"scripts":{"dev:web":"x","it's odd":"y"}}"#,
            )],
        );
        let commands: Vec<String> = detect(&root)
            .into_iter()
            .map(|script| script.command)
            .collect();
        assert_eq!(commands, ["npm run dev:web", r"npm run 'it'\''s odd'"]);
    }

    #[test]
    fn empty_and_malformed_projects_have_no_scripts() {
        assert!(detect(&project("empty", &[])).is_empty());
        assert!(detect(&project(
            "broken",
            &[("package.json", "{"), ("Cargo.toml", "[[[")]
        ))
        .is_empty());
    }

    #[test]
    fn pick_follows_name_order_then_prefers_the_root() {
        let root = project(
            "pick",
            &[
                (
                    "package.json",
                    r#"{"scripts":{"dev":"vite","run":"node ."}}"#,
                ),
                ("api/Cargo.toml", "[package]\nname = \"api\"\n"),
            ],
        );
        let scripts = detect(&root);
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick(&scripts, &names(&["dev", "run"])).unwrap().id,
            ":package.json:dev"
        );
        assert_eq!(
            pick(&scripts, &names(&["run"])).unwrap().id,
            ":package.json:run"
        );
        assert_eq!(
            pick(&scripts, &names(&["test"])).unwrap().id,
            "api:Cargo.toml:test"
        );
        assert!(pick(&scripts, &names(&["serve"])).is_none());
    }
}

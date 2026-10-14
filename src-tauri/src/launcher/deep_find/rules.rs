//! What Descriptive Find reads. Adapted from DigUp's `Rules`: only the
//! folders you choose, never keys or password files, never hidden or
//! developer folders, and code only when code search is on.

use std::path::Path;

/// What kind of file a passage came from; stored in the index and used for
/// the result's label and the query's kind hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Doc,
    Pdf,
    Code,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Doc => "doc",
            Kind::Pdf => "pdf",
            Kind::Code => "code",
        }
    }
}

/// Text documents are read whole up to this size; anything bigger is
/// almost always a log or a data export.
pub const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Folders never worth walking into, wherever they are.
const SKIPPED_FOLDERS: &[&str] = &[
    "node_modules",
    "bower_components",
    "venv",
    "site-packages",
    "__pycache__",
    "DerivedData",
    "Pods",
    "Carthage",
    "$RECYCLE.BIN",
    "System Volume Information",
    "AppData",
];

/// Other people's code kept in a repo and what builds leave behind. Code
/// search doesn't go into them.
const CODE_SKIPPED_FOLDERS: &[&str] = &[
    "vendor",
    "Vendor",
    "vendored",
    "third_party",
    "third-party",
    "thirdparty",
    "3rdparty",
    "external",
    "extern",
    "deps",
    "build",
    "Build",
    "dist",
    "out",
    "target",
    "obj",
    "coverage",
    "htmlcov",
    "generated",
    "__generated__",
    "wandb",
    "mlruns",
    "lightning_logs",
];

/// Any of these makes a folder a code project.
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "Package.swift",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
    "CMakeLists.txt",
];

const DOC_EXTENSIONS: &[&str] = &[
    "txt", "text", "md", "markdown", "mdx", "rst", "org", "adoc", "tex", "rtf", "docx", "odt",
    "html", "htm",
];

const SOURCE_EXTENSIONS: &[&str] = &[
    "py", "pyi", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "swift", "ts", "tsx", "mts",
    "cts", "js", "jsx", "mjs", "cjs", "go", "rs", "java", "kt", "kts", "scala", "groovy", "rb",
    "php", "cs", "fs", "sh", "bash", "zsh", "fish", "ps1", "lua", "pl", "pm", "r", "jl", "dart",
    "sql", "vue", "svelte", "astro", "css", "scss", "sass", "less", "gradle", "cmake", "proto",
    "graphql", "gql", "tf", "hcl", "nix", "zig", "hs", "ex", "exs", "erl", "clj", "cljs", "elm",
    "ml", "mli", "vim", "el", "lisp", "metal", "glsl", "hlsl", "wgsl", "cu", "toml", "yaml", "yml",
];

const CODE_FILE_NAMES: &[&str] = &[
    "Makefile",
    "makefile",
    "GNUmakefile",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "justfile",
    "Rakefile",
    "Gemfile",
    "Podfile",
    "Brewfile",
    "Procfile",
    "Vagrantfile",
    "Jenkinsfile",
];

/// Keys, certificates, password and credential files: never read.
pub fn is_secret(name: &str) -> bool {
    let lower = name.to_lowercase();
    const SECRET_TYPES: &[&str] = &[
        "pem",
        "key",
        "p12",
        "pfx",
        "keystore",
        "jks",
        "ppk",
        "kdbx",
        "gpg",
        "asc",
        "crt",
        "cer",
        "der",
        "p8",
        "mobileprovision",
    ];
    if let Some(ext) = Path::new(&lower).extension().and_then(|ext| ext.to_str()) {
        if SECRET_TYPES.contains(&ext) {
            return true;
        }
    }
    [
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        ".env",
        "secret",
        "credential",
        ".netrc",
        ".npmrc",
        ".pypirc",
        "htpasswd",
        ".htpasswd",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

/// What to make of a file: its kind, or `None` to leave it alone.
pub fn classify(path: &Path, include_code: bool) -> Option<Kind> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with('.') || is_secret(name) {
        return None;
    }
    let lower = name.to_lowercase();
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if ext == "pdf" {
        return Some(Kind::Pdf);
    }
    if DOC_EXTENSIONS.contains(&ext.as_str()) {
        return Some(Kind::Doc);
    }
    if include_code
        && (SOURCE_EXTENSIONS.contains(&ext.as_str())
            || CODE_FILE_NAMES.contains(&name)
            || name.starts_with("Dockerfile"))
    {
        if ["license", "licence", "copying", "notice"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return None;
        }
        return Some(Kind::Code);
    }
    None
}

/// Whether the crawl goes into `folder`. Hidden folders, bundles and
/// developer folders are skipped; inside code projects only when code search
/// is on (a project's own files flood an index of your documents otherwise).
/// A root you picked yourself is always walked.
pub fn walks_into(folder: &Path, is_root: bool, include_code: bool, excluded: &[String]) -> bool {
    if is_root {
        return true;
    }
    let Some(name) = folder.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.starts_with('.')
        || name.ends_with(".noindex")
        || name.ends_with(".app")
        || name.ends_with(".photoslibrary")
        || name.ends_with(".xcodeproj")
        || name.ends_with(".xcworkspace")
        || SKIPPED_FOLDERS.contains(&name)
    {
        return false;
    }
    if let Some(home) = dirs::home_dir() {
        if folder == home.join("Library") {
            return false;
        }
    }
    let folder_str = folder.to_string_lossy();
    if excluded.iter().any(|path| is_inside(&folder_str, path)) {
        return false;
    }
    if include_code {
        !CODE_SKIPPED_FOLDERS.contains(&name)
    } else {
        !is_project(folder)
    }
}

/// Whether a change the watcher reports at `path` is somewhere the crawl of
/// one of `roots` goes.
pub fn is_walked(path: &Path, roots: &[String], include_code: bool, excluded: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    let Some(root) = roots.iter().find(|root| is_inside(&path_str, root)) else {
        return false;
    };
    let root_path = Path::new(root);
    let Ok(relative) = path.strip_prefix(root_path) else {
        return false;
    };
    let mut folder = root_path.to_path_buf();
    let parts: Vec<_> = relative.components().collect();
    // Every folder between the root and the file must be walked into.
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        folder.push(part);
        if !walks_into(&folder, false, include_code, excluded) {
            return false;
        }
    }
    true
}

pub fn is_inside(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches(['/', '\\']);
    path == folder || (path.starts_with(folder) && path[folder.len()..].starts_with(['/', '\\']))
}

fn is_project(folder: &Path) -> bool {
    PROJECT_MARKERS
        .iter()
        .any(|marker| folder.join(marker).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn documents_are_read_and_secrets_never() {
        assert_eq!(classify(Path::new("/a/Lease.PDF"), false), Some(Kind::Pdf));
        assert_eq!(classify(Path::new("/a/notes.md"), false), Some(Kind::Doc));
        assert_eq!(
            classify(Path::new("/a/report.docx"), false),
            Some(Kind::Doc)
        );
        assert_eq!(classify(Path::new("/a/.env.local"), true), None);
        assert_eq!(classify(Path::new("/a/server.key"), true), None);
        assert_eq!(classify(Path::new("/a/id_ed25519"), true), None);
    }

    #[test]
    fn code_only_when_code_search_is_on() {
        assert_eq!(classify(Path::new("/a/main.rs"), false), None);
        assert_eq!(classify(Path::new("/a/main.rs"), true), Some(Kind::Code));
        assert_eq!(classify(Path::new("/a/Dockerfile"), true), Some(Kind::Code));
        assert_eq!(classify(Path::new("/a/app.min.js"), true), None);
        assert_eq!(classify(Path::new("/a/LICENSE.md"), true), Some(Kind::Doc));
    }

    #[test]
    fn hidden_and_developer_folders_are_skipped() {
        assert!(!walks_into(Path::new("/x/.git"), false, true, &[]));
        assert!(!walks_into(Path::new("/x/node_modules"), false, true, &[]));
        assert!(!walks_into(Path::new("/x/target"), false, true, &[]));
        assert!(walks_into(Path::new("/x/target"), true, true, &[]));
        assert!(!walks_into(
            Path::new("/x/private/tax"),
            false,
            false,
            &["/x/private".to_string()]
        ));
    }

    #[test]
    fn watcher_paths_respect_the_crawl() {
        let roots = vec!["/Users/a/Documents".to_string()];
        assert!(is_walked(
            &PathBuf::from("/Users/a/Documents/work/plan.md"),
            &roots,
            false,
            &[]
        ));
        assert!(!is_walked(
            &PathBuf::from("/Users/a/Documents/.cache/plan.md"),
            &roots,
            false,
            &[]
        ));
        assert!(!is_walked(
            &PathBuf::from("/Users/a/Desktop/plan.md"),
            &roots,
            false,
            &[]
        ));
        assert!(!is_walked(
            &PathBuf::from("/Users/a/DocumentsOld/plan.md"),
            &roots,
            false,
            &[]
        ));
    }
}

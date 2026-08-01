use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
}

pub struct ProjectIndex {
    projects: Mutex<Vec<ProjectEntry>>,
}

impl ProjectIndex {
    pub fn new(roots: &[String], excluded_paths: &[String]) -> Self {
        let index = Self {
            projects: Mutex::new(Vec::new()),
        };
        index.rebuild(roots, excluded_paths);
        index
    }

    pub fn rebuild(&self, roots: &[String], excluded_paths: &[String]) -> usize {
        let mut projects = Vec::new();
        let exclusions: Vec<PathBuf> = excluded_paths.iter().map(PathBuf::from).collect();
        for root in roots.iter().map(PathBuf::from).filter(|path| path.is_dir()) {
            scan(&root, 0, &exclusions, &mut projects);
        }
        projects.sort_by_cached_key(|project| project.name.to_lowercase());
        projects.dedup_by(|left, right| left.path == right.path);
        let count = projects.len();
        *self.projects.lock().unwrap() = projects;
        count
    }

    pub fn len(&self) -> usize {
        self.projects.lock().unwrap().len()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<ProjectEntry> {
        let query = query.to_lowercase();
        self.projects
            .lock()
            .unwrap()
            .iter()
            .filter(|project| project.name.to_lowercase().contains(&query))
            .take(limit)
            .cloned()
            .collect()
    }
}

fn scan(
    directory: &Path,
    depth: usize,
    excluded_paths: &[PathBuf],
    projects: &mut Vec<ProjectEntry>,
) {
    if depth > 2
        || excluded_paths
            .iter()
            .any(|excluded| directory.starts_with(excluded))
        || is_generated_directory(directory)
    {
        return;
    }
    if directory.join(".git").exists() {
        if let Some(name) = directory.file_name().and_then(|name| name.to_str()) {
            projects.push(ProjectEntry {
                name: name.to_string(),
                path: directory.to_string_lossy().to_string(),
            });
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            scan(&path, depth + 1, excluded_paths, projects);
        }
    }
}

fn is_generated_directory(directory: &Path) -> bool {
    matches!(
        directory.file_name().and_then(|name| name.to_str()),
        Some("node_modules" | "target" | ".build" | "dist")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_paths_are_not_indexed() {
        let root = std::env::temp_dir().join(format!("samlu-project-index-{}", std::process::id()));
        let included = root.join("included");
        let excluded = root.join("excluded");
        std::fs::create_dir_all(included.join(".git")).unwrap();
        std::fs::create_dir_all(excluded.join(".git")).unwrap();

        let index = ProjectIndex::new(
            &[root.to_string_lossy().to_string()],
            &[excluded.to_string_lossy().to_string()],
        );
        let matches = index.search("", 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "included");
        let _ = std::fs::remove_dir_all(root);
    }
}

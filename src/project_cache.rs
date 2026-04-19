//! Per-project cache directory under `~/.crane/projects/<slug>/`.
//!
//! Central hook for any feature that wants to persist data keyed by a
//! project: branch-picker collapsed state, commit-tree indices, file
//! content indexing, search caches, per-repo LSP artifacts, etc.
//!
//! The slug is derived from the Project's absolute path so two projects
//! named "api" in different directories don't collide.

use std::path::{Path, PathBuf};

/// Root for all per-project caches (`~/.crane/projects/`). Missing dirs
/// are created on demand by callers via [`ensure_project_dir`].
pub fn root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(format!("{home}/.crane/projects")))
}

/// Stable slug for a project path — last path component plus an 8-char
/// hex digest of the full absolute path. Safe to use as a directory
/// name on every platform we target.
pub fn slug_for(project_path: &Path) -> String {
    let name = project_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let digest = crate::util::hash64(project_path) as u32;
    format!("{sanitized}-{digest:08x}")
}

/// Returns (and creates if missing) the cache dir for `project_path`.
/// Public API — first consumers will be the commit-tree index and
/// fuzzy-file-finder recent list.
#[allow(dead_code)]
pub fn ensure_project_dir(project_path: &Path) -> Option<PathBuf> {
    let dir = root()?.join(slug_for(project_path));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Convenience: path to a named file inside the project's cache dir.
/// Caller is responsible for creating/reading/writing the file itself.
#[allow(dead_code)]
pub fn file(project_path: &Path, name: &str) -> Option<PathBuf> {
    Some(ensure_project_dir(project_path)?.join(name))
}

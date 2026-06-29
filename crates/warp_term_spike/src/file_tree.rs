//! Lazy filesystem tree for the Files pane: given a root dir and the set of
//! expanded dirs, produce the flat list of visible rows (directories first,
//! then files, alphabetical — matching Crane's Files tree order). Only
//! expanded directories are read, so deep trees stay cheap.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct FileRow {
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub name: String,
    pub path: PathBuf,
}

pub fn build_rows(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<FileRow> {
    let mut rows = Vec::new();
    walk(root, 0, expanded, &mut rows);
    rows
}

fn walk(dir: &Path, depth: usize, expanded: &HashSet<PathBuf>, rows: &mut Vec<FileRow>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(bool, String, PathBuf)> = read
        .flatten()
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            let name = e.file_name().to_string_lossy().to_string();
            (is_dir, name, path)
        })
        .collect();
    // Directories first, then files; each group alphabetical (case-insensitive).
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    for (is_dir, name, path) in entries {
        let is_expanded = is_dir && expanded.contains(&path);
        rows.push(FileRow {
            depth,
            is_dir,
            expanded: is_expanded,
            name,
            path: path.clone(),
        });
        if is_expanded {
            walk(&path, depth + 1, expanded, rows);
        }
    }
}

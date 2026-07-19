//! Small shared primitives that don't belong to any single subsystem.

use std::path::{Path, PathBuf};

/// Cross-platform home directory. On Unix reads `$HOME`; on Windows
/// falls back to `%USERPROFILE%` when `$HOME` is unset.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(not(unix))]
            {
                std::env::var("USERPROFILE").ok().map(PathBuf::from)
            }
            #[cfg(unix)]
            None
        })
}

/// Walks up from `start`'s parent directory, invoking `matches` on each
/// ancestor until it returns `true` or the filesystem root is reached.
/// If `start` is a file path, walking begins at its parent; if it's a
/// directory, walking begins at that directory. Returns the matching
/// directory, not any inner file name the predicate checked.
pub fn find_ancestor<F: FnMut(&Path) -> bool>(start: &Path, mut matches: F) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    if cur.as_os_str().is_empty() {
        return None;
    }
    loop {
        if matches(&cur) {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

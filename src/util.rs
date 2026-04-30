//! Small shared primitives that don't belong to any single subsystem.

use std::hash::{Hash, Hasher};
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

/// Home directory with a non-None fallback. Returns `home_dir()` when
/// available, otherwise the current working directory. Use for file
/// dialogs and workspace defaults where an empty path would break.
pub fn home_dir_or_cwd() -> PathBuf {
    home_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// Atomic-ish file replacement: write to `dst` by first writing to a
/// temp file, then renaming. On Windows `std::fs::rename` fails when
/// the destination already exists (AccessDenied), so we delete the
/// old file first. On Unix the rename atomically replaces.
pub fn replace_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::rename(tmp, dst)
    }
    #[cfg(not(unix))]
    {
        if dst.exists() {
            std::fs::remove_file(dst)?;
        }
        std::fs::rename(tmp, dst)
    }
}

/// One-liner over `DefaultHasher`. Use for cache keys whose components
/// all implement `Hash`. Not cryptographic — don't use for anything
/// exposed to users.
pub fn hash64<H: Hash>(value: H) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
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

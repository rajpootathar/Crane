//! Small shared primitives that don't belong to any single subsystem.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

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

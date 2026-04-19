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
/// Replaces the four ad-hoc ancestor loops in lsp/format/eslint/git.
pub fn find_ancestor<F: Fn(&Path) -> bool>(start: &Path, matches: F) -> Option<PathBuf> {
    let mut cur = start.parent().unwrap_or(start).to_path_buf();
    loop {
        if matches(&cur) {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

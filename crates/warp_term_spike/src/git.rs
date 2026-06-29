//! Minimal git shell-out for the Changes tab — `git status --porcelain` in a
//! worktree, parsed into status + path. Matches Crane's rule of shelling out
//! to the `git` binary (never libgit2).

use std::path::Path;
use std::process::Command;

pub struct Change {
    /// Porcelain XY status, trimmed (e.g. "M", "A", "D", "??", "R").
    pub status: String,
    pub path: String,
}

/// Working-tree changes in `root`, or empty on any error / non-repo.
pub fn changes(root: &Path) -> Vec<Change> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            if l.len() < 4 {
                return None;
            }
            let status = l[..2].trim().to_string();
            let path = l[3..].to_string();
            Some(Change { status, path })
        })
        .collect()
}

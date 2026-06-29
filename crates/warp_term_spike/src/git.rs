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
            // Porcelain v1: "XY <path>" (or "XY <old> -> <new>" for renames).
            // Index by char count, not bytes, to stay panic-safe on unicode.
            if l.chars().count() < 4 {
                return None;
            }
            let xy: String = l.chars().take(2).collect();
            // Single normalized status letter (most significant).
            let status = if xy.contains('?') {
                "?"
            } else if xy.contains('A') {
                "A"
            } else if xy.contains('D') {
                "D"
            } else if xy.contains('R') {
                "R"
            } else if xy.contains('C') {
                "C"
            } else if xy.contains('M') {
                "M"
            } else if xy.contains('U') {
                "U"
            } else {
                xy.trim()
            }
            .to_string();
            let rest: String = l.chars().skip(3).collect();
            // Renames/copies: "old -> new" — show the new path.
            let path = rest
                .rsplit(" -> ")
                .next()
                .unwrap_or(&rest)
                .trim_matches('"')
                .to_string();
            Some(Change { status, path })
        })
        .collect()
}

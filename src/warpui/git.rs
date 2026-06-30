//! Minimal git shell-out for the Changes tab — `git status --porcelain` in a
//! worktree, parsed into status + path. Matches Crane's rule of shelling out
//! to the `git` binary (never libgit2).

use std::path::Path;
use std::process::Command;

pub struct Change {
    /// Porcelain XY status, trimmed (e.g. "M", "A", "D", "??", "R").
    pub status: String,
    pub path: String,
    /// True if the change is staged (index column X is set).
    pub staged: bool,
}

fn run(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `git add -- <path>` (1:1 old Crane).
pub fn stage(repo: &Path, path: &str) -> Result<(), String> {
    run(repo, &["add", "--", path])
}

/// `git restore --staged -- <path>` (1:1 old Crane).
pub fn unstage(repo: &Path, path: &str) -> Result<(), String> {
    run(repo, &["restore", "--staged", "--", path])
}

/// `git commit -m <message>` (1:1 old Crane).
pub fn commit(repo: &Path, message: &str) -> Result<(), String> {
    run(repo, &["commit", "-m", message])
}

/// `git push` non-interactively (1:1 old Crane). Network op — call off-thread.
pub fn push(repo: &Path) -> Result<(), String> {
    run(repo, &["push"])
}

/// Current branch name in `root` (or a short SHA when detached), empty on error.
pub fn current_branch(root: &Path) -> String {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git log --oneline --graph --decorate -n 300` in `root`, as lines.
pub fn log(root: &Path) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            "--oneline",
            "--graph",
            "--decorate",
            "--all",
            "-n",
            "300",
        ])
        .output()
    else {
        return vec!["<git not available>".to_string()];
    };
    if !out.status.success() {
        return vec!["<not a git repository>".to_string()];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
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
            // Index column (X) set and not untracked → staged.
            let staged = {
                let x = xy.chars().next().unwrap_or(' ');
                x != ' ' && x != '?'
            };
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
            Some(Change {
                status,
                path,
                staged,
            })
        })
        .collect()
}

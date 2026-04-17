use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
}

pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

pub fn list_worktrees(repo: &Path) -> Vec<WorktreeInfo> {
    let out = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let (Some(p), Some(b)) = (cur_path.take(), cur_branch.take()) {
                result.push(WorktreeInfo { path: p, branch: b });
            }
            cur_path = Some(PathBuf::from(rest));
            cur_branch = Some("detached".into());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "bare" {
            cur_branch = Some("(bare)".into());
        } else if line == "detached" {
            cur_branch = Some("detached".into());
        }
    }
    if let (Some(p), Some(b)) = (cur_path, cur_branch) {
        result.push(WorktreeInfo { path: p, branch: b });
    }
    result
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl ChangeStatus {
    pub fn glyph(self) -> &'static str {
        match self {
            ChangeStatus::Added => "A",
            ChangeStatus::Modified => "M",
            ChangeStatus::Deleted => "D",
            ChangeStatus::Renamed => "R",
            ChangeStatus::Untracked => "?",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub staged: bool,
    pub status: ChangeStatus,
}

#[derive(Clone, Default, Debug)]
pub struct GitStatus {
    pub branch: String,
    pub changes: Vec<FileChange>,
    pub added: usize,
    pub deleted: usize,
}

pub fn status(repo: &Path) -> Option<GitStatus> {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1", "--branch"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut branch = String::new();
    let mut changes = Vec::new();
    for line in stdout.lines() {
        if let Some(b) = line.strip_prefix("## ") {
            branch = b.split("...").next().unwrap_or("").trim().to_string();
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let path = line[3..].to_string();
        let map = |c: char| match c {
            'A' => ChangeStatus::Added,
            'M' => ChangeStatus::Modified,
            'D' => ChangeStatus::Deleted,
            'R' => ChangeStatus::Renamed,
            _ => ChangeStatus::Modified,
        };
        if x == '?' && y == '?' {
            changes.push(FileChange {
                path,
                staged: false,
                status: ChangeStatus::Untracked,
            });
            continue;
        }
        if x != ' ' && x != '?' {
            changes.push(FileChange {
                path: path.clone(),
                staged: true,
                status: map(x),
            });
        }
        if y != ' ' && y != '?' {
            changes.push(FileChange {
                path,
                staged: false,
                status: map(y),
            });
        }
    }

    let (added, deleted) = shortstat(repo).unwrap_or((0, 0));
    Some(GitStatus {
        branch,
        changes,
        added,
        deleted,
    })
}

fn shortstat(repo: &Path) -> Option<(usize, usize)> {
    let out = Command::new("git")
        .args(["diff", "--shortstat", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut added = 0usize;
    let mut deleted = 0usize;
    for part in s.split(',') {
        let part = part.trim();
        if let Some(num) = part.split_whitespace().next() {
            if let Ok(n) = num.parse::<usize>() {
                if part.contains("insertion") {
                    added = n;
                } else if part.contains("deletion") {
                    deleted = n;
                }
            }
        }
    }
    Some((added, deleted))
}

pub fn stage(repo: &Path, path: &str) -> Result<(), String> {
    run(repo, &["add", "--", path])
}

pub fn unstage(repo: &Path, path: &str) -> Result<(), String> {
    run(repo, &["restore", "--staged", "--", path])
}

pub fn commit(repo: &Path, message: &str) -> Result<(), String> {
    run(repo, &["commit", "-m", message])
}

pub fn push(repo: &Path) -> Result<(), String> {
    run(repo, &["push"])
}

pub fn pull(repo: &Path) -> Result<(), String> {
    run(repo, &["pull", "--ff-only"])
}

pub fn worktree_add(repo: &Path, path: &Path, branch: &str, create_new: bool) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    let mut args: Vec<&str> = vec!["worktree", "add"];
    if create_new {
        args.push("-b");
        args.push(branch);
        args.push(&path_str);
    } else {
        args.push(&path_str);
        args.push(branch);
    }
    run(repo, &args)
}

pub fn list_local_branches(repo: &Path) -> Vec<String> {
    let out = match Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn head_content(repo: &Path, path: &str) -> String {
    let out = match Command::new("git")
        .args(["show", &format!("HEAD:{path}")])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return String::new(),
    };
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn run(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

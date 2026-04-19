use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub path: PathBuf,
    pub branch: String,
}

pub fn list_workspaces(repo: &Path) -> Vec<WorkspaceInfo> {
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
                result.push(WorkspaceInfo { path: p, branch: b });
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
        result.push(WorkspaceInfo { path: p, branch: b });
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
        if let Some(num) = part.split_whitespace().next()
            && let Ok(n) = num.parse::<usize>() {
                if part.contains("insertion") {
                    added = n;
                } else if part.contains("deletion") {
                    deleted = n;
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
    // Run non-interactively: without these, an HTTPS remote that
    // wants credentials will block the background thread forever
    // waiting on a tty, with zero UI feedback.
    let out = Command::new("git")
        .args(["push"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

pub fn pull(repo: &Path) -> Result<(), String> {
    run(repo, &["pull", "--ff-only"])
}

pub fn workspace_add(repo: &Path, path: &Path, branch: &str, create_new: bool) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    // With -b, git requires `-b <new-branch> <path>` — no `--` between
    // them; `--` would be parsed as commit-ish. Only the non-`-b` form
    // (which takes `<path> <commit-ish>`) benefits from `--` to prevent
    // a leading-dash path being read as a flag.
    let mut args: Vec<&str> = vec!["worktree", "add"];
    if create_new {
        args.push("-b");
        args.push(branch);
        args.push(&path_str);
    } else {
        args.push("--");
        args.push(&path_str);
        args.push(branch);
    }
    run(repo, &args)
}

/// Walks up from `start` looking for the nearest `.git` directory or
/// gitfile. In a monorepo with nested repos / submodules this returns
/// the *innermost* repo containing the path — so features that bind to
/// "current repo" (branch picker, commit tree, status bar branch label)
/// track the file the user is actually looking at, not the outer
/// Workspace root.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let canon = start.canonicalize().ok()?;
    crate::util::find_ancestor(&canon, |dir| dir.join(".git").exists())
}

/// Discover all `.git` roots under `start`, capped by depth to avoid
/// walking into `node_modules` / `target` / huge submodule trees.
/// Always includes `start` itself if it's a repo.
pub fn discover_repos(start: &Path, max_depth: usize) -> Vec<PathBuf> {
    fn skip(name: &str) -> bool {
        matches!(
            name,
            "node_modules" | "target" | "dist" | "build" | ".next" | "vendor"
                | ".venv" | "venv" | ".cache" | ".turbo" | ".cargo"
        )
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(start.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let is_repo = dir.join(".git").exists();
        if is_repo {
            out.push(dir.clone());
        }
        if depth >= max_depth {
            continue;
        }
        // Recurse into every directory (including repos, so nested
        // submodules are found). `skip()` filters node_modules / target
        // / etc. below.
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let Some(n) = name.to_str() else { continue };
            if n.starts_with('.') && n != ".git" || skip(n) {
                continue;
            }
            stack.push((entry.path(), depth + 1));
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn current_branch(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo)
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    // Detached HEAD: surface the short hash so the status bar + picker
    // still show something clickable instead of silently disappearing.
    let short = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !short.status.success() {
        return None;
    }
    let h = String::from_utf8_lossy(&short.stdout).trim().to_string();
    if h.is_empty() {
        None
    } else {
        Some(format!("(detached {h})"))
    }
}

pub fn list_remote_branches(repo: &Path) -> Vec<String> {
    let out = match Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/",
        ])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.ends_with("/HEAD"))
        .collect()
}

/// Switch the working tree of `repo` to `branch` without creating a
/// new worktree. Fails (returns Err) on dirty tree or unknown branch —
/// git's own messages surface to the user.
pub fn checkout_branch(repo: &Path, branch: &str) -> Result<(), String> {
    // `git switch` is the unambiguous modern form — won't confuse a
    // dash-prefixed branch name with a flag the way `checkout` can.
    run(repo, &["switch", branch])
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

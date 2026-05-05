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
    /// For renames, this is the NEW path. The UI groups and sorts by
    /// this, so renamed files land in their destination folder instead
    /// of being split across both sides of the arrow.
    pub path: String,
    /// Source side of a rename, if any. Only set when `status` is
    /// `Renamed` — `path` holds the new name, `old_path` the old name.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    /// True if this file has staged changes in the index.
    pub has_staged: bool,
    /// True if this file has unstaged changes in the worktree.
    pub has_unstaged: bool,
    /// The staged-side status, if any (e.g. Added, Modified, Renamed).
    pub staged_status: Option<ChangeStatus>,
    /// The unstaged-side status, if any (e.g. Modified, Deleted).
    pub unstaged_status: Option<ChangeStatus>,
}

#[derive(Clone, Default, Debug)]
pub struct GitStatus {
    pub branch: String,
    pub changes: Vec<FileChange>,
    pub added: usize,
    pub deleted: usize,
    /// `Some` when an upstream is configured and `git rev-list` worked.
    /// `None` for fresh branches without `@{u}`. The Changes-pane
    /// toolbar uses this to decide whether to render the `↑N ↓N` pair.
    pub ahead_behind: Option<AheadBehind>,
}

pub fn status(repo: &Path) -> Option<GitStatus> {
    let out = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "--branch",
            // Without this, untracked directories collapse into a
            // single entry (e.g. `documents/`) instead of listing each
            // file — the Changes panel then shows one nameless `?` row
            // while the Files panel (which walks the FS) shows the
            // real contents. `all` makes git enumerate every untracked
            // path.
            "--untracked-files=all",
            // -z = NUL-separated records with paths left as-is. Without
            // it, porcelain v1 wraps any path containing spaces / non-
            // ASCII / shell-specials in double quotes and C-escapes
            // them. The Changes panel was then passing those quoted
            // strings straight to `git add` → fatal: pathspec '"..."'
            // did not match any files.
            "-z",
        ])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut branch = String::new();
    let mut changes = Vec::new();
    // With -z each record terminates in NUL. Rename entries emit two
    // consecutive records: the new path first, then the old path with
    // no status prefix. Iterate manually so we can peek the follow-up
    // record when we see an R/C status.
    let records: Vec<&str> = stdout.split('\0').filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    while i < records.len() {
        let line = records[i];
        i += 1;
        if let Some(b) = line.strip_prefix("## ") {
            branch = b.split("...").next().unwrap_or("").trim().to_string();
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let path_part = line[3..].to_string();
        // For R (rename) / C (copy) status the old path is in the next
        // NUL-separated record, not joined with " -> " like the non-z
        // format.
        let (path, old_path) = if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            let old = records.get(i).map(|s| s.to_string());
            if old.is_some() {
                i += 1;
            }
            (path_part, old)
        } else {
            (path_part, None)
        };
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
                old_path,
                status: ChangeStatus::Untracked,
                has_staged: false,
                has_unstaged: true,
                staged_status: None,
                unstaged_status: Some(ChangeStatus::Untracked),
            });
            continue;
        }
        // Build a single merged entry per file. X = staged side,
        // Y = unstaged side. Both can be set simultaneously (e.g. MM).
        let has_staged = x != ' ' && x != '?';
        let has_unstaged = y != ' ' && y != '?';
        let staged_status = if has_staged { Some(map(x)) } else { None };
        let unstaged_status = if has_unstaged { Some(map(y)) } else { None };
        // Pick a representative status for the row. Prefer staged side
        // so the status glyph reflects the most significant change.
        let status = staged_status.or(unstaged_status).unwrap_or(ChangeStatus::Modified);
        // old_path belongs to the Renamed status — always the staged (X)
        // side. If Y is also set (e.g. "RM"), the rename is staged and
        // the worktree modification is against the new path.
        changes.push(FileChange {
            path,
            old_path: if staged_status == Some(ChangeStatus::Renamed) { old_path } else { None },
            status,
            has_staged,
            has_unstaged,
            staged_status,
            unstaged_status,
        });
    }

    let (added, deleted) = shortstat(repo).unwrap_or((0, 0));
    let ahead_behind = ahead_behind(repo);
    Some(GitStatus {
        branch,
        changes,
        added,
        deleted,
        ahead_behind,
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

/// Stage a single hunk by piping a unified-diff patch through
/// `git apply --cached`. The patch must be a valid hunk fragment
/// including its `@@ ... @@` header and trailing context.
pub fn stage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    apply_hunk(repo, patch, false)
}

/// Unstage a single hunk by piping the patch through
/// `git apply --reverse --cached`.
pub fn unstage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    apply_hunk(repo, patch, true)
}

fn apply_hunk(repo: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    let args = if reverse {
        vec!["apply", "--reverse", "--cached", "-"]
    } else {
        vec!["apply", "--cached", "-"]
    };
    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(patch.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

/// Get the content of a file as it exists in the staging area (index).
/// Runs `git show :<path>`. Returns None if the path is not in the index.
pub fn staged_content(repo: &Path, rel_path: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["show", &format!(":{rel_path}")])
        .current_dir(repo)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Get the unified diff between HEAD and the working tree for a file.
/// Returns the raw diff text including hunk headers and context lines.
pub fn file_diff_raw(repo: &Path, rel_path: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["diff", "--", rel_path])
        .current_dir(repo)
        .output()
        .ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Get the unified diff between HEAD and the staging area
/// (i.e. staged changes only).
pub fn file_diff_staged(repo: &Path, rel_path: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["diff", "--cached", "--", rel_path])
        .current_dir(repo)
        .output()
        .ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Parse a unified diff into individual hunk patches. Each patch
/// includes the `diff --git` header, the hunk header `@@ ... @@`,
/// and the content lines. Returns (hunk_index, patch_text) pairs.
pub fn parse_hunks(diff: &str) -> Vec<(usize, String)> {
    let mut hunks = Vec::new();
    // Find the diff header line (first line starting with "diff --git")
    let header_end = diff.find('\n').unwrap_or(0);
    let header = if diff.starts_with("diff --git") {
        &diff[..header_end]
    } else {
        ""
    };
    // Find old/new mode lines or index lines between header and first hunk
    let mut first_hunk = 0;
    for (i, line) in diff.lines().enumerate() {
        if line.starts_with("@@") {
            first_hunk = i;
            break;
        }
    }
    let prefix: &str = if !header.is_empty() {
        // Include header + any index/mode lines before first hunk
        &diff[..diff
            .lines()
            .take(first_hunk)
            .map(|l| l.len() + 1)
            .sum::<usize>()
            .min(diff.len())]
    } else {
        ""
    };

    let lines: Vec<&str> = diff.lines().collect();
    let mut i = first_hunk;
    let mut hunk_idx = 0;
    while i < lines.len() {
        if lines[i].starts_with("@@") {
            // Collect this hunk's lines until next hunk or end
            let start = i;
            i += 1;
            while i < lines.len() && !lines[i].starts_with("@@") {
                i += 1;
            }
            let hunk_content: String = lines[start..i].join("\n");
            let patch = if prefix.is_empty() {
                hunk_content
            } else {
                format!("{}\n{}", prefix, hunk_content)
            };
            hunks.push((hunk_idx, patch));
            hunk_idx += 1;
        } else {
            i += 1;
        }
    }
    hunks
}

pub fn commit(repo: &Path, message: &str) -> Result<(), String> {
    run(repo, &["commit", "-m", message])
}

pub fn push(repo: &Path) -> Result<String, String> {
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
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        // git pushes the actual error to stderr; preserve it verbatim
        // so the user sees auth failures / network errors clearly.
        return Err(if stderr.is_empty() {
            "git push failed (no error output)".into()
        } else {
            stderr
        });
    }
    // git push reports progress to stderr even on success ("To
    // git@host…\n   abc..def  branch -> branch"). Look for the
    // ref-update line first; otherwise distinguish "Everything
    // up-to-date" from a generic success.
    let combined = if stderr.is_empty() { stdout } else { stderr };
    let summary = combined
        .lines()
        .rev()
        .find_map(|l| {
            let t = l.trim();
            if t.contains("Everything up-to-date") {
                Some(t.to_string())
            } else if t.starts_with("*") || t.contains("->") {
                Some(t.trim_start_matches('*').trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Pushed".to_string());
    Ok(summary)
}

pub fn pull(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    // First-line summary: typically "Already up to date." or
    // "Updating abc..def" (followed by the diff stat). Picking the
    // first non-empty line keeps the pill compact and meaningful.
    let summary = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Pulled".to_string());
    Ok(summary)
}

pub fn fetch(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .args(["fetch", "--prune"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    // git fetch prints to stderr; stdout is usually empty. Look for
    // the "<from> -> <to>" line; otherwise report "No new refs" so
    // the user knows nothing changed.
    let combined = if stderr.is_empty() { stdout } else { stderr };
    let updates: Vec<&str> = combined
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("->"))
        .collect();
    let summary = if updates.is_empty() {
        "No new refs".to_string()
    } else if updates.len() == 1 {
        updates[0].to_string()
    } else {
        format!("Fetched {} refs", updates.len())
    };
    Ok(summary)
}

/// Commits ahead/behind the upstream branch. `None` when no upstream
/// is configured (typical for fresh branches before first push). The
/// UI uses this to render "↑N ↓N" indicators next to the branch name
/// in the Changes-pane toolbar.
#[derive(Clone, Copy, Debug, Default)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

pub fn ahead_behind(repo: &Path) -> Option<AheadBehind> {
    let out = Command::new("git")
        .args([
            "rev-list",
            "--left-right",
            "--count",
            "@{u}...HEAD",
        ])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let behind = parts[0].parse::<usize>().ok()?;
    let ahead = parts[1].parse::<usize>().ok()?;
    Some(AheadBehind { ahead, behind })
}

/// Summary of what would be lost if a worktree were removed right now.
/// `unpushed_commits` counts commits on the current branch that aren't
/// on its upstream; `None` for upstream means no upstream is configured,
/// in which case every local commit on the branch is effectively
/// unpushed — we return `Some(n)` for "ahead of main" via `main..HEAD`
/// as a best-effort floor, or 0 if that comparison also fails.
/// `modified_files` counts anything `git status --porcelain` reports
/// (staged, unstaged, untracked).
#[derive(Clone, Debug, Default)]
pub struct WorktreeDirty {
    pub unpushed_commits: usize,
    pub modified_files: usize,
    pub has_upstream: bool,
}

pub fn worktree_dirty(worktree: &Path) -> WorktreeDirty {
    let modified_files = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(worktree)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0);

    let upstream = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(worktree)
        .output()
        .ok()
        .filter(|o| o.status.success());
    let has_upstream = upstream.is_some();

    let range = if has_upstream { "@{u}..HEAD" } else { "main..HEAD" };
    let unpushed_commits = Command::new("git")
        .args(["rev-list", "--count", range])
        .current_dir(worktree)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<usize>().ok())
        .unwrap_or(0);

    WorktreeDirty {
        unpushed_commits,
        modified_files,
        has_upstream,
    }
}

/// Remove a worktree via `git worktree remove --force <path>`. Force is
/// used because this is invoked from an explicit "Remove Worktree" UI
/// action — the user has already decided to discard local state, and
/// non-force would refuse on any uncommitted change and leave the
/// directory on disk, blocking future `worktree add` of the same branch
/// (which is the exact regression being fixed here).
pub fn workspace_remove(repo: &Path, path: &Path) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    run(repo, &["worktree", "remove", "--force", &path_str])
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
/// True when `path` is tracked by `repo` as a git submodule. Uses
/// `git submodule status --recursive` and matches against the absolute
/// submodule paths. Submodules share commits with the parent repo, so
/// treating them as separate Projects would show them twice in the UI.
pub fn is_submodule(repo: &Path, path: &Path) -> bool {
    let out = match Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["submodule", "status", "--recursive"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        // Format: " <sha> <path> (<describe>)"; leading char may be
        // '-' (uninit), '+' (different sha), 'U' (merge conflict).
        let rest = line.get(1..).unwrap_or("").trim_start();
        let mut parts = rest.splitn(3, ' ');
        let _sha = parts.next();
        let sub_path = match parts.next() {
            Some(p) => p,
            None => continue,
        };
        let abs = repo.join(sub_path);
        if abs == path || abs.canonicalize().ok() == path.canonicalize().ok() {
            return true;
        }
    }
    false
}

/// True when `path` is ignored by `repo`'s gitignore rules. Uses
/// `git check-ignore`, which exits 0 when ignored, 1 when not. Used to
/// decide whether a nested `.git` inside a parent repo should surface
/// as its own Sub-project: submodules stay hidden, but gitignored
/// siblings (e.g. cloned external repos dropped into a monorepo) get
/// promoted.
pub fn is_path_ignored(repo: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(repo).unwrap_or(path);
    let Some(rel_str) = rel.to_str() else {
        return false;
    };
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["check-ignore", "-q", "--"])
        .arg(rel_str)
        .output();
    matches!(out, Ok(o) if o.status.code() == Some(0))
}

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

/// Per-line diff classification for the editor gutter. Returned by
/// `parse_file_diff()` from `git diff HEAD -U0 -- <path>`.
#[derive(Clone, Debug, Default)]
pub struct FileDiff {
    /// 1-based line number → diff info for lines that exist in the working tree.
    pub lines: std::collections::HashMap<usize, DiffLine>,
    /// Deletion gaps: small red markers between lines where content was removed.
    /// `after_line` is 1-based — the gap sits between line `after_line` and
    /// `after_line + 1`.  0 means before line 1.
    pub deletions: Vec<DeletionGap>,
    /// Per-hunk modification blocks. A `-N +M` hunk produces one block that
    /// holds the full N old lines, regardless of whether N == M, N > M, or
    /// N < M. The gutter tooltip uses these so a `-20 +5` block can display
    /// all 20 deleted lines next to all 5 new lines.
    pub blocks: Vec<DiffBlock>,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Index into `FileDiff::blocks` for Modified lines. None for Added.
    pub block_idx: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DiffLineKind {
    Added,
    Modified,
}

#[derive(Clone, Debug)]
pub struct DeletionGap {
    /// 1-based: the gap sits between this line and the next. 0 = before line 1.
    pub after_line: usize,
    /// The original lines that were deleted (from HEAD), for tooltip display.
    pub head_lines: Vec<String>,
}

/// One `-N +M` modification hunk: full old content from HEAD plus the
/// 1-based line range in the working tree where the new lines live.
#[derive(Clone, Debug)]
pub struct DiffBlock {
    /// 1-based first line in the working tree.
    pub new_start: usize,
    /// Number of new lines (M in `-N +M`).
    pub new_count: usize,
    /// Full old content from HEAD (N lines in `-N +M`).
    pub old_lines: Vec<String>,
}

/// Parse `git diff HEAD -U0 -- <path>` into per-line change markers and
/// deletion gaps. Returns `None` if the file is untracked or unchanged.
pub fn parse_file_diff(repo: &Path, rel_path: &str) -> Option<FileDiff> {
    let tracked = Command::new("git")
        .args(["ls-files", "--error-unmatch", rel_path])
        .current_dir(repo)
        .output()
        .ok()
        .is_some_and(|o| o.status.success());

    if !tracked {
        return None;
    }

    let out = Command::new("git")
        .args(["diff", "HEAD", "-U0", "--", rel_path])
        .current_dir(repo)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return None;
    }

    let head = head_content(repo, rel_path);
    let head_lines: Vec<&str> = head.lines().collect();

    let mut diff = FileDiff::default();

    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("@@") else { continue };
        let end = rest.find("@@").unwrap_or(rest.len());
        let header = rest[..end].trim();

        let minus = header.split_whitespace().find(|s| s.starts_with('-'))?;
        let plus = header.split_whitespace().find(|s| s.starts_with('+'))?;

        let (old_start, old_count) = parse_range(&minus[1..])?;
        let (new_start, new_count) = parse_range(&plus[1..])?;

        if new_count > 0 && old_count == 0 {
            // Pure addition
            for i in 0..new_count {
                diff.lines.insert(new_start + i, DiffLine {
                    kind: DiffLineKind::Added,
                    block_idx: None,
                });
            }
        } else if new_count > 0 && old_count > 0 {
            // `-N +M` modification — capture all N old lines as one block.
            // Even when N >> M (e.g. -20 +5) the whole hunk is treated as a
            // change, not a deletion: every new line is BLUE and the tooltip
            // shows the full old vs new content.
            let old_block: Vec<String> = (0..old_count)
                .filter_map(|i| head_lines.get(old_start + i - 1).map(|s| s.to_string()))
                .collect();
            let block_idx = diff.blocks.len();
            diff.blocks.push(DiffBlock {
                new_start,
                new_count,
                old_lines: old_block,
            });
            for i in 0..new_count {
                diff.lines.insert(new_start + i, DiffLine {
                    kind: DiffLineKind::Modified,
                    block_idx: Some(block_idx),
                });
            }
        } else if new_count == 0 && old_count > 0 {
            // Pure deletion
            let deleted: Vec<String> = (0..old_count)
                .filter_map(|i| head_lines.get(old_start + i - 1).map(|s| s.to_string()))
                .collect();
            diff.deletions.push(DeletionGap {
                after_line: new_start.saturating_sub(1),
                head_lines: deleted,
            });
        }
    }

    if diff.lines.is_empty() && diff.deletions.is_empty() && diff.blocks.is_empty() {
        None
    } else {
        Some(diff)
    }
}

/// Parse a diff range like "3,2" → (3, 2) or "5" → (5, 1).
fn parse_range(s: &str) -> Option<(usize, usize)> {
    if let Some(comma) = s.find(',') {
        let start: usize = s[..comma].parse().ok()?;
        let count: usize = s[comma + 1..].parse().ok()?;
        Some((start, count))
    } else {
        let start: usize = s.parse().ok()?;
        Some((start, 1))
    }
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

/// `git show --name-status --format= <sha>` — returns a list of
/// (status_char, path) for files changed by the commit. Status is
/// one of A/M/D/R/C. Empty Vec on any error.
pub fn commit_files(repo: &Path, sha: &str) -> Vec<(char, PathBuf)> {
    let out = match Command::new("git")
        .args(["show", "--name-status", "--format=", sha])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let Some(status) = parts.next() else { continue };
        let Some(path) = parts.next() else { continue };
        let ch = status.chars().next().unwrap_or('?');
        result.push((ch, PathBuf::from(path)));
    }
    result
}

/// `git show <ref>:<path>` — content of `path` at the given ref.
/// Empty bytes on missing (e.g. for newly-added files queried at
/// the parent commit).
pub fn show_at(repo: &Path, reference: &str, path: &Path) -> Vec<u8> {
    let arg = format!("{reference}:{}", path.display());
    let out = match Command::new("git")
        .args(["show", &arg])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    out.stdout
}

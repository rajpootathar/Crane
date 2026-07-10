//! Minimal git shell-out for the Changes tab — `git status --porcelain` in a
//! worktree, parsed into status + path. Matches Crane's rule of shelling out
//! to the `git` binary (never libgit2).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use parking_lot::Mutex;

pub struct Change {
    /// Porcelain XY status, trimmed (e.g. "M", "A", "D", "??", "R").
    pub status: String,
    /// The RAW two-column porcelain XY code (e.g. "MM", "M ", " M", "??",
    /// "R "). `status` collapses this to one letter and `staged` to one
    /// bool, both of which lose the index-vs-worktree distinction; the
    /// shell needs the un-collapsed columns to render an `MM` file's
    /// tri-state (index modified AND worktree modified) correctly and to
    /// offer BOTH Stage (the worktree change) and Unstage (the index
    /// change) for it. X = column 0 (index/staged), Y = column 1 (worktree).
    pub xy: String,
    /// For renames/copies this is the NEW path. The shell groups and
    /// sorts by this so a renamed file lands in its destination folder.
    pub path: String,
    /// Source side of a rename/copy, if any. Only set when the record
    /// is an `R`/`C` — `path` holds the new name, `old_path` the old
    /// name — so the shell can show `old -> new` and stage correctly.
    pub old_path: Option<String>,
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

/// `git push` non-interactively (1:1 old Crane). Network op — call
/// off-thread (see [`spawn_git_op`]). Returns the real ref-update
/// summary parsed from git's stderr on success, git's stderr verbatim
/// on failure.
///
/// `stdin(Stdio::null())` + `GIT_TERMINAL_PROMPT=0` so an HTTPS remote
/// that wants credentials fails fast instead of blocking the background
/// thread forever on a tty / askpass prompt.
pub fn push(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["push"])
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
    // ref-update line; otherwise distinguish "Everything up-to-date"
    // from a generic success.
    let combined = if stderr.is_empty() { stdout } else { stderr };
    let summary = combined
        .lines()
        .rev()
        .find_map(|l| {
            let t = l.trim();
            if t.contains("Everything up-to-date") {
                Some(t.to_string())
            } else if t.starts_with('*') || t.contains("->") {
                Some(t.trim_start_matches('*').trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Pushed".to_string());
    Ok(summary)
}

/// Content of `rel_path` at HEAD as RAW BYTES (`git show HEAD:<path>`) — the
/// Diff Pane needs bytes to detect binary files before assuming UTF-8, and it
/// needs failure semantics richer than "empty string" (which the old
/// `head_content` collapsed everything to, silently diffing empty text):
///
/// - `Ok(Some(bytes))` — the file exists in HEAD.
/// - `Ok(None)` — the file is NOT in HEAD (untracked / newly added / unborn
///   HEAD on a fresh repo). The caller diffs against empty content, so every
///   line renders as an add — the legitimate outcome, not an error.
/// - `Err(msg)` — a real git failure (git missing, not a repository, corrupt
///   object store). The caller surfaces `msg` as an error row.
pub fn head_bytes(repo: &Path, rel_path: &str) -> Result<Option<Vec<u8>>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("HEAD:{rel_path}")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        return Ok(Some(out.stdout));
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // Expected "not in HEAD" shapes (per `git show` messages):
    //   "fatal: path 'x' does not exist in 'HEAD'"
    //   "fatal: path 'x' exists on disk, but not in 'HEAD'"
    //   "fatal: invalid object name 'HEAD'"   (unborn HEAD, fresh repo)
    let absent = err.contains("does not exist in")
        || err.contains("exists on disk, but not in")
        || err.contains("invalid object name");
    if absent { Ok(None) } else { Err(err) }
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

/// Sum of added/deleted lines across all uncommitted changes (`git diff --numstat HEAD`).
/// Returns `(added, deleted)`. Runs synchronously — call at reload/load time only, never
/// per frame. Binary files produce `-` in the numstat output which is silently skipped.
pub fn diff_numstat(root: &Path) -> (u32, u32) {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--numstat", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    else {
        return (0, 0);
    };
    if !out.status.success() {
        return (0, 0);
    }
    let mut added = 0u32;
    let mut deleted = 0u32;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut cols = line.splitn(3, '\t');
        added += cols.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        deleted += cols.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    }
    (added, deleted)
}

/// Whether the working tree at `root` has ANY uncommitted change (staged,
/// unstaged, or untracked) — `git status --porcelain` returning a non-empty
/// listing. Used to paint the "dirty dot" on a branch whose `diff --numstat`
/// totals are (0, 0) (e.g. only untracked files), mirroring old egui Crane.
pub fn is_dirty(root: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    else {
        return false;
    };
    out.status.success() && !out.stdout.is_empty()
}

/// Local branch names in `root` (`git branch --format=%(refname:short)`),
/// or empty on any error. Port of old Crane `git.rs::list_local_branches`.
pub fn list_local_branches(root: &Path) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["branch", "--format=%(refname:short)"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Remote-tracking branch names in `root` (`git branch -r
/// --format=%(refname:short)`), excluding symbolic HEAD refs. Empty on error.
pub fn list_remote_branches(root: &Path) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["branch", "-r", "--format=%(refname:short)"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains("->"))
        .map(str::to_string)
        .collect()
}

/// `git checkout <branch>` in `root`. Port of old Crane `git.rs::checkout_branch`.
pub fn checkout_branch(root: &Path, branch: &str) -> Result<(), String> {
    run(root, &["checkout", branch])
}

// ── Git Log commit ops (old git.rs, drained from the row context menu) ──
// One sync shell-out each, 1:1 with old Crane. All mutate the working
// tree / history, so the shell runs them off-thread and reloads the
// graph + Changes on completion (the .git watcher fires anyway).

/// `git checkout <sha>` — detached-HEAD checkout of a commit (context-menu
/// "Checkout this commit"). Port of old Crane `git.rs::checkout_commit`.
pub fn checkout_commit(repo: &Path, sha: &str) -> Result<(), String> {
    run(repo, &["checkout", sha])
}

/// `git branch <name> <sha>` — create a new branch at `sha` without
/// switching to it (context-menu "Create branch from here…"). Port of old
/// Crane `git.rs::branch_from`.
pub fn branch_from(repo: &Path, name: &str, sha: &str) -> Result<(), String> {
    run(repo, &["branch", name, sha])
}

/// `git cherry-pick <sha>` — apply the commit on top of HEAD (context-menu
/// "Cherry-pick onto current"). A conflict surfaces as git's stderr; the
/// user resolves it in a Terminal Pane like any other cherry-pick.
pub fn cherry_pick(repo: &Path, sha: &str) -> Result<(), String> {
    run(repo, &["cherry-pick", sha])
}

/// `git revert --no-edit <sha>` — create a revert commit on HEAD without
/// opening an editor (context-menu "Revert").
pub fn revert(repo: &Path, sha: &str) -> Result<(), String> {
    run(repo, &["revert", "--no-edit", sha])
}

/// `git show <ref>:<path>` — raw content of `path` at the given ref.
/// Empty bytes on missing (e.g. a newly-added file queried at the parent
/// commit). Port of old Crane `git.rs::show_at`.
pub fn show_at(repo: &Path, reference: &str, path: &Path) -> Vec<u8> {
    let arg = format!("{reference}:{}", path.display());
    let out = match Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &arg])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    out.stdout
}

/// `git init` in `dir` — turns a loose folder into a git repository.
/// On success the folder gains a `.git` directory; reload the project list
/// afterwards so the `is_loose` flag reflects the new state.
pub fn init(dir: &Path) -> Result<(), String> {
    run(dir, &["init"])
}

/// `git worktree remove --force <wt_path>` run from the main repo `main`.
/// `--force` so a worktree with uncommitted changes / a locked state is still
/// removed (matches old Crane's remove_worktree intent). Never removes the
/// primary working tree — the caller guards that case.
pub fn remove_worktree(main: &Path, wt_path: &Path) -> Result<(), String> {
    let p = wt_path.to_string_lossy();
    run(main, &["worktree", "remove", "--force", &p])
}

/// `git -C <main_repo> worktree add [-b <branch>] <path> <branch-or-startpoint>`.
/// Port of old Crane `git.rs::workspace_add`. When `create_branch` is true, uses
/// `-b <branch>` so git creates the branch at the current HEAD and checks it out
/// into the new worktree. When false, checks the *existing* `branch` out into the
/// new worktree (`worktree add -- <path> <branch>`). Returns git's stderr verbatim
/// on failure. Pure shell-out; never panics.
pub fn add_worktree(
    main_repo: &Path,
    branch: &str,
    path: &Path,
    create_branch: bool,
) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    // With `-b`, git wants `-b <new-branch> <path>` with no `--` separator
    // (a `--` there would be read as a commit-ish). The non-`-b` form takes
    // `<path> <commit-ish>` and benefits from `--` so a leading-dash path is
    // not mistaken for a flag.
    let mut args: Vec<&str> = vec!["worktree", "add"];
    if create_branch {
        args.push("-b");
        args.push(branch);
        args.push(&path_str);
    } else {
        args.push("--");
        args.push(&path_str);
        args.push(branch);
    }
    run(main_repo, &args)
}

/// Parse `git -C <main_repo> worktree list --porcelain` into
/// `(worktree_path, branch_name)` pairs. `branch_name` is the short ref from the
/// `branch refs/heads/<name>` line, or `"(detached)"` for a detached HEAD /
/// bare entry. Used for live worktree auto-detection (#3). Empty vec on any
/// error / non-repo; never panics.
pub fn list_worktrees(main_repo: &Path) -> Vec<(PathBuf, String)> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(main_repo)
        .args(["worktree", "list", "--porcelain"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result: Vec<(PathBuf, String)> = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    let flush =
        |result: &mut Vec<(PathBuf, String)>, p: Option<PathBuf>, b: Option<String>| {
            if let Some(path) = p {
                result.push((path, b.unwrap_or_else(|| "(detached)".to_string())));
            }
        };
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            // New record — flush the previous one first.
            flush(&mut result, cur_path.take(), cur_branch.take());
            cur_path = Some(PathBuf::from(rest));
            cur_branch = None;
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "detached" || line == "bare" {
            cur_branch = Some("(detached)".to_string());
        }
    }
    flush(&mut result, cur_path.take(), cur_branch.take());
    result
}

/// Discover every git repo at or under `start`, up to `max_depth` levels deep.
/// Iterative DFS that records any dir where `.git` exists and **recurses into
/// repos** (so nested submodules / cloned external repos are found). Skips the
/// usual build-output / dependency dirs (`node_modules`, `target`, …), all
/// hidden dirs (including `.git` itself — repo detection is `dir/.git` at the
/// parent, so there is never a reason to walk git internals), and symlinks.
/// `sort` + `dedup` for stable output. Always includes `start` itself when it
/// is a repo. 1:1 port of old Crane's `discover_repos`.
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
        // Recurse into every directory (including repos, so nested clones are
        // found). Skip hidden dirs (`.git` included — nothing under it is a
        // repo) and `skip()`'s node_modules / target / etc.
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let Some(n) = name.to_str() else { continue };
            if n.starts_with('.') || skip(n) {
                continue;
            }
            stack.push((entry.path(), depth + 1));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Absolute paths of every submodule `repo` tracks (recursively), normalized
/// via `canonicalize` where possible. Submodules share commits with the parent
/// repo, so grouping treats them as hidden (Crane's own `vendor/warp` must not
/// surface as a sibling). Computed ONCE per grouping pass so membership can be
/// tested per candidate without re-forking `git` for each one.
pub fn submodule_paths(repo: &Path) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    let out = match Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["submodule", "status", "--recursive"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return set,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        // Format: " <sha> <path> (<describe>)"; leading char may be
        // '-' (uninit), '+' (different sha), 'U' (merge conflict).
        let rest = line.get(1..).unwrap_or("").trim_start();
        let mut parts = rest.splitn(3, ' ');
        let _sha = parts.next();
        let Some(sub_path) = parts.next() else { continue };
        let abs = repo.join(sub_path);
        set.insert(abs.canonicalize().unwrap_or(abs));
    }
    set
}

/// Subset of `paths` ignored by `repo`'s gitignore rules, decided in a SINGLE
/// `git check-ignore` invocation (it accepts many pathnames and echoes back the
/// ignored ones) instead of one fork per path. Grouping promotes a gitignored,
/// non-submodule nested clone (an external repo dropped into a monorepo) to its
/// own Project. Returned paths are the exact `PathBuf`s from `paths`.
pub fn ignored_paths(repo: &Path, paths: &[PathBuf]) -> HashSet<PathBuf> {
    let mut result = HashSet::new();
    // Map the relative string handed to git back to the original abs path;
    // `check-ignore` echoes each ignored pathname verbatim as passed.
    let mut rel_to_abs: HashMap<String, PathBuf> = HashMap::new();
    let mut rels: Vec<String> = Vec::new();
    for p in paths {
        let rel = p.strip_prefix(repo).unwrap_or(p);
        if let Some(s) = rel.to_str() {
            rel_to_abs.insert(s.to_string(), p.clone());
            rels.push(s.to_string());
        }
    }
    if rels.is_empty() {
        return result;
    }
    let out = match Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "--"])
        .args(&rels)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        // Exit 0 = one or more ignored, 1 = none ignored; 128 = error (bad
        // repo / pathspec) — treat anything else as "nothing ignored".
        Ok(o) if matches!(o.status.code(), Some(0) | Some(1)) => o,
        _ => return result,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some(abs) = rel_to_abs.get(line) {
            result.insert(abs.clone());
        }
    }
    result
}

/// Create a branch `name` in `repo`. When `checkout` is true, runs
/// `git checkout -b <name>` (create + switch); otherwise `git branch <name>`
/// (create without switching). Returns git's stderr on failure. Pure
/// shell-out; never panics.
pub fn create_branch(repo: &Path, name: &str, checkout: bool) -> Result<(), String> {
    if checkout {
        run(repo, &["checkout", "-b", name])
    } else {
        run(repo, &["branch", name])
    }
}

/// Working-tree changes in `root`, or empty on any error / non-repo.
///
/// Runs `git status --porcelain -z --untracked-files=all`:
/// - `-z` emits NUL-separated records with paths left verbatim. Without
///   it, porcelain wraps any path with spaces / non-ASCII / shell-
///   specials in double quotes and C-escapes it — and the shell then
///   passes that quoted, escaped string straight to `git add -- <path>`,
///   which fails with `fatal: pathspec '"…"' did not match any files`.
/// - `--untracked-files=all` makes git enumerate every untracked file
///   instead of collapsing a brand-new directory into one nameless
///   `dir/` row.
///
/// With `-z`, a rename/copy (`R`/`C`) emits TWO consecutive records: the
/// new path first, then the old path with no status prefix. We peek the
/// follow-up record and keep it as `old_path`.
pub fn changes(root: &Path) -> Vec<Change> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Each record is NUL-terminated. Split and drop the trailing empty.
    let records: Vec<&str> = stdout.split('\0').filter(|s| !s.is_empty()).collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let l = records[i];
        i += 1;
        // Record layout: "XY <path>" — the two status columns, a space,
        // then the path (verbatim under -z). Index by char count so a
        // multi-byte first path char can't panic the slice.
        if l.chars().count() < 4 {
            continue;
        }
        let xy: String = l.chars().take(2).collect();
        let x = xy.chars().next().unwrap_or(' ');
        // Index column (X) set and not untracked → staged.
        let staged = x != ' ' && x != '?';
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
        let path: String = l.chars().skip(3).collect();
        // Rename/copy: the OLD path is the next NUL-separated record
        // (under -z it is a separate record, not " -> " joined).
        let is_rename = xy.contains('R') || xy.contains('C');
        let old_path = if is_rename {
            let old = records.get(i).map(|s| s.to_string());
            if old.is_some() {
                i += 1;
            }
            old
        } else {
            None
        };
        result.push(Change {
            status,
            xy: xy.clone(),
            path,
            old_path,
            staged,
        });
    }
    result
}

/// Working-tree changes as a flat `(path, staged, status_char)` tuple list,
/// sorted by path so the Right Panel can group them into a directory tree
/// trivially. Thin projection over [`changes`] — the shell groups by the
/// leading path components and paints the status glyph from `status_char`.
/// `status_char` is the single normalized porcelain letter ('M', 'A', 'D',
/// 'R', 'C', 'U', or '?'); ' ' when the status string is empty.
pub fn changes_flat(root: &Path) -> Vec<(String, bool, char)> {
    let mut rows: Vec<(String, bool, char)> = changes(root)
        .into_iter()
        .map(|c| {
            let ch = c.status.chars().next().unwrap_or(' ');
            (c.path, c.staged, ch)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// `git pull --ff-only` (1:1 old Crane `git.rs::pull`). Network op — call
/// off-thread (see [`spawn_git_op`]). Runs non-interactively so an HTTPS
/// remote that wants credentials fails fast instead of blocking on a tty.
/// Returns the first non-empty summary line ("Already up to date.",
/// "Updating abc..def", …) on success, git's stderr verbatim on failure.
pub fn pull(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["pull", "--ff-only"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    let summary = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Pulled".to_string());
    Ok(summary)
}

/// `git fetch --prune` (1:1 old Crane `git.rs::fetch`). Network op — call
/// off-thread. git fetch reports to stderr; we surface the single "<from>
/// -> <to>" ref-update line, "No new refs" when nothing changed, or a
/// "Fetched N refs" count when several updated.
pub fn fetch(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fetch", "--prune"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
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

/// `git fetch --all --prune --tags` — the Git Log dock's "Fetch all"
/// button, hitting every configured remote (1:1 the child command old
/// Crane's `refresh.rs::fetch_all_async` ran). Network op — BLOCKING; the
/// shell runs it off-thread (`ctx.spawn` / `std::thread`) with an
/// in-flight flag for the spinner, exactly like [`fetch`]. No result
/// plumbing is needed beyond the summary: the `.git/refs` watcher picks
/// up the fetched refs and triggers the graph reload on its own. Same
/// summary shape as [`fetch`] ("<from> -> <to>", "No new refs", or a
/// "Fetched N refs" count).
pub fn fetch_all(repo: &Path) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fetch", "--all", "--prune", "--tags"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
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

/// Commits `(ahead, behind)` the upstream branch, or `None` when no
/// upstream is configured (fresh branch before first push) or the
/// `git rev-list` call fails. Port of old Crane `git.rs::ahead_behind`,
/// returning a plain tuple instead of the `AheadBehind` struct.
///
/// `git rev-list --left-right --count @{u}...HEAD` prints "<behind>
/// <ahead>" — left side (`@{u}`) is commits we're missing (behind),
/// right side (`HEAD`) is commits we have that upstream lacks (ahead).
pub fn ahead_behind(repo: &Path) -> Option<(usize, usize)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--left-right", "--count", "@{u}...HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
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
    Some((ahead, behind))
}

// ── Per-file line diff (editor gutter markers) ────────────────────────
// Feeds the warp editor's gutter change-bar: which NEW-file lines were
// Added / Modified since HEAD, plus a Deleted boundary where lines were
// removed with nothing added in their place. Pure `git diff` shell-out —
// parses only the `@@ … @@` hunk headers, so it never has to diff text
// itself.

/// One gutter marker: a 1-based NEW-file line number paired with a change
/// kind encoded as a `char`:
///
/// - `'A'` — **Added**: the hunk added lines with no removals at that spot
///   (pure insertion). Every added line in the hunk gets its own `'A'`.
/// - `'M'` — **Modified**: the hunk both removed and added lines at that
///   spot (a replacement). Every added line in the hunk gets its own `'M'`.
/// - `'D'` — **Deleted**: the hunk removed lines with nothing added
///   (pure deletion). Emitted **once** per deletion, anchored to the
///   surviving NEW-file line the removal sits *after* (git's `+c` with
///   count 0); clamped to line 1 when the deletion is at the top of file.
///
/// The editor agent maps `'A' → DiffKind::Added`, `'M' → DiffKind::Modified`,
/// `'D' → DiffKind::Deleted` for `gutter_element`.
pub type LineDiff = (u32, char);

/// Per-file line diff for the editor gutter. Runs
/// `git diff --no-color --no-ext-diff -U0 -- <file>` in `repo` (unified
/// context of 0 so hunk headers hug the exact changed lines) and parses
/// the `@@ -a,b +c,d @@` headers into [`LineDiff`] markers over the NEW
/// file.
///
/// Header math (with `-U0`): `b` is the count of removed OLD lines, `d`
/// the count of added NEW lines starting at NEW line `c`. Then:
/// - `d > 0, b == 0` → lines `c .. c+d` are `'A'` (pure insertion).
/// - `d > 0, b > 0`  → lines `c .. c+d` are `'M'` (removal + insertion).
/// - `d == 0, b > 0` → one `'D'` at `c.max(1)` (pure deletion boundary).
///
/// Compares the working tree against the index+HEAD (unstaged *and* staged
/// edits both show, matching what the user sees in the buffer). Returns an
/// empty vec on any error, a non-repo path, or a clean file — never
/// panics, never errors out.
pub fn file_line_diff(repo: &Path, file: &Path) -> Vec<LineDiff> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--no-color", "--no-ext-diff", "-U0", "HEAD", "--"])
        .arg(file)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut marks: Vec<LineDiff> = Vec::new();
    for line in text.lines() {
        // Only hunk headers carry the line-number ranges we need.
        if !line.starts_with("@@") {
            continue;
        }
        let Some((old_count, new_start, new_count)) = parse_hunk_header(line) else {
            continue;
        };
        if new_count == 0 {
            // Pure deletion — anchor a single boundary at the surviving
            // NEW line the removal follows (clamp to 1 at top of file).
            if old_count > 0 {
                marks.push((new_start.max(1), 'D'));
            }
        } else {
            let kind = if old_count > 0 { 'M' } else { 'A' };
            for i in 0..new_count {
                marks.push((new_start + i, kind));
            }
        }
    }
    marks
}

/// Parse a `@@ -a,b +c,d @@` unified-diff hunk header into
/// `(old_count b, new_start c, new_count d)`. The `,b` / `,d` counts are
/// optional in the format and default to 1 when omitted. Returns `None`
/// on any malformed header.
fn parse_hunk_header(header: &str) -> Option<(u32, u32, u32)> {
    // header looks like: "@@ -a,b +c,d @@ optional section heading"
    let body = header.strip_prefix("@@ ")?;
    let ranges = body.split(" @@").next()?; // "-a,b +c,d"
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_count = count_of(old)?;
    let (new_start, new_count) = start_and_count(new)?;
    Some((old_count, new_start, new_count))
}

/// `"a,b"` → count `b`; bare `"a"` → count 1 (unified-diff default).
fn count_of(spec: &str) -> Option<u32> {
    match spec.split_once(',') {
        Some((_, c)) => c.parse().ok(),
        None => Some(1),
    }
}

/// `"c,d"` → `(c, d)`; bare `"c"` → `(c, 1)` (unified-diff default).
fn start_and_count(spec: &str) -> Option<(u32, u32)> {
    match spec.split_once(',') {
        Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
}

// ── Async git-op model ────────────────────────────────────────────────
// Mirrors old Crane's `GitOpKind` / `GitOpStatus` / `dispatch_git_op`
// (src/state/state.rs). Crane bans async runtimes — the op runs on a
// `std::thread`, flips a shared `OpStatus`, and calls `wake()` (egui
// `request_repaint`) so the Right Panel redraws with the result pill.

/// Which git operation a background thread is running.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Push,
    Pull,
    Fetch,
    Commit,
}

impl OpKind {
    /// Human label for the in-progress / result pill.
    pub fn label(self) -> &'static str {
        match self {
            OpKind::Push => "Push",
            OpKind::Pull => "Pull",
            OpKind::Fetch => "Fetch",
            OpKind::Commit => "Commit",
        }
    }
}

/// Lifecycle of the most recent git op. `Done`/`Failed` carry the
/// summary / error string for the result pill.
#[derive(Clone, Debug)]
pub enum OpState {
    Idle,
    Running,
    Done(String),
    Failed(String),
}

/// Shared status the Right Panel polls each frame. `kind` is `None`
/// while `Idle`; `Some(kind)` for the op that is Running / last
/// finished. Wrap in `Arc<Mutex<_>>` and hand a clone to
/// [`spawn_git_op`].
#[derive(Clone, Debug)]
pub struct OpStatus {
    pub kind: Option<OpKind>,
    pub state: OpState,
    /// The repo the current / last op ran against, so the shell can
    /// attribute a Running / Done / Failed pill to its project and stop
    /// project A's push error from bleeding into project B's UI. Empty
    /// while `Idle`; set to the op's `dir` by [`spawn_git_op`] /
    /// [`spawn_git_commit`] the moment they flip to `Running`.
    pub repo: PathBuf,
}

impl Default for OpStatus {
    fn default() -> Self {
        OpStatus {
            kind: None,
            state: OpState::Idle,
            repo: PathBuf::new(),
        }
    }
}

impl OpStatus {
    /// True while a background op is in flight — the shell disables the
    /// Push/Pull/Fetch buttons and shows a spinner to avoid double-dispatch.
    pub fn is_running(&self) -> bool {
        matches!(self.state, OpState::Running)
    }
}

/// Run a network git op (`Push` / `Pull` / `Fetch`) on a `std::thread`,
/// flipping `status` to `Running`, then `Done`/`Failed`, waking the UI
/// after each transition. Mirrors old Crane `dispatch_git_op`, including
/// the cheap `ahead_behind` short-circuit so Push/Pull answer instantly
/// when there's nothing to do instead of stalling on the network.
///
/// If an op is already `Running`, this is a no-op (guards against
/// double-dispatch). `Commit` needs a message and is rejected here —
/// use [`spawn_git_commit`] for that.
pub fn spawn_git_op(
    kind: OpKind,
    dir: PathBuf,
    status: Arc<Mutex<OpStatus>>,
    wake: impl Fn() + Send + 'static,
) {
    {
        let mut guard = status.lock();
        if guard.is_running() {
            return;
        }
        guard.kind = Some(kind);
        guard.repo = dir.clone();
        guard.state = OpState::Running;
    }

    // Cheap pre-checks (single `git rev-list`): tell the user upfront
    // when Push/Pull have nothing to do rather than after a network wait.
    if matches!(kind, OpKind::Push | OpKind::Pull) {
        if let Some((ahead, behind)) = ahead_behind(&dir) {
            let mut guard = status.lock();
            if kind == OpKind::Push && ahead == 0 {
                guard.state = OpState::Done(if behind > 0 {
                    format!("Nothing to push (behind {behind} — pull first)")
                } else {
                    "Nothing to push (up to date)".into()
                });
                drop(guard);
                wake();
                return;
            }
            if kind == OpKind::Pull && behind == 0 {
                guard.state = OpState::Done("Already up to date".into());
                drop(guard);
                wake();
                return;
            }
        }
    }

    std::thread::spawn(move || {
        let result: Result<String, String> = match kind {
            OpKind::Push => push(&dir),
            OpKind::Pull => pull(&dir),
            OpKind::Fetch => fetch(&dir),
            OpKind::Commit => Err("Commit requires a message — use spawn_git_commit".into()),
        };
        let mut guard = status.lock();
        guard.state = match result {
            Ok(message) => OpState::Done(message),
            Err(error) => OpState::Failed(error),
        };
        drop(guard);
        wake();
    });
}

/// Async commit on a `std::thread` — the message-carrying companion to
/// [`spawn_git_op`] (whose fixed signature can't take a message). Flips
/// `status` to `Running` → `Done`/`Failed`, waking the UI. Refuses an
/// empty / whitespace message so the pill reports a clear error instead
/// of git's "aborting commit due to empty message".
pub fn spawn_git_commit(
    dir: PathBuf,
    message: String,
    status: Arc<Mutex<OpStatus>>,
    wake: impl Fn() + Send + 'static,
) {
    {
        let mut guard = status.lock();
        if guard.is_running() {
            return;
        }
        guard.kind = Some(OpKind::Commit);
        guard.repo = dir.clone();
        guard.state = OpState::Running;
    }
    std::thread::spawn(move || {
        let result = if message.trim().is_empty() {
            Err("No commit message".to_string())
        } else {
            commit(&dir, &message).map(|()| "Committed".to_string())
        };
        let mut guard = status.lock();
        guard.state = match result {
            Ok(message) => OpState::Done(message),
            Err(error) => OpState::Failed(error),
        };
        drop(guard);
        wake();
    });
}

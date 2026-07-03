//! Minimal git shell-out for the Changes tab — `git status --porcelain` in a
//! worktree, parsed into status + path. Matches Crane's rule of shelling out
//! to the `git` binary (never libgit2).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use parking_lot::Mutex;

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

/// `git init` in `dir` — turns a loose folder into a git repository.
/// On success the folder gains a `.git` directory; reload the project list
/// afterwards so the `is_loose` flag reflects the new state.
pub fn init(dir: &Path) -> Result<(), String> {
    run(dir, &["init"])
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
}

impl Default for OpStatus {
    fn default() -> Self {
        OpStatus {
            kind: None,
            state: OpState::Idle,
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
            OpKind::Push => push(&dir).map(|()| "Pushed".to_string()),
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

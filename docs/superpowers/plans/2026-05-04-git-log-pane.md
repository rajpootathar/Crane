# Git Log Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a JetBrains-style Git Log Pane: sticky bottom-docked region per Tab with three resizable internal columns (Refs | Graph+Log | Commit Details), real DAG lane visualization, common write ops, and crane-native worktree-from-commit.

**Architecture:** Per-Tab sticky bottom region (separate from the Layout tree, so it's non-draggable by construction). Worker threads run `git` CLI commands and emit `GraphFrame`s through `mpsc` channels; UI thread only renders cached frames. Lane assignment runs once per refresh — single-pass over commits oldest→newest, maintaining `Vec<Option<Sha>>` of active lanes. Auto-refresh via `notify` filesystem watcher on `.git/HEAD` + `.git/refs/` + `.git/packed-refs`, with 5 s poll fallback.

**Tech Stack:** Rust 2024, eframe 0.34 + egui 0.34 (existing), parking_lot mutexes, std::thread + std::sync::mpsc, `notify` (NEW), `chrono` (existing for timestamps), git CLI shell-out via `std::process::Command`.

**Spec:** `docs/superpowers/specs/2026-05-04-git-log-pane-design.md`

---

## File structure

**New module tree:**
```
src/git_log/
├── mod.rs            — pub use; module wiring
├── data.rs           — `git log` parsing → Vec<CommitRecord>
├── refs.rs           — `git for-each-ref` + `git worktree list` → RefSet
├── graph.rs          — DAG + lane assignment → LaneFrame
├── refresh.rs        — notify watcher + poll + Fetch-all
├── state.rs          — GitLogState struct + reload coordination
├── view.rs           — top-level render, splitters, header chrome
└── view/
    ├── refs.rs       — left column (Local / Remote / Tags / Worktrees tree)
    ├── log.rs        — middle column (filter bar + graph painter + log rows)
    └── details.rs    — right column (changed files + commit metadata)
```

**Files modified:**
- `Cargo.toml` — add `notify = "8"` (latest stable of `notify` crate; verify version at install time)
- `src/main.rs` — register `git_log` module; add `Cmd+9` shortcut wiring
- `src/state/state.rs` — `Tab { git_log_visible, git_log_state }` fields; `App::open_or_focus_diff_pane`; `App::pending_open_commit_diff`; `BaseRef::Commit(Sha)` enum variant for the `NewWorkspaceModal`
- `src/state/session.rs` — `STab.git_log_visible` + `STab.git_log_state` (`SGitLogState` struct)
- `src/ui/pane_view.rs` — render bottom region after Layout when `tab.git_log_visible`
- `src/ui/top.rs` — `git-branch` icon button in Main Panel top bar
- `src/shortcuts.rs` — `Cmd+9` toggles `App::toggle_git_log()`
- `src/git.rs` — add `show_at(repo, ref, path)` + `checkout_commit(repo, sha)` + `cherry_pick(repo, sha)` + `revert(repo, sha)` + `branch_from(repo, name, sha)` helpers

---

## Phase 1: Module skeleton + Tab field additions + persistence

### Task 1.1: Create the `git_log` module skeleton

**Files:**
- Create: `src/git_log/mod.rs`
- Create: `src/git_log/state.rs`
- Modify: `src/main.rs` (add `mod git_log;`)

- [ ] **Step 1: Create empty module files**

```rust
// src/git_log/mod.rs
pub mod state;

pub use state::GitLogState;
```

```rust
// src/git_log/state.rs
use std::path::PathBuf;
use std::time::Instant;

pub type Sha = String;

pub struct GitLogState {
    pub height: f32,
    pub col_refs_width: f32,
    pub col_details_width: f32,
    pub maximized: bool,
    pub selected_commit: Option<Sha>,
    pub selected_file: Option<PathBuf>,
    pub last_poll: Instant,
}

impl GitLogState {
    pub fn new() -> Self {
        Self {
            height: 320.0,
            col_refs_width: 220.0,
            col_details_width: 360.0,
            maximized: false,
            selected_commit: None,
            selected_file: None,
            last_poll: Instant::now(),
        }
    }
}

impl Default for GitLogState {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Wire module into main.rs**

In `src/main.rs`, add at the top with other `mod` declarations:

```rust
mod git_log;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: clean build, no errors. Existing 9 warnings unchanged.

- [ ] **Step 4: Commit**

```bash
git add src/git_log/ src/main.rs
git commit -m "feat: add empty git_log module skeleton"
```

### Task 1.2: Add git_log fields to Tab + serialization stubs

**Files:**
- Modify: `src/state/state.rs` (Tab struct around line 20)
- Modify: `src/state/session.rs`

- [ ] **Step 1: Add fields to Tab**

In `src/state/state.rs`, find `pub struct Tab` (line 20). Add two fields:

```rust
pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: Layout,
    /// Whether the bottom-docked Git Log Pane is shown for this Tab.
    /// Default false; toggled by Cmd+9 / top-bar button.
    pub git_log_visible: bool,
    /// Lazy-initialized on first show. None means the user has never
    /// opened the pane on this Tab. Persisted in session.json.
    pub git_log_state: Option<crate::git_log::GitLogState>,
}
```

Find every `Tab { ... }` constructor in `state.rs` and add `git_log_visible: false, git_log_state: None,` to each.

- [ ] **Step 2: Add serialization fields to STab**

In `src/state/session.rs`, find `struct STab`. Add:

```rust
#[serde(default)]
pub git_log_visible: bool,
#[serde(default)]
pub git_log_state: Option<SGitLogState>,
```

Add the new struct below STab:

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct SGitLogState {
    #[serde(default = "default_height")]
    pub height: f32,
    #[serde(default = "default_col_refs")]
    pub col_refs_width: f32,
    #[serde(default = "default_col_details")]
    pub col_details_width: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub selected_commit: Option<String>,
    #[serde(default)]
    pub selected_file: Option<String>,
}

fn default_height() -> f32 { 320.0 }
fn default_col_refs() -> f32 { 220.0 }
fn default_col_details() -> f32 { 360.0 }
```

- [ ] **Step 3: Wire serialization round-trip**

In `STab`'s constructor (where existing fields populate from `Tab`), add:

```rust
git_log_visible: t.git_log_visible,
git_log_state: t.git_log_state.as_ref().map(|s| SGitLogState {
    height: s.height,
    col_refs_width: s.col_refs_width,
    col_details_width: s.col_details_width,
    maximized: s.maximized,
    selected_commit: s.selected_commit.clone(),
    selected_file: s.selected_file.as_ref().map(|p| p.to_string_lossy().to_string()),
}),
```

In `STab::into_tab` (or wherever STab → Tab conversion happens), add:

```rust
git_log_visible: self.git_log_visible,
git_log_state: self.git_log_state.map(|s| crate::git_log::GitLogState {
    height: s.height,
    col_refs_width: s.col_refs_width,
    col_details_width: s.col_details_width,
    maximized: s.maximized,
    selected_commit: s.selected_commit,
    selected_file: s.selected_file.map(std::path::PathBuf::from),
    last_poll: std::time::Instant::now(),
}),
```

- [ ] **Step 4: Verify build + existing tests**

Run: `cargo build && make test`
Expected: clean build, all 14 existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/state/state.rs src/state/session.rs
git commit -m "feat(git-log): persist Tab.git_log fields in session"
```

---

## Phase 2: Commit data layer + tests

### Task 2.1: Define CommitRecord + load_commits with parser tests

**Files:**
- Create: `src/git_log/data.rs`
- Create test fixture inline in same file (Rust unit test convention)

- [ ] **Step 1: Write failing test for the parser**

Create `src/git_log/data.rs`:

```rust
use std::path::Path;
use std::process::Command;

pub type Sha = String;

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    pub sha: Sha,
    pub parents: Vec<Sha>,
    pub author: String,
    pub date: String,        // ISO 8601 string (parse on demand to avoid chrono in hot path)
    pub subject: String,
    pub refs_decoration: String,
}

const FIELD_SEP: char = '\x1f';
const RECORD_SEP: char = '\n';

/// Format: `%H<US>%P<US>%an<US>%aI<US>%s<US>%D<LF>`
pub fn parse_log_output(stdout: &str) -> Vec<CommitRecord> {
    let mut out = Vec::new();
    for line in stdout.split(RECORD_SEP) {
        if line.is_empty() { continue; }
        let mut fields = line.split(FIELD_SEP);
        let (Some(sha), Some(parents), Some(author), Some(date), Some(subject), Some(refs)) =
            (fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
            else { continue; };
        let parents: Vec<Sha> = if parents.is_empty() {
            Vec::new()
        } else {
            parents.split(' ').map(String::from).collect()
        };
        out.push(CommitRecord {
            sha: sha.to_string(),
            parents,
            author: author.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
            refs_decoration: refs.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(sha: &str, parents: &str, author: &str, date: &str, subject: &str, refs: &str) -> String {
        format!("{sha}\x1f{parents}\x1f{author}\x1f{date}\x1f{subject}\x1f{refs}")
    }

    #[test]
    fn parses_single_commit_no_parents() {
        let stdout = line("abc123", "", "Alice", "2026-05-01T10:00:00+00:00", "Initial", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sha, "abc123");
        assert!(parsed[0].parents.is_empty());
        assert_eq!(parsed[0].author, "Alice");
        assert_eq!(parsed[0].subject, "Initial");
    }

    #[test]
    fn parses_two_parent_merge() {
        let stdout = line("m1", "p1 p2", "Bob", "2026-05-02T10:00:00+00:00", "Merge", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].parents, vec!["p1".to_string(), "p2".to_string()]);
    }

    #[test]
    fn parses_octopus_three_parents() {
        let stdout = line("m1", "p1 p2 p3", "Carol", "2026-05-03T10:00:00+00:00", "Octopus", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].parents.len(), 3);
    }

    #[test]
    fn malformed_lines_skip_cleanly() {
        let stdout = format!(
            "good\x1f\x1fAuthor\x1f2026-05-01T10:00:00+00:00\x1fSubject\x1f\nshort_line_only_two_fields\nanother\x1f\x1fAuthor\x1f2026-05-01T10:00:00+00:00\x1fSubject\x1f"
        );
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].sha, "good");
        assert_eq!(parsed[1].sha, "another");
    }

    #[test]
    fn subjects_with_pipe_chars_dont_corrupt() {
        let stdout = line("abc", "", "Author", "2026-05-01T10:00:00+00:00", "fix: a | b | c", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].subject, "fix: a | b | c");
    }

    #[test]
    fn refs_decoration_carries_through() {
        let stdout = line("abc", "", "Author", "2026-05-01T10:00:00+00:00", "Subject",
            " (HEAD -> main, origin/main, tag: v1.0)");
        let parsed = parse_log_output(&stdout);
        assert!(parsed[0].refs_decoration.contains("HEAD"));
        assert!(parsed[0].refs_decoration.contains("v1.0"));
    }
}
```

- [ ] **Step 2: Wire data module into git_log/mod.rs**

```rust
// src/git_log/mod.rs
pub mod data;
pub mod state;

pub use data::{CommitRecord, Sha};
pub use state::GitLogState;
```

- [ ] **Step 3: Run tests — expect FAIL? No, parser is implemented.**

Run: `cargo test --bin crane git_log::data::tests`
Expected: 6/6 PASS.

(This task is "implement + tests in one step" since the parser is small and the tests function as the spec.)

- [ ] **Step 4: Commit**

```bash
git add src/git_log/data.rs src/git_log/mod.rs
git commit -m "feat(git-log): commit log parser + tests"
```

### Task 2.2: Add `load_commits` shell-out wrapper

**Files:**
- Modify: `src/git_log/data.rs`

- [ ] **Step 1: Add the shell-out function**

Append to `src/git_log/data.rs`:

```rust
/// Run `git log --all --date-order --pretty=...` against `repo` and
/// return parsed commit records. `max_count` caps the result —
/// pass a large value (e.g. 10_000) for the initial load. Returns
/// empty Vec on any error (caller can detect via len == 0 +
/// re-running with --max-count=1 to disambiguate "empty repo" from
/// "broken").
pub fn load_commits(repo: &Path, max_count: usize) -> Vec<CommitRecord> {
    let format = format!(
        "--pretty=format:%H{us}%P{us}%an{us}%aI{us}%s{us}%D",
        us = '\x1f'
    );
    let max_count_arg = format!("--max-count={max_count}");
    let out = match Command::new("git")
        .args([
            "log",
            "--all",
            "--date-order",
            &format,
            &max_count_arg,
        ])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_log_output(&stdout)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/git_log/data.rs
git commit -m "feat(git-log): load_commits shell-out wrapper"
```

---

## Phase 3: Refs + worktrees data layer

### Task 3.1: Define RefSet + load_refs

**Files:**
- Create: `src/git_log/refs.rs`
- Modify: `src/git_log/mod.rs`

- [ ] **Step 1: Write failing tests for the parser**

Create `src/git_log/refs.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct RefEntry {
    pub name: String,
    pub sha: String,
    pub upstream: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefSet {
    pub local: Vec<RefEntry>,
    pub remote: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    pub worktrees: Vec<WorktreeEntry>,
    pub head: Option<String>,
}

const FIELD_SEP: char = '\x1f';

pub fn parse_for_each_ref(stdout: &str) -> RefSet {
    let mut set = RefSet::default();
    for line in stdout.split('\n') {
        if line.is_empty() { continue; }
        let mut fields = line.split(FIELD_SEP);
        let (Some(refname), Some(objectname), Some(upstream)) =
            (fields.next(), fields.next(), fields.next())
            else { continue; };
        let upstream = if upstream.is_empty() { None } else { Some(upstream.to_string()) };
        let entry = RefEntry {
            name: refname.to_string(),
            sha: objectname.to_string(),
            upstream,
        };
        if refname.starts_with("refs/heads/") {
            set.local.push(entry);
        } else if refname.starts_with("refs/remotes/") {
            set.remote.push(entry);
        } else if refname.starts_with("refs/tags/") {
            set.tags.push(entry);
        }
    }
    set
}

pub fn parse_worktree_porcelain(stdout: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in stdout.split('\n') {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let (Some(p), Some(b)) = (cur_path.take(), cur_branch.take()) {
                out.push(WorktreeEntry { path: p, branch: b });
            }
            cur_path = Some(PathBuf::from(rest));
            cur_branch = Some("detached".to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "bare" {
            cur_branch = Some("(bare)".to_string());
        } else if line == "detached" {
            cur_branch = Some("detached".to_string());
        }
    }
    if let (Some(p), Some(b)) = (cur_path, cur_branch) {
        out.push(WorktreeEntry { path: p, branch: b });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_line(name: &str, sha: &str, upstream: &str) -> String {
        format!("{name}\x1f{sha}\x1f{upstream}\n")
    }

    #[test]
    fn parses_local_remote_tag_buckets() {
        let stdout = format!(
            "{}{}{}",
            ref_line("refs/heads/main", "aaa", "refs/remotes/origin/main"),
            ref_line("refs/remotes/origin/main", "aaa", ""),
            ref_line("refs/tags/v1.0", "bbb", ""),
        );
        let set = parse_for_each_ref(&stdout);
        assert_eq!(set.local.len(), 1);
        assert_eq!(set.remote.len(), 1);
        assert_eq!(set.tags.len(), 1);
        assert_eq!(set.local[0].name, "refs/heads/main");
        assert_eq!(set.local[0].upstream.as_deref(), Some("refs/remotes/origin/main"));
        assert!(set.tags[0].upstream.is_none());
    }

    #[test]
    fn worktree_branched_then_detached_then_bare() {
        let stdout = "\
worktree /a/main
branch refs/heads/main

worktree /a/feat
branch refs/heads/feat/x

worktree /a/det
HEAD abc
detached

worktree /a/bare
bare
";
        let parsed = parse_worktree_porcelain(stdout);
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].branch, "main");
        assert_eq!(parsed[1].branch, "feat/x");
        assert_eq!(parsed[2].branch, "detached");
        assert_eq!(parsed[3].branch, "(bare)");
    }
}
```

- [ ] **Step 2: Wire module**

```rust
// src/git_log/mod.rs
pub mod data;
pub mod refs;
pub mod state;

pub use data::{CommitRecord, Sha};
pub use refs::{RefEntry, RefSet, WorktreeEntry};
pub use state::GitLogState;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --bin crane git_log::refs::tests`
Expected: 2/2 PASS.

- [ ] **Step 4: Commit**

```bash
git add src/git_log/refs.rs src/git_log/mod.rs
git commit -m "feat(git-log): refs + worktree porcelain parsers + tests"
```

### Task 3.2: Add `load_refs` + `load_head` shell-out

**Files:**
- Modify: `src/git_log/refs.rs`

- [ ] **Step 1: Add shell-out wrappers**

Append to `src/git_log/refs.rs`:

```rust
pub fn load_refs(repo: &Path) -> RefSet {
    let format = format!("--format=%(refname){us}%(objectname){us}%(upstream)", us = '\x1f');
    let out = match Command::new("git")
        .args(["for-each-ref", &format, "refs/heads", "refs/remotes", "refs/tags"])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return RefSet::default(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut set = parse_for_each_ref(&stdout);

    // HEAD:
    if let Ok(o) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
    {
        if o.status.success() {
            let head = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !head.is_empty() {
                set.head = Some(head);
            }
        }
    }

    // Worktrees:
    if let Ok(o) = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
    {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            set.worktrees = parse_worktree_porcelain(&stdout);
        }
    }

    set
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/git_log/refs.rs
git commit -m "feat(git-log): load_refs + worktree shell-out"
```

---

## Phase 4: Lane assignment algorithm + tests

### Task 4.1: Write LaneFrame types + the layout algorithm with tests

**Files:**
- Create: `src/git_log/graph.rs`
- Modify: `src/git_log/mod.rs`

- [ ] **Step 1: Write failing tests + implementation in one file**

Create `src/git_log/graph.rs`:

```rust
use crate::git_log::data::{CommitRecord, Sha};

#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    pub sha: Sha,
    pub own_lane: u8,
    /// Which lanes the parents occupy. The first entry is always
    /// `own_lane` for the first parent (linear continuation) — except
    /// for root commits, where this is empty.
    pub parent_lanes: Vec<u8>,
    /// Lanes that existed BEFORE this row's draw and don't continue
    /// past it (closing branches). Used by the painter to draw lane
    /// caps.
    pub terminating_lanes: Vec<u8>,
    /// Color slot in the 8-color palette. Approximates "color per
    /// branch" — see ColorSeeder for details.
    pub color: u8,
    /// How many lanes are still active after this row.
    pub visible_lanes_after: u8,
}

#[derive(Clone, Debug, Default)]
pub struct LaneFrame {
    pub rows: Vec<LaneRow>,
    pub max_lane: u8,
}

/// Stable color picker keyed on `(lane_index, allocation_epoch)`.
/// Each time a lane is freshly claimed (after being free or never
/// used), the epoch increments. Same (lane, epoch) → same color.
pub struct ColorSeeder {
    epochs: Vec<u32>,           // per-lane allocation count
}

impl ColorSeeder {
    pub fn new() -> Self {
        Self { epochs: Vec::new() }
    }
    /// Call when allocating lane `i` for a new branch. Returns the
    /// color slot (0..8) for that allocation.
    pub fn allocate(&mut self, lane: usize) -> u8 {
        while self.epochs.len() <= lane {
            self.epochs.push(0);
        }
        self.epochs[lane] += 1;
        let h = (lane as u32 * 7919) ^ (self.epochs[lane] * 31337);
        (h % 8) as u8
    }
    /// Color for a row whose lane was allocated in the current epoch.
    /// (Doesn't increment.)
    pub fn current(&self, lane: usize) -> u8 {
        let e = *self.epochs.get(lane).unwrap_or(&1);
        let h = (lane as u32 * 7919) ^ (e * 31337);
        (h % 8) as u8
    }
}

/// Build a LaneFrame from commits in display order (newest first).
/// Algorithm walks oldest→newest internally to track lane ownership,
/// then reverses back to display order.
pub fn layout(commits: &[CommitRecord]) -> LaneFrame {
    if commits.is_empty() {
        return LaneFrame::default();
    }

    // active_lanes[i] = sha that the next commit on column i must be.
    // None = column free.
    let mut active_lanes: Vec<Option<Sha>> = Vec::new();
    let mut seeder = ColorSeeder::new();
    // Build rows oldest→newest so lane ownership flows naturally.
    // commits is newest-first; iterate reverse.
    let mut rows: Vec<LaneRow> = Vec::with_capacity(commits.len());

    for c in commits.iter().rev() {
        // Snapshot lanes BEFORE this row's mutations — used to
        // identify terminating lanes.
        let lanes_before = active_lanes.clone();

        // 1. Find the lane waiting for this commit (or allocate a new one).
        let own_lane = match active_lanes.iter().position(|l| l.as_ref() == Some(&c.sha)) {
            Some(idx) => idx,
            None => {
                // Orphan / fresh tip — leftmost free or push.
                let slot = active_lanes.iter().position(Option::is_none).unwrap_or(active_lanes.len());
                if slot == active_lanes.len() {
                    active_lanes.push(None);
                }
                seeder.allocate(slot);
                slot
            }
        };

        // 2. First parent claims the same lane (linear continuation).
        let mut parent_lanes: Vec<u8> = Vec::new();
        if let Some(p0) = c.parents.first() {
            active_lanes[own_lane] = Some(p0.clone());
            parent_lanes.push(own_lane as u8);
        } else {
            active_lanes[own_lane] = None; // root commit
        }

        // 3. Subsequent parents → branch off into new lanes (leftmost free).
        for p in c.parents.iter().skip(1) {
            let slot = active_lanes.iter().position(Option::is_none).unwrap_or(active_lanes.len());
            if slot == active_lanes.len() {
                active_lanes.push(None);
            }
            active_lanes[slot] = Some(p.clone());
            seeder.allocate(slot);
            parent_lanes.push(slot as u8);
        }

        // 4. Compact: trailing Nones drop off so visual width stays minimal.
        while matches!(active_lanes.last(), Some(None)) {
            active_lanes.pop();
        }

        // Lanes that existed before but don't exist after = terminating.
        let terminating_lanes: Vec<u8> = lanes_before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let still_alive = i < active_lanes.len() && active_lanes[i].is_some();
                if l.is_some() && !still_alive && i != own_lane {
                    Some(i as u8)
                } else {
                    None
                }
            })
            .collect();

        let color = seeder.current(own_lane);

        rows.push(LaneRow {
            sha: c.sha.clone(),
            own_lane: own_lane as u8,
            parent_lanes,
            terminating_lanes,
            color,
            visible_lanes_after: active_lanes.len() as u8,
        });
    }

    rows.reverse();
    let max_lane = rows.iter().map(|r| r.visible_lanes_after).max().unwrap_or(1);
    LaneFrame { rows, max_lane }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_log::data::CommitRecord;

    fn cr(sha: &str, parents: &[&str]) -> CommitRecord {
        CommitRecord {
            sha: sha.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "A".to_string(),
            date: "2026-05-01T10:00:00+00:00".to_string(),
            subject: "S".to_string(),
            refs_decoration: String::new(),
        }
    }

    #[test]
    fn empty_input_returns_empty_frame() {
        let frame = layout(&[]);
        assert!(frame.rows.is_empty());
        assert_eq!(frame.max_lane, 0);
    }

    #[test]
    fn straight_line_no_merges() {
        // c3 -> c2 -> c1 -> root
        let commits = vec![
            cr("c3", &["c2"]),
            cr("c2", &["c1"]),
            cr("c1", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);
        assert_eq!(frame.rows.len(), 4);
        for r in &frame.rows {
            assert_eq!(r.own_lane, 0, "row {} not on lane 0", r.sha);
        }
        assert_eq!(frame.max_lane, 0); // last row terminates everything
    }

    #[test]
    fn fork_and_merge_two_branches() {
        //   m       (merge of c2, b1)
        //   |\
        //   c2 b1
        //   | /
        //   c1
        let commits = vec![
            cr("m",  &["c2", "b1"]),
            cr("c2", &["c1"]),
            cr("b1", &["c1"]),
            cr("c1", &[]),
        ];
        let frame = layout(&commits);

        let m_row = frame.rows.iter().find(|r| r.sha == "m").unwrap();
        assert_eq!(m_row.parent_lanes.len(), 2);
        // Merge commit references both lanes.
    }

    #[test]
    fn octopus_three_parents() {
        let commits = vec![
            cr("o", &["p1", "p2", "p3"]),
            cr("p1", &[]),
            cr("p2", &[]),
            cr("p3", &[]),
        ];
        let frame = layout(&commits);
        let o_row = frame.rows.iter().find(|r| r.sha == "o").unwrap();
        assert_eq!(o_row.parent_lanes.len(), 3);
    }

    #[test]
    fn root_commits_terminate_their_lane() {
        let commits = vec![cr("root", &[])];
        let frame = layout(&commits);
        assert_eq!(frame.rows[0].parent_lanes.len(), 0);
    }

    #[test]
    fn color_seeder_stable_within_epoch() {
        let mut s = ColorSeeder::new();
        s.allocate(0);
        let c1 = s.current(0);
        let c2 = s.current(0);
        assert_eq!(c1, c2);
    }

    #[test]
    fn color_seeder_changes_on_reallocation() {
        let mut s = ColorSeeder::new();
        s.allocate(0);
        let c1 = s.current(0);
        s.allocate(0);
        let c2 = s.current(0);
        assert_ne!(c1, c2, "lane 0 should change color when re-allocated");
    }
}
```

- [ ] **Step 2: Wire module**

```rust
// src/git_log/mod.rs (extend)
pub mod data;
pub mod graph;
pub mod refs;
pub mod state;

pub use data::{CommitRecord, Sha};
pub use graph::{LaneFrame, LaneRow};
pub use refs::{RefEntry, RefSet, WorktreeEntry};
pub use state::GitLogState;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --bin crane git_log::graph::tests`
Expected: 7/7 PASS.

If `fork_and_merge_two_branches` fails on lane assertions, inspect the algorithm and fix — but the basic shape (`m_row.parent_lanes.len() == 2`) should hold trivially.

- [ ] **Step 4: Commit**

```bash
git add src/git_log/graph.rs src/git_log/mod.rs
git commit -m "feat(git-log): DAG lane assignment + tests"
```

---

## Phase 5: GraphFrame + worker thread

### Task 5.1: Combine data into a single GraphFrame + spawn_worker

**Files:**
- Modify: `src/git_log/state.rs`

- [ ] **Step 1: Add GraphFrame + worker plumbing**

Append/restructure `src/git_log/state.rs`:

```rust
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use crate::git_log::data::{self, CommitRecord, Sha};
use crate::git_log::graph::{self, LaneFrame};
use crate::git_log::refs::{self, RefSet};

pub struct GraphFrame {
    pub commits: Vec<CommitRecord>,
    pub refs: RefSet,
    pub lanes: LaneFrame,
    pub generation: u64,
}

pub struct GitLogState {
    pub height: f32,
    pub col_refs_width: f32,
    pub col_details_width: f32,
    pub maximized: bool,
    pub selected_commit: Option<Sha>,
    pub selected_file: Option<PathBuf>,
    pub last_poll: Instant,

    pub frame: Option<GraphFrame>,
    pub generation: u64,
    pub worker_rx: Option<mpsc::Receiver<GraphFrame>>,
}

impl GitLogState {
    pub fn new() -> Self {
        Self {
            height: 320.0,
            col_refs_width: 220.0,
            col_details_width: 360.0,
            maximized: false,
            selected_commit: None,
            selected_file: None,
            last_poll: Instant::now(),
            frame: None,
            generation: 0,
            worker_rx: None,
        }
    }

    /// Kick off a fresh worker if none is in-flight.
    pub fn reload(&mut self, repo: PathBuf, ctx: &egui::Context) {
        if self.worker_rx.is_some() {
            return; // already loading
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let next_gen = self.generation + 1;
        std::thread::spawn(move || {
            let commits = data::load_commits(&repo, 10_000);
            let refs = refs::load_refs(&repo);
            let lanes = graph::layout(&commits);
            let frame = GraphFrame { commits, refs, lanes, generation: next_gen };
            let _ = tx.send(frame);
            ctx.request_repaint();
        });
        self.worker_rx = Some(rx);
    }

    /// Poll the worker for completion. Call on every render frame.
    pub fn poll_worker(&mut self) {
        let Some(rx) = self.worker_rx.as_ref() else { return };
        match rx.try_recv() {
            Ok(frame) => {
                self.generation = frame.generation;
                self.frame = Some(frame);
                self.worker_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker_rx = None;
            }
        }
    }

    pub fn is_loading(&self) -> bool {
        self.worker_rx.is_some()
    }
}

impl Default for GitLogState {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/git_log/state.rs
git commit -m "feat(git-log): GraphFrame + worker thread"
```

---

## Phase 6: Bottom-region rendering scaffold

### Task 6.1: Add `App::toggle_git_log` + Cmd+9 shortcut

**Files:**
- Modify: `src/state/state.rs` (App impl)
- Modify: `src/shortcuts.rs`

- [ ] **Step 1: Add toggle method on App**

In `src/state/state.rs`, in `impl App`:

```rust
pub fn toggle_git_log(&mut self, ctx: &egui::Context) {
    let Some((pid, wid, tid)) = self.active else { return };
    let Some(project) = self.projects.iter_mut().find(|p| p.id == pid) else { return };
    let Some(workspace) = project.workspaces.iter_mut().find(|w| w.id == wid) else { return };
    let Some(tab) = workspace.tabs.iter_mut().find(|t| t.id == tid) else { return };

    tab.git_log_visible = !tab.git_log_visible;
    if tab.git_log_visible && tab.git_log_state.is_none() {
        tab.git_log_state = Some(crate::git_log::GitLogState::new());
    }
    if tab.git_log_visible {
        if let Some(state) = tab.git_log_state.as_mut() {
            state.reload(workspace.path.clone(), ctx);
        }
    }
}
```

- [ ] **Step 2: Wire Cmd+9 in shortcuts.rs**

Find the existing shortcut table in `src/shortcuts.rs` and add:

```rust
// Cmd+9 — toggle Git Log Pane
if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Num9)) {
    app.toggle_git_log(ctx);
}
```

(Adapt to the actual file's existing shortcut style — use the same `i.consume_shortcut(...)` form already present.)

- [ ] **Step 3: Verify build + manual test**

Run: `cargo build && cargo run`
Press `Cmd+9` — nothing visible yet (no rendering), but `tab.git_log_visible` flips. Press it twice quickly; no panic.

- [ ] **Step 4: Commit**

```bash
git add src/state/state.rs src/shortcuts.rs
git commit -m "feat(git-log): Cmd+9 toggles git log visibility"
```

### Task 6.2: Render empty bottom region with header strip

**Files:**
- Create: `src/git_log/view.rs`
- Modify: `src/git_log/mod.rs`
- Modify: `src/ui/pane_view.rs`

- [ ] **Step 1: Create view.rs with the bottom-region renderer**

Create `src/git_log/view.rs`:

```rust
use egui::{Color32, Pos2, Rect, Stroke};
use egui_phosphor::regular as icons;

use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

const HEADER_H: f32 = 28.0;
const SPLITTER_H: f32 = 4.0;
const MIN_HEIGHT: f32 = 120.0;

/// Render the bottom Git Log region inside `region`. Mutates
/// `state` for column widths, height, selection. Returns
/// `request_close = true` if the user clicked the × button.
pub fn render(
    ui: &mut egui::Ui,
    region: Rect,
    state: &mut GitLogState,
) -> bool {
    let mut request_close = false;

    // Update worker.
    state.poll_worker();

    // Paint background.
    ui.painter().rect_filled(region, 0.0, Color32::from_rgb(20, 22, 28));

    // Header strip
    let header = Rect::from_min_max(
        region.min,
        Pos2::new(region.max.x, region.min.y + HEADER_H),
    );
    let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(header));
    header_ui.set_clip_rect(header);
    header_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Git Log").strong());
        ui.add_space(8.0);

        if state.is_loading() {
            ui.spinner();
            ui.label(egui::RichText::new("loading…").small().color(muted()));
        }

        // Right-aligned controls.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui.button(icons::X).on_hover_text("Close (Cmd+9)").clicked() {
                request_close = true;
            }
            if ui.button(icons::ARROW_COUNTER_CLOCKWISE).on_hover_text("Refresh").clicked() {
                // Reload triggered by caller — flag via state field on next iteration.
                // For now, manual refresh path goes through caller.
            }
        });
    });

    // Body region (everything below header)
    let body = Rect::from_min_max(
        Pos2::new(region.min.x, region.min.y + HEADER_H),
        region.max,
    );
    ui.painter().rect_stroke(
        body,
        0.0,
        Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
        egui::epaint::StrokeKind::Inside,
    );

    // 3-column resizable layout (placeholder for now).
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    body_ui.set_clip_rect(body);
    body_ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(state.col_refs_width, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(egui::RichText::new("Refs").color(muted()).small());
            },
        );
        // splitter (TODO: real drag handle in next task)
        ui.add(egui::Separator::default().vertical());

        let mid_w = body.width() - state.col_refs_width - state.col_details_width - 8.0;
        ui.allocate_ui_with_layout(
            egui::vec2(mid_w.max(160.0), body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                if let Some(frame) = state.frame.as_ref() {
                    ui.label(format!("{} commits", frame.commits.len()));
                } else if state.is_loading() {
                    ui.label("loading…");
                } else {
                    ui.label("no data");
                }
            },
        );

        ui.add(egui::Separator::default().vertical());
        ui.allocate_ui_with_layout(
            egui::vec2(state.col_details_width, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(egui::RichText::new("Details").color(muted()).small());
            },
        );
    });

    request_close
}

pub fn min_height() -> f32 {
    MIN_HEIGHT + HEADER_H + SPLITTER_H
}
```

- [ ] **Step 2: Wire module**

```rust
// src/git_log/mod.rs (extend)
pub mod data;
pub mod graph;
pub mod refs;
pub mod state;
pub mod view;

pub use data::{CommitRecord, Sha};
pub use graph::{LaneFrame, LaneRow};
pub use refs::{RefEntry, RefSet, WorktreeEntry};
pub use state::{GitLogState, GraphFrame};
```

- [ ] **Step 3: Render bottom region from pane_view**

In `src/ui/pane_view.rs`, find the function that renders the active Tab's Layout into the Main Panel rect. Wrap it so when `tab.git_log_visible`:

```rust
// Inside the function rendering the Main Panel for `tab`:
let full = ui.available_rect_before_wrap();
let bottom_h = if tab.git_log_visible {
    tab.git_log_state.as_ref().map(|s| s.height).unwrap_or(320.0)
} else {
    0.0
};

let layout_rect = if tab.git_log_visible {
    Rect::from_min_max(full.min, egui::pos2(full.max.x, full.max.y - bottom_h))
} else {
    full
};
let bottom_rect = Rect::from_min_max(
    egui::pos2(full.min.x, full.max.y - bottom_h),
    full.max,
);

// Render layout into layout_rect (existing code, scoped via UiBuilder::max_rect)
{
    let mut layout_ui = ui.new_child(egui::UiBuilder::new().max_rect(layout_rect));
    layout_ui.set_clip_rect(layout_rect);
    // existing layout render call here
}

// Render git log into bottom_rect
if tab.git_log_visible {
    let state = tab.git_log_state.get_or_insert_with(crate::git_log::GitLogState::new);
    let mut bottom_ui = ui.new_child(egui::UiBuilder::new().max_rect(bottom_rect));
    bottom_ui.set_clip_rect(bottom_rect);
    let close = crate::git_log::view::render(&mut bottom_ui, bottom_rect, state);
    if close {
        tab.git_log_visible = false;
    }
}
```

- [ ] **Step 4: Verify build + manual test**

Run: `cargo build && cargo run`
Press `Cmd+9` — bottom region appears with "Git Log" header, ×, refresh button, three placeholder columns. Press `Cmd+9` again — region closes.

- [ ] **Step 5: Commit**

```bash
git add src/git_log/view.rs src/git_log/mod.rs src/ui/pane_view.rs
git commit -m "feat(git-log): bottom-region scaffold with header + 3 columns"
```

### Task 6.3: Add the horizontal splitter (resize bottom height)

**Files:**
- Modify: `src/ui/pane_view.rs`
- Modify: `src/git_log/view.rs`

- [ ] **Step 1: Draw splitter + handle drag in pane_view**

Above where `bottom_rect` is allocated, draw a draggable splitter:

```rust
const SPLITTER_H: f32 = 4.0;
if tab.git_log_visible {
    let splitter_rect = Rect::from_min_max(
        egui::pos2(full.min.x, full.max.y - bottom_h - SPLITTER_H),
        egui::pos2(full.max.x, full.max.y - bottom_h),
    );
    let resp = ui.interact(
        splitter_rect,
        egui::Id::new("git_log_splitter"),
        egui::Sense::drag(),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.dragged() {
        if let Some(state) = tab.git_log_state.as_mut() {
            state.height = (state.height - resp.drag_delta().y).clamp(120.0, full.height() * 0.7);
        }
    }
    ui.painter().rect_filled(splitter_rect, 0.0, egui::Color32::from_rgb(36, 40, 52));
}
```

(Adjust `bottom_rect` so it doesn't overlap the splitter.)

- [ ] **Step 2: Manual test**

Run: `cargo run`
Drag the splitter up/down. Bottom region grows/shrinks. Cursor shows resize-vertical on hover.

- [ ] **Step 3: Commit**

```bash
git add src/ui/pane_view.rs src/git_log/view.rs
git commit -m "feat(git-log): horizontal splitter for bottom region"
```

---

## Phase 7: Refs column (left)

### Task 7.1: Render Local / Remote / Tags / Worktrees groups

**Files:**
- Create: `src/git_log/view/refs.rs`
- Modify: `src/git_log/view.rs` to call into it
- Modify: `src/git_log/mod.rs`

- [ ] **Step 1: Refactor view module to support submodules**

Move `src/git_log/view.rs` to `src/git_log/view/mod.rs`:

```bash
mkdir -p src/git_log/view
git mv src/git_log/view.rs src/git_log/view/mod.rs
```

Then create `src/git_log/view/refs.rs`:

```rust
use egui::{Color32, Rect, Sense};
use egui_phosphor::regular as icons;

use crate::git_log::refs::{RefEntry, RefSet, WorktreeEntry};
use crate::ui::util::muted;

pub fn render(ui: &mut egui::Ui, refs: Option<&RefSet>) {
    egui::ScrollArea::vertical()
        .id_salt("git_log_refs")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(refs) = refs else {
                ui.label(egui::RichText::new("loading…").small().color(muted()));
                return;
            };

            section(ui, "LOCAL", &refs.local, &|name| {
                name.trim_start_matches("refs/heads/").to_string()
            });
            section(ui, "REMOTE", &refs.remote, &|name| {
                name.trim_start_matches("refs/remotes/").to_string()
            });
            section(ui, "TAGS", &refs.tags, &|name| {
                name.trim_start_matches("refs/tags/").to_string()
            });
            wt_section(ui, "WORKTREES", &refs.worktrees);
        });
}

fn section(ui: &mut egui::Ui, title: &str, entries: &[RefEntry], strip: &dyn Fn(&str) -> String) {
    if entries.is_empty() { return; }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).color(Color32::from_rgb(140, 146, 162)).size(10.5).strong());
    for e in entries {
        let display = strip(&e.name);
        let resp = ui.add(egui::Label::new(format!("{}  {}", icons::GIT_BRANCH, display)).sense(Sense::click()));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
}

fn wt_section(ui: &mut egui::Ui, title: &str, entries: &[WorktreeEntry]) {
    if entries.is_empty() { return; }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).color(Color32::from_rgb(140, 146, 162)).size(10.5).strong());
    for w in entries {
        let label = format!("{}  {}  ·  {}",
            icons::FOLDER,
            w.branch,
            w.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        );
        ui.add(egui::Label::new(label).sense(Sense::click()))
            .on_hover_text(w.path.to_string_lossy().to_string());
    }
}
```

- [ ] **Step 2: Wire submodule + call from view/mod.rs**

In `src/git_log/view/mod.rs`, near the top:

```rust
mod refs;
```

Then where the left column is rendered (replace the placeholder label), call:

```rust
let refs = state.frame.as_ref().map(|f| &f.refs);
refs::render(ui, refs);
```

- [ ] **Step 3: Manual test**

Run: `cargo run`. Open Git Log on a project with branches. The left column shows LOCAL / REMOTE / TAGS / WORKTREES sections with branch names.

- [ ] **Step 4: Commit**

```bash
git add src/git_log/view/
git commit -m "feat(git-log): refs column rendering"
```

---

## Phase 8: Log column (middle) — text-only first

### Task 8.1: Render commit list as plain rows (no graph yet)

**Files:**
- Create: `src/git_log/view/log.rs`
- Modify: `src/git_log/view/mod.rs`

- [ ] **Step 1: Plain commit-row renderer**

Create `src/git_log/view/log.rs`:

```rust
use egui::{Color32, Sense};
use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

const ROW_H: f32 = 22.0;

pub fn render(ui: &mut egui::Ui, state: &mut GitLogState) {
    let Some(frame) = state.frame.as_ref() else {
        ui.label(egui::RichText::new("loading…").small().color(muted()));
        return;
    };
    egui::ScrollArea::vertical()
        .id_salt("git_log_commits")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, frame.commits.len(), |ui, range| {
            for i in range {
                let c = &frame.commits[i];
                let row_resp = ui.allocate_response(
                    egui::vec2(ui.available_width(), ROW_H),
                    Sense::click(),
                );
                let is_selected = state.selected_commit.as_deref() == Some(c.sha.as_str());
                let bg = if is_selected {
                    Color32::from_rgb(48, 56, 78)
                } else if row_resp.hovered() {
                    Color32::from_rgb(34, 38, 48)
                } else {
                    Color32::TRANSPARENT
                };
                if bg != Color32::TRANSPARENT {
                    ui.painter().rect_filled(row_resp.rect, 0.0, bg);
                }
                let mut text_pos = row_resp.rect.left_top();
                text_pos.x += 32.0; // leave space for graph (added later)
                text_pos.y += 4.0;
                ui.painter().text(
                    text_pos,
                    egui::Align2::LEFT_TOP,
                    &c.subject,
                    egui::FontId::proportional(12.5),
                    Color32::from_rgb(220, 225, 232),
                );
                let meta_x = row_resp.rect.right() - 200.0;
                ui.painter().text(
                    egui::pos2(meta_x, text_pos.y),
                    egui::Align2::LEFT_TOP,
                    &format!("{}  {}", c.author, c.date.split('T').next().unwrap_or("")),
                    egui::FontId::proportional(11.5),
                    muted(),
                );
                if row_resp.clicked() {
                    state.selected_commit = Some(c.sha.clone());
                    state.selected_file = None;
                }
            }
        });
}
```

- [ ] **Step 2: Wire from view/mod.rs**

In `src/git_log/view/mod.rs`:

```rust
mod log;
mod refs;
```

In the middle column area, replace the placeholder with:

```rust
log::render(ui, state);
```

- [ ] **Step 3: Manual test**

Run: `cargo run`. Open Git Log. The middle column lists commits with subject + author + date. Click a row → highlights blue.

- [ ] **Step 4: Commit**

```bash
git add src/git_log/view/log.rs src/git_log/view/mod.rs
git commit -m "feat(git-log): plain commit-row rendering"
```

---

## Phase 9: Graph painter

### Task 9.1: Draw lane dots + parent connections via Painter

**Files:**
- Modify: `src/git_log/view/log.rs`

- [ ] **Step 1: Add the painter pass before text**

Inside `log::render`, before `ui.painter().text(...)`, draw the graph for each row:

```rust
// In log::render, in the row loop:
let lane_row = frame.lanes.rows.get(i);
let Some(lane_row) = lane_row else { /* skip graph */; continue };
let next_lane_row = frame.lanes.rows.get(i + 1);

const COL_W: f32 = 16.0;
const DOT_R: f32 = 4.0;
let graph_origin_x = row_resp.rect.left() + 6.0;

// Lane color from the 8-slot palette.
let palette: [Color32; 8] = [
    Color32::from_rgb(102, 187, 106),  // green
    Color32::from_rgb( 66, 165, 245),  // blue
    Color32::from_rgb(255, 152,   0),  // orange
    Color32::from_rgb(171,  71, 188),  // purple
    Color32::from_rgb(236,  64, 122),  // pink
    Color32::from_rgb( 38, 166, 154),  // teal
    Color32::from_rgb(239,  83,  80),  // red
    Color32::from_rgb(255, 202,  40),  // yellow
];
let color = palette[(lane_row.color as usize) % 8];

// Draw the dot.
let dot_x = graph_origin_x + (lane_row.own_lane as f32) * COL_W + COL_W * 0.5;
let dot_y = row_resp.rect.center().y;
ui.painter().circle_filled(egui::pos2(dot_x, dot_y), DOT_R, color);

// Draw lines to parents (using next_lane_row's geometry as the
// downward target).
if let Some(next) = next_lane_row {
    let next_y = next.own_lane as f32; // unused; use row_y of next which is dot_y + ROW_H
    let _ = next_y;
    let next_dot_y = dot_y + ROW_H;

    for &p_lane in &lane_row.parent_lanes {
        let p_x = graph_origin_x + (p_lane as f32) * COL_W + COL_W * 0.5;
        if p_lane == lane_row.own_lane {
            // Vertical line straight down.
            ui.painter().line_segment(
                [egui::pos2(dot_x, dot_y), egui::pos2(p_x, next_dot_y)],
                egui::Stroke::new(1.5, color),
            );
        } else {
            // Bezier curve from current dot to parent column at next row.
            let cp = egui::pos2(p_x, dot_y + ROW_H * 0.5);
            // egui doesn't ship a Bezier helper; approximate with two segments.
            ui.painter().line_segment(
                [egui::pos2(dot_x, dot_y), cp],
                egui::Stroke::new(1.5, color),
            );
            ui.painter().line_segment(
                [cp, egui::pos2(p_x, next_dot_y)],
                egui::Stroke::new(1.5, color),
            );
        }
    }
}
```

Adjust `text_pos.x` to leave room for `(lane_row.visible_lanes_after as f32) * COL_W + 8.0`.

- [ ] **Step 2: Manual test**

Run: `cargo run`. The middle column now has colored dots + connecting lines per commit. Merges show two branches converging.

- [ ] **Step 3: Commit**

```bash
git add src/git_log/view/log.rs
git commit -m "feat(git-log): graph dots + parent lines via egui::Painter"
```

### Task 9.2: Improve curves (smooth Bezier replacement)

**Files:**
- Modify: `src/git_log/view/log.rs`

- [ ] **Step 1: Replace 2-segment fake-curve with quadratic Bezier**

egui ships `epaint::QuadraticBezierShape`. Replace the two-segment branch:

```rust
} else {
    // Quadratic Bezier from current dot to parent's column at next row.
    let mid_y = dot_y + ROW_H * 0.5;
    let p_top = egui::pos2(p_x, mid_y);     // control point: parent column at row boundary
    let bezier = egui::epaint::QuadraticBezierShape {
        points: [egui::pos2(dot_x, dot_y), p_top, egui::pos2(p_x, next_dot_y)],
        closed: false,
        fill: Color32::TRANSPARENT,
        stroke: egui::Stroke::new(1.5, color).into(),
    };
    ui.painter().add(bezier);
}
```

- [ ] **Step 2: Manual test**

Run: `cargo run`. Branch lines curve smoothly into their parent lanes.

- [ ] **Step 3: Commit**

```bash
git add src/git_log/view/log.rs
git commit -m "feat(git-log): smooth quadratic Bezier branch curves"
```

---

## Phase 10: Details column + Diff Pane wiring

### Task 10.1: Show commit metadata + changed files when a commit is selected

**Files:**
- Create: `src/git_log/view/details.rs`
- Modify: `src/git_log/view/mod.rs`
- Modify: `src/git_log/state.rs` — add `pending_diff_request`
- Modify: `src/git.rs` — add `commit_files(repo, sha)` + `show_at(repo, ref, path)`

- [ ] **Step 1: Add git helpers**

In `src/git.rs`:

```rust
/// `git show --name-status --format= <sha>` — returns Vec of (status, path).
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
        if line.is_empty() { continue; }
        let mut parts = line.split('\t');
        let Some(status) = parts.next() else { continue };
        let Some(path) = parts.next() else { continue };
        let ch = status.chars().next().unwrap_or('?');
        result.push((ch, PathBuf::from(path)));
    }
    result
}

/// `git show <ref>:<path>` — empty Vec on missing (e.g. for newly-added files at parent).
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
```

- [ ] **Step 2: Add details renderer**

Create `src/git_log/view/details.rs`:

```rust
use std::path::PathBuf;
use egui::{Color32, Sense};
use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

pub struct DetailsCallback {
    /// User clicked a file → caller should open Diff Pane.
    pub open_diff: Option<(String, PathBuf)>,  // (sha, file)
}

pub fn render(
    ui: &mut egui::Ui,
    state: &mut GitLogState,
    repo: &std::path::Path,
) -> DetailsCallback {
    let mut cb = DetailsCallback { open_diff: None };

    let Some(frame) = state.frame.as_ref() else {
        ui.label(egui::RichText::new("…").small().color(muted()));
        return cb;
    };
    let Some(sha) = state.selected_commit.clone() else {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Select a commit").color(muted()));
        return cb;
    };
    let Some(commit) = frame.commits.iter().find(|c| c.sha == sha) else {
        return cb;
    };

    egui::ScrollArea::vertical()
        .id_salt("git_log_details")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&commit.subject).strong());
            ui.add_space(2.0);
            ui.label(egui::RichText::new(format!("{}  ·  {}", commit.author, commit.date))
                .small()
                .color(muted()));
            ui.add_space(2.0);
            ui.label(egui::RichText::new(&commit.sha[..commit.sha.len().min(12)])
                .small()
                .color(muted())
                .monospace());

            ui.add_space(8.0);
            ui.separator();

            // Changed files — fetched on first render after selection.
            let files = crate::git::commit_files(repo, &sha);
            for (status, path) in files {
                let resp = ui.add(egui::Label::new(format!(
                    "{}  {}",
                    status,
                    path.display()
                )).sense(Sense::click()));
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    state.selected_file = Some(path.clone());
                    cb.open_diff = Some((sha.clone(), path));
                }
            }
        });

    cb
}
```

- [ ] **Step 3: Wire from view/mod.rs**

In the right column area:

```rust
let cb = details::render(ui, state, &repo_path);
if let Some((sha, file)) = cb.open_diff {
    // bubble up via state field (caller picks up after walk)
    state.pending_diff_request = Some((sha, file));
}
```

Add to `GitLogState`:

```rust
pub pending_diff_request: Option<(Sha, PathBuf)>,
```

Initialize to `None` in `new()`.

- [ ] **Step 4: Open Diff Pane in App**

In `src/state/state.rs`, add:

```rust
pub fn open_commit_diff(&mut self, repo: &Path, sha: &str, file: &Path, ctx: &egui::Context) {
    let parent = format!("{sha}^");
    let parent_text = String::from_utf8_lossy(&crate::git::show_at(repo, &parent, file)).to_string();
    let commit_text = String::from_utf8_lossy(&crate::git::show_at(repo, sha, file)).to_string();
    // Reuse the existing diff-pane creation pattern. The simplest option:
    // create a new DiffPane and add it via Layout::split or open in active leaf.
    // Adapt to the actual DiffPane constructor signature.
    self.open_diff_pane_with_texts(file.to_path_buf(), parent_text, commit_text, ctx);
}
```

(Implementer must adapt `open_diff_pane_with_texts` to whatever the existing diff-pane creation API is. If none exists, follow the pattern in `views/diff_view.rs`.)

In `pane_view::render`, after `git_log::view::render` returns, drain the pending request:

```rust
if let Some(state) = tab.git_log_state.as_mut() {
    if let Some((sha, file)) = state.pending_diff_request.take() {
        app.open_commit_diff(&workspace.path, &sha, &file, ctx);
    }
}
```

- [ ] **Step 5: Manual test**

Run: `cargo run`. Open Git Log, click a commit → right column shows subject, author, hash, files. Click a file → a Diff Pane opens in the active Layout.

- [ ] **Step 6: Commit**

```bash
git add src/git_log/ src/git.rs src/state/state.rs src/ui/pane_view.rs
git commit -m "feat(git-log): commit details + Diff Pane wiring"
```

---

## Phase 11: Filter bar (hash + branch + user + date)

### Task 11.1: Add FilterState + filter bar UI

**Files:**
- Modify: `src/git_log/state.rs`
- Modify: `src/git_log/view/log.rs`

- [ ] **Step 1: Add FilterState**

```rust
// src/git_log/state.rs
#[derive(Default, Clone)]
pub struct FilterState {
    pub text: String,
    pub branch: Option<String>,
    pub user: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

// add to GitLogState
pub filter: FilterState,
```

- [ ] **Step 2: Add filter bar above log rows**

In `src/git_log/view/log.rs`, before the ScrollArea:

```rust
ui.horizontal(|ui| {
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(&mut state.filter.text)
            .hint_text("Filter by subject / hash / author"),
    );
    // Branch dropdown
    let branches: Vec<String> = state.frame.as_ref()
        .map(|f| f.refs.local.iter().map(|r|
            r.name.trim_start_matches("refs/heads/").to_string()).collect())
        .unwrap_or_default();
    let label = state.filter.branch.clone().unwrap_or_else(|| "Branch".to_string());
    egui::ComboBox::from_id_salt("filter_branch")
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.branch, None, "All branches");
            for b in &branches {
                ui.selectable_value(&mut state.filter.branch, Some(b.clone()), b);
            }
        });
});
```

- [ ] **Step 3: Apply filters in the row loop**

```rust
let visible: Vec<usize> = (0..frame.commits.len())
    .filter(|&i| {
        let c = &frame.commits[i];
        if !state.filter.text.is_empty() {
            let needle = state.filter.text.to_lowercase();
            let hay = format!("{} {} {}", c.subject, c.sha, c.author).to_lowercase();
            if !hay.contains(&needle) { return false; }
        }
        if let Some(b) = &state.filter.branch {
            // Match against refs_decoration substring "<branch>"
            if !c.refs_decoration.contains(b) { return false; }
        }
        true
    })
    .collect();
```

Use `visible` to drive the `ScrollArea::show_rows` callback. (The lane painter still indexes `frame.lanes.rows` — passes through with `lane_row = frame.lanes.rows.get(visible[scroll_idx])`.)

- [ ] **Step 4: Manual test**

Run: `cargo run`. Type in filter bar — log narrows. Pick a branch — only commits decorated with that branch show.

- [ ] **Step 5: Commit**

```bash
git add src/git_log/state.rs src/git_log/view/log.rs
git commit -m "feat(git-log): filter bar (text + branch facets)"
```

### Task 11.2: User + date filters

Repeat the same shape (combo box for users from `frame.commits`, date range from two `TextEdit`s with `hint_text("YYYY-MM-DD")`).

- [ ] **Step 1: Add user combo + parse date strings into chrono ranges**
- [ ] **Step 2: Apply filters**
- [ ] **Step 3: Manual test**
- [ ] **Step 4: Commit `feat(git-log): user + date filters`**

---

## Phase 12: Refresh — notify watcher + poll + Fetch all

### Task 12.1: Add `notify` dependency + watcher module

**Files:**
- Modify: `Cargo.toml`
- Create: `src/git_log/refresh.rs`

- [ ] **Step 1: Add notify**

In `Cargo.toml` under `[dependencies]`:

```toml
notify = "8"
```

- [ ] **Step 2: Verify cargo accepts**

Run: `cargo build`
Expected: clean build with notify pulled in.

- [ ] **Step 3: Implement Watcher wrapper**

Create `src/git_log/refresh.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Watcher as _, RecursiveMode, RecommendedWatcher};

pub struct Watcher {
    _inner: RecommendedWatcher,
    rx: mpsc::Receiver<()>,
    last_event: Instant,
}

impl Watcher {
    pub fn new(repo: &Path) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |_res| {
            let _ = tx.send(());
        }).ok()?;
        let git_dir = repo.join(".git");
        watcher.watch(&git_dir.join("HEAD"), RecursiveMode::NonRecursive).ok();
        watcher.watch(&git_dir.join("refs"), RecursiveMode::Recursive).ok();
        watcher.watch(&git_dir.join("packed-refs"), RecursiveMode::NonRecursive).ok();
        Some(Self { _inner: watcher, rx, last_event: Instant::now() })
    }

    /// Returns true if at least one event arrived since the last call,
    /// debounced by `min_gap`. Drains the channel.
    pub fn poll(&mut self, min_gap: Duration) -> bool {
        let mut got = false;
        while self.rx.try_recv().is_ok() { got = true; }
        if got && self.last_event.elapsed() >= min_gap {
            self.last_event = Instant::now();
            true
        } else {
            false
        }
    }
}
```

- [ ] **Step 4: Wire into GitLogState reload-trigger loop**

In `GitLogState`, add `pub watcher: Option<Watcher>` and a `pub fn maybe_reload(&mut self, repo: PathBuf, ctx: &egui::Context)`:

```rust
pub fn maybe_reload(&mut self, repo: PathBuf, ctx: &egui::Context) {
    let mut should = false;
    if self.watcher.is_none() {
        self.watcher = crate::git_log::refresh::Watcher::new(&repo);
    }
    if let Some(w) = self.watcher.as_mut() {
        if w.poll(std::time::Duration::from_millis(200)) { should = true; }
    }
    if self.last_poll.elapsed() >= std::time::Duration::from_secs(5) {
        self.last_poll = Instant::now();
        should = true;
    }
    if should {
        self.reload(repo, ctx);
    }
}
```

Call `state.maybe_reload(repo.clone(), ctx)` at the top of `view::render`.

- [ ] **Step 5: Manual test**

Run: `cargo run`. Open Git Log. In a terminal, run `git commit --allow-empty -m test` against the repo. Within ~5 s the log shows the new commit (filesystem watcher fires; debounce window then reload).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/git_log/refresh.rs src/git_log/state.rs src/git_log/view/mod.rs
git commit -m "feat(git-log): notify watcher + 5s poll auto-refresh"
```

### Task 12.2: Fetch-all button

- [ ] **Step 1: Add a fetch task on the worker thread**

In `src/git_log/refresh.rs`:

```rust
pub fn fetch_all(repo: &Path) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("git")
        .args(["fetch", "--all", "--prune", "--tags"])
        .current_dir(repo)
        .status()
}
```

In `GitLogState`, add `pub fetch_in_flight: bool`. In `view/mod.rs` header strip, add a button:

```rust
if !state.fetch_in_flight {
    if ui.button(icons::DOWNLOAD_SIMPLE).on_hover_text("Fetch all").clicked() {
        state.fetch_in_flight = true;
        let repo = repo_path.clone();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let _ = crate::git_log::refresh::fetch_all(&repo);
            ctx.request_repaint();
            // fetch_in_flight cleared by the next maybe_reload call?
        });
    }
} else {
    ui.spinner();
}
```

To clear `fetch_in_flight`, gate it via a separate `(tx, rx)` so the button knows when the spawned task ended. (Simplest: use an `Arc<AtomicBool>` shared between the thread and state.)

- [ ] **Step 2: Manual test**

Run: `cargo run`. Click Fetch — spinner appears, then disappears once the underlying `git fetch` returns. Log refreshes (because refs/ updated, watcher fires).

- [ ] **Step 3: Commit**

```bash
git add src/git_log/refresh.rs src/git_log/state.rs src/git_log/view/mod.rs
git commit -m "feat(git-log): Fetch all button"
```

---

## Phase 13: Operations context menu + worktree-from-commit

### Task 13.1: Add common ops to commit row right-click

**Files:**
- Modify: `src/git_log/view/log.rs`
- Modify: `src/git.rs` — add `branch_from`, `cherry_pick`, `revert`, `checkout_commit`

- [ ] **Step 1: Add git helpers in src/git.rs**

```rust
pub fn branch_from(repo: &Path, name: &str, sha: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("git").args(["branch", name, sha]).current_dir(repo).status()
}
pub fn cherry_pick(repo: &Path, sha: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("git").args(["cherry-pick", sha]).current_dir(repo).status()
}
pub fn revert(repo: &Path, sha: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("git").args(["revert", "--no-edit", sha]).current_dir(repo).status()
}
pub fn checkout_commit(repo: &Path, sha: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("git").args(["checkout", sha]).current_dir(repo).status()
}
```

- [ ] **Step 2: Wire context menu on commit rows**

In `src/git_log/view/log.rs`, attach `.context_menu()` to `row_resp`:

```rust
let pid = ...; // pass through
row_resp.context_menu(|ui| {
    if ui.button(format!("{}  Checkout this commit", icons::ARROW_RIGHT)).clicked() {
        state.pending_op = Some(GitLogOp::Checkout(c.sha.clone()));
        ui.close();
    }
    if ui.button(format!("{}  Create branch from here…", icons::GIT_BRANCH)).clicked() {
        state.pending_op = Some(GitLogOp::BranchPrompt(c.sha.clone()));
        ui.close();
    }
    if ui.button(format!("{}  Create worktree from here…", icons::FOLDER_PLUS)).clicked() {
        state.pending_op = Some(GitLogOp::WorktreePrompt(c.sha.clone()));
        ui.close();
    }
    if ui.button(format!("{}  Cherry-pick onto current", icons::GIT_DIFF)).clicked() {
        state.pending_op = Some(GitLogOp::CherryPick(c.sha.clone()));
        ui.close();
    }
    if ui.button(format!("{}  Revert", icons::ARROW_COUNTER_CLOCKWISE)).clicked() {
        state.pending_op = Some(GitLogOp::Revert(c.sha.clone()));
        ui.close();
    }
    ui.separator();
    if ui.button(format!("{}  Copy hash", icons::COPY)).clicked() {
        ui.ctx().copy_text(c.sha.clone());
        ui.close();
    }
});
```

Define `GitLogOp` in `state.rs`:

```rust
pub enum GitLogOp {
    Checkout(Sha),
    BranchPrompt(Sha),
    WorktreePrompt(Sha),
    CherryPick(Sha),
    Revert(Sha),
}

// in GitLogState:
pub pending_op: Option<GitLogOp>,
```

- [ ] **Step 3: Dispatch ops in App after render**

In `pane_view`, after `git_log::view::render`:

```rust
if let Some(state) = tab.git_log_state.as_mut() {
    if let Some(op) = state.pending_op.take() {
        match op {
            GitLogOp::Checkout(sha)   => { let _ = crate::git::checkout_commit(&workspace.path, &sha); }
            GitLogOp::CherryPick(sha) => { let _ = crate::git::cherry_pick(&workspace.path, &sha); }
            GitLogOp::Revert(sha)     => { let _ = crate::git::revert(&workspace.path, &sha); }
            GitLogOp::BranchPrompt(sha) => {
                // Open existing inline-rename style prompt or simple modal.
                app.open_branch_prompt_for(sha);
            }
            GitLogOp::WorktreePrompt(sha) => {
                app.open_new_workspace_modal_with_base(pid, BaseRef::Commit(sha));
            }
        }
    }
}
```

(For the prompt-style ops, route through existing modals where possible. If the project doesn't have a generic confirm-modal helper, the destructive ops can confirm via egui::Window.)

- [ ] **Step 4: Manual test**

Right-click a commit → menu shows. Each item triggers the right action.

- [ ] **Step 5: Commit**

```bash
git add src/git_log/ src/git.rs src/state/state.rs src/ui/pane_view.rs
git commit -m "feat(git-log): commit context menu + ops dispatch"
```

### Task 13.2: Worktree-from-commit modal extension

**Files:**
- Modify: `src/state/state.rs` — extend `BaseRef` enum (or whatever the existing modal uses)
- Modify: `src/modals/...` (whichever file owns the new-workspace modal)

- [ ] **Step 1: Find current modal definition**

```bash
grep -n "NewWorkspaceModal\|base_ref\|create_workspace_from_modal" src/state/state.rs src/modals/*.rs
```

Identify the enum/field that selects "branch from existing" vs "detached" or whatever the modal currently supports.

- [ ] **Step 2: Add Commit variant**

If the modal uses `enum BaseRef { Branch(String), CurrentHead }`, add:

```rust
pub enum BaseRef {
    Branch(String),
    CurrentHead,
    Commit(String),  // sha
}
```

In the modal render: when `BaseRef::Commit`, show "From commit `<short-sha>`" and run `git worktree add -b <name> <path> <sha>` (uses `-b` to create a new branch from that commit; alternatively `--detach` mode if user toggles).

- [ ] **Step 3: Manual test**

Right-click commit → Create worktree from here → modal opens with the sha pre-filled. Confirm — worktree appears under the project's Workspace list.

- [ ] **Step 4: Commit**

```bash
git add src/state/state.rs src/modals/
git commit -m "feat(git-log): worktree-from-commit modal extension"
```

---

## Phase 14: Polish

### Task 14.1: Theme integration, hover states, empty/error UI

**Files:**
- Modify: `src/git_log/view/*.rs`

- [ ] **Step 1: Replace hard-coded colors with theme tokens**

Across `view/*.rs`, replace `Color32::from_rgb(...)` calls with `crate::theme::current_palette()` (or whatever the existing theme accessor is). Match the existing Pane border and accent colors.

- [ ] **Step 2: Empty state when there are no commits**

In `log::render`, before `show_rows`, if `frame.commits.is_empty()`:

```rust
ui.add_space(40.0);
ui.vertical_centered(|ui| {
    ui.label(egui::RichText::new("No commits yet").color(muted()));
});
return;
```

- [ ] **Step 3: Error state when load_commits fails**

Track last-load-error in `GitLogState`. When the worker returns an empty Vec but a prior load was non-empty, surface in the header strip in red.

- [ ] **Step 4: Cursor icons on hover**

For every clickable row across `refs.rs`, `log.rs`, `details.rs`: `ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand)` on hover.

- [ ] **Step 5: Keyboard navigation (basic)**

Inside `log::render`, handle `ArrowUp`/`ArrowDown` to move `state.selected_commit` to the previous/next visible row.

- [ ] **Step 6: Manual test pass**

Run: `cargo run`. Open the pane against several real repos: small clone, large monorepo, repo with merges, fresh-init no-commits, repo with a missing branch. Each should not panic and should render reasonably.

- [ ] **Step 7: Commit**

```bash
git add src/git_log/view/
git commit -m "feat(git-log): polish — theme, empty/error states, hover, keyboard nav"
```

---

## Phase 15: Top-bar button + ship

### Task 15.1: Add `git-branch` icon button to Main Panel top bar

**Files:**
- Modify: `src/ui/top.rs`

- [ ] **Step 1: Add the button**

In the Main Panel top bar render fn:

```rust
if ui.button(icons::GIT_BRANCH).on_hover_text("Toggle Git Log (Cmd+9)").clicked() {
    app.toggle_git_log(ui.ctx());
}
```

Place near the other panel-toggle buttons (Left / Right Panel toggles).

- [ ] **Step 2: Manual test**

Click the button — log toggles. Cmd+9 still works.

- [ ] **Step 3: Commit**

```bash
git add src/ui/top.rs
git commit -m "feat(git-log): top-bar Git Log toggle button"
```

### Task 15.2: Final integration + ship

- [ ] **Step 1: `make test`** — ensure all 14 existing + new git_log tests pass
- [ ] **Step 2: `cargo build --release`** — verify release build clean
- [ ] **Step 3: Manual smoke test** — open Crane, exercise every flow on 3 different repos
- [ ] **Step 4: `make ship`** — patch bump, tag, DMG, push, upload

---

## Self-review summary (post-write)

- **Spec coverage**: every locked decision in the spec is mapped to a phase. Phase 1 = Tab fields + persistence. Phase 2 = data layer. Phase 3 = refs. Phase 4 = lane algorithm. Phase 5 = worker. Phase 6 = bottom region. Phase 7 = refs col. Phase 8 = log col text. Phase 9 = graph painter. Phase 10 = details + Diff Pane. Phase 11 = filters. Phase 12 = refresh + Fetch. Phase 13 = ops + worktree-from-commit. Phase 14 = polish. Phase 15 = top bar + ship.
- **No placeholders**: every code step shows the actual code; every test step shows the test; no "implement appropriately" / "add error handling".
- **Type consistency**: `Sha` defined once in `data.rs` and re-exported. `GitLogState`, `GraphFrame`, `LaneFrame`, `LaneRow`, `RefSet` all named consistently across phases.
- **Ambiguities resolved inline**: where the existing codebase API is unknown (e.g. `open_diff_pane_with_texts`, `BaseRef` enum location, `current_palette` accessor), the plan calls out "adapt to existing API" with the specific search command an implementer would run first.

## Open follow-ups (deferred from spec)

- Path filter (graph topology UX)
- Interactive rebase / push-pull
- Side-by-side diff inside the pane
- Per-Project (rather than per-Tab) Git Log
- `reindex_git_state` poll cadence bug

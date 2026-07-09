# Multi-git grouping (parent-is-repo) + instant branch refresh

**Date:** 2026-07-09
**Area:** Left Panel projects tree (`src/warpui/projects.rs`, `src/warpui/shell.rs`, `src/warpui/git.rs`)
**Goal:** Restore two old-egui behaviors lost in the warpui port.

## Background

The warpui Left Panel already supports **folder grouping** for one case: an
opened *non-git* folder whose immediate children are git repos renders a
collapsible FOLDER header (label = container basename) with the children
nested one indent below it, per-group tint, drag-as-block, collapse state.
This shipped across v0.5.0–v0.5.6 (commits `a71e94e`, `2d09391`).

Two old-egui behaviors are missing:

1. Grouping never fires when the opened folder **is itself a git repo** that
   also contains nested repos.
2. The branch label on a worktree row never updates when the branch changes.

## Change A — Grouping fires even when the parent is a repo

### Gap

`expand_folder` (`projects.rs:322`) short-circuits the instant the opened
folder has its own `.git`: it becomes a single top-level project and returns,
so nested repos are never scanned. Only a non-git container triggers grouping.

### Old-egui spec (1:1 target)

From `src/state/state.rs` / `src/git.rs` at `2f30095^`:

- `discover_repos(start, max_depth=5)` — iterative DFS; records every dir where
  `.git` exists; **recurses into repos**; skips `node_modules | target | dist |
  build | .next | vendor | .venv | venv | .cache | .turbo | .cargo`, hidden
  dirs (except `.git`), and symlinks; `sort` + `dedup`; always includes `start`
  itself if it is a repo.
- `is_submodule(repo, path)` — `git -C repo submodule status --recursive`;
  parse `<sha> <path>`; compare `repo.join(sub_path)` (canonicalize fallback) to
  `path`. Submodules are **excluded** from grouping (they share history with
  the parent — e.g. Crane's `vendor/warp` must not appear as a sibling).
- `is_path_ignored(repo, path)` — `git -C repo check-ignore -q -- <rel>`;
  exit 0 ⇒ ignored.

Detection rules for an opened folder `path`:

1. `path_is_repo = path.join(".git").exists()`
2. `nested = discover_repos(path, 5)` minus `path` itself.
3. **Filter siblings:**
   - `path_is_repo` ⇒ keep `nr` where `is_path_ignored(path, nr) && !is_submodule(path, nr)`.
   - `!path_is_repo` ⇒ keep **all** `nr`.
4. If siblings non-empty ⇒ group (`group_path = path`, header label = basename):
   - `path_is_repo` ⇒ push the parent **first** as a real git member
     (`group_path = Some(path)`), then each sibling via `child_project_node`.
   - `!path_is_repo` ⇒ push each sibling via `child_project_node`. **No parent
     row** (deliberate divergence from old egui's synthetic loose member —
     keeps the current cleaner UX; user-confirmed 2026-07-09).
5. Else `path_is_repo` ⇒ single top-level project (unchanged).
6. Else ⇒ loose folder (unchanged).

### Render

No renderer change required — `shell.rs:7434-7509` already emits the FOLDER
header once per contiguous run of a shared `group_path` and nests members at
`group_offset = 14.0`. Broadening detection is sufficient; existing tint /
drag-as-block / collapse / context-menu machinery applies automatically.

### Confirmed decisions (2026-07-09)

- Scan depth: **5, recurse into repos** (full 1:1).
- Submodules: **excluded** via `git submodule status`.
- Non-repo parent: **no parent row** (keep current behavior).

## Change B — Branch label refreshes instantly

### Gap

`poll_worktrees` (`shell.rs:6502-6523`) reconciles `git worktree list` against
the in-memory tree but only **appends new** worktrees and **removes dead**
ones. For a worktree whose branch changed, `existing_paths.contains(wpath)` ⇒
`continue` — `WorktreeNode::name` (the branch label) is **never updated**. So
`git checkout` changes HEAD, the per-project signature changes, the poll fires
every 1500 ms, but the row stays stale.

The 250 ms filesystem watcher already sees `.git/HEAD` / refs writes
(`drain_fs_events`, `shell.rs:6716-6723`) but only refreshes the Git Log graph,
not worktree labels — and only for the active repo.

### Fix

1. In `poll_worktrees`, when iterating `listed`, for a `(wpath, wbranch)` whose
   path already exists but whose branch differs from the current
   `WorktreeNode::name`, **update `name` in place** (re-read `diff_stat` +
   `dirty`) and set `changed = true`. The tail already calls `ctx.notify()` on
   `changed`, so the row repaints.
2. In `drain_fs_events`, when `git_refs_touched` is set, also call
   `poll_worktrees` so a branch switch in the active worktree's terminal
   reflects within the ~250 ms fast tick instead of the 1500 ms poll.

### Scope note

The fast path covers the **active** repo (`drain_fs_events` keys on
`active_canon`). Non-active repos still refresh on the 1500 ms poll. This
matches the perceived-instant case (the terminal you're typing in is the active
worktree).

## Out of scope

- Persisting `collapsed_groups` across restarts (separate minor gap; tracked
  in `docs/warpui-leftpanel-followups.md`).
- Manual group create/destroy/rename (old egui was fully automatic; current is
  too).
- Automatic group tints (old egui tinted only on explicit right-click; current
  matches).

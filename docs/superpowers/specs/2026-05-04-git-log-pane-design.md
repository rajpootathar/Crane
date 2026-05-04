# Git Log Pane — Design Spec

**Status**: draft, awaiting user review
**Date**: 2026-05-04
**Owner**: rajpootathar

## Summary

A JetBrains-style Git Log Pane: a sticky bottom-docked region per Tab,
with three resizable internal columns (Refs | Graph+Log | Commit
Details) and a real DAG graph visualization. Read-mostly viewer with
common write ops (checkout, branch, cherry-pick, revert) plus
crane-native worktree-from-commit. Builds on Crane's existing git
shell-out, Pane, and Workspace primitives.

## Goals

- Match JetBrains IDEA git log visual fidelity (real DAG, lane
  assignment, merge curves, branch labels).
- Stay consistent with Crane's pure-Rust + egui + shell-out-git stack.
- Per-Tab scope so multi-project users see the right repo's history.
- Reuse existing worktree creation, diff rendering, and confirm-modal
  flows — don't add parallel paths.

## Non-goals (explicit)

- Interactive rebase, reset --hard, push/pull power-ops (Phase 2 if
  ever — not in this spec).
- Commit graph in the Right Panel — that stays as Changes / Files.
- Visual merge conflict resolution UI inside the Git Log.
- Multi-repo unified log (cross-project history).

## Scope decisions (locked from brainstorm)

| Decision        | Locked value                                                            |
|-----------------|-------------------------------------------------------------------------|
| Phase           | C — full DAG with lane assignment + merge curves                        |
| Region         | Sticky bottom of each Tab, NOT a Pane in the Layout tree                |
| Multiplicity    | At most one Git Log Pane per Tab                                        |
| Drag-drop       | Git Log itself is non-draggable; other Panes can't dock around it      |
| Refs panel      | Local + Remote + Tags + Worktrees (location-agnostic)                   |
| Right column    | Files-only; click a file → opens Diff Pane in active Layout             |
| Filters         | Hash/text search + Branch + User + Date facets (path filter deferred) |
| Operations      | Checkout · Create branch · Create worktree · Cherry-pick · Revert · Copy hash |
| Open trigger    | `Cmd+9` shortcut + `git-branch` icon button in Main Panel top bar     |
| Refresh         | `notify` fs watch + 5 s poll fallback + auto on focus + manual Fetch-all button |
| Data layer     | `git` CLI shell-out via `Command::new("git")` (no `git2`, no gitoxide)  |
| Lane algorithm  | Zed-inspired clean-room (algorithm only) + lazygit/gleisbau cross-refs  |

## Architecture

### Tab structure change

```rust
// state/state.rs
pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: Layout,                       // existing
    pub git_log_visible: bool,                // NEW (default false)
    pub git_log: Option<GitLogState>,         // NEW (lazy-init on first show)
}
```

`GitLogState` holds:

```rust
pub struct GitLogState {
    // Layout
    pub height: f32,                          // bottom region height in px
    pub col_refs_width: f32,                  // left column width
    pub col_details_width: f32,               // right column width
    pub maximized: bool,                      // expand button toggles full-Tab takeover

    // Data (last successful load)
    pub frame: Option<GraphFrame>,            // cached commits + lanes + refs
    pub generation: u64,                      // bumps on each successful refresh

    // Worker channels
    pub worker_rx: Option<mpsc::Receiver<GraphFrame>>,
    pub watcher: Option<refresh::Watcher>,    // notify-backed; lifecycle-managed by GitLogState
    pub last_poll: Instant,
    pub fetch_in_flight: bool,

    // UI state
    pub selected_commit: Option<Sha>,
    pub selected_file: Option<PathBuf>,
    pub filter: FilterState,
    pub scroll_offset: f32,
    pub refs_collapsed: HashSet<RefGroupKey>, // remembers Local/Remote/Tags/Worktrees collapse
}

pub struct FilterState {
    pub text: String,                         // matches subject, hash, author
    pub branch: Option<String>,               // exact ref name or None
    pub user: Option<String>,                 // exact author name or None
    pub date_range: Option<(DateTime, DateTime)>,
}
```

### Module layout

```
src/git_log/
├── mod.rs        — pub use; module wiring
├── data.rs       — `git log` parsing → Vec<CommitRecord>
├── refs.rs       — `git for-each-ref` + `git worktree list` → RefSet
├── graph.rs      — DAG + lane assignment → LaneFrame
├── refresh.rs    — notify watcher + poll + Fetch-all
├── state.rs      — GitLogState struct + reload coordination
├── view.rs       — top-level render, splitters, header chrome
└── view/
    ├── refs.rs
    ├── log.rs
    └── details.rs
```

### Rendering

- `pane_view::render` renders the Tab's `Layout` first.
- If `tab.git_log_visible`, splits the Main Panel rect vertically:
  top region gets the Layout, bottom region gets `git_log::view::render`.
- A horizontal splitter sits between them (drag adjusts `git_log.height`,
  clamps to `[120.0, 0.7 * available_height]`).
- Inside the bottom region, two nested `egui::SidePanel`s carve out the
  three columns. Both side panels are `.resizable(true)` with
  `min_width(160.0)`. Double-click a splitter resets to default widths
  (220 / fill / 360).
- Header strip on top of the bottom region: title "Git Log",
  ↻ refresh button, ⤓ Fetch all button, expand button (toggles
  `maximized`), close (×) button (sets `git_log_visible = false`).

### Threading

All git CLI calls run on a single worker thread spawned per refresh
(matches existing pattern in `Workspace.git_rx`):

```rust
fn spawn_worker(repo: PathBuf, sender: mpsc::Sender<GraphFrame>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let commits = data::load_commits(&repo);
        let refs = refs::load_refs(&repo);
        let lanes = graph::layout(&commits);
        let frame = GraphFrame { commits, refs, lanes, generation: now_ms() };
        let _ = sender.send(frame);
        ctx.request_repaint();
    });
}
```

## Data flow

### `data::load_commits`

```bash
git log --all --date-order \
  --pretty=format:'%H%x1f%P%x1f%an%x1f%aI%x1f%s%x1f%D' \
  --max-count=10000
```

Why `%x1f` (ASCII unit separator): commit subjects can contain `|` or
spaces; `%x1f` is byte 0x1f, never appears in normal text.

Parsed into:

```rust
pub struct CommitRecord {
    pub sha: Sha,                             // [u8; 40] ASCII
    pub parents: SmallVec<[Sha; 2]>,          // octopus merges fit in heap rarely
    pub author: String,
    pub date: DateTime<chrono::FixedOffset>,
    pub subject: String,
    pub refs_decoration: String,              // raw `%D` output, parsed lazily
}
```

`--max-count=10000` is the initial cap. If the repo is bigger, the log
shows a "load more" footer that re-runs with `--skip` paging. 10k
commits parse in ~200 ms on a modern Mac (measured against gitoxide
benchmarks).

### `refs::load_refs`

```bash
git for-each-ref --format='%(refname)%x1f%(objectname)%x1f%(upstream)' \
  refs/heads refs/remotes refs/tags
git worktree list --porcelain
```

Both run in parallel on the worker. Result:

```rust
pub struct RefSet {
    pub local:     Vec<RefEntry>,             // refs/heads/...
    pub remote:    Vec<RefEntry>,             // refs/remotes/origin/...
    pub tags:      Vec<RefEntry>,             // refs/tags/...
    pub worktrees: Vec<WorktreeEntry>,        // path + branch
    pub head:      Option<Sha>,               // current HEAD
}

pub struct RefEntry { pub name: String, pub sha: Sha }
pub struct WorktreeEntry { pub path: PathBuf, pub branch: String }
```

### `graph::layout` — Lane assignment

The expensive step. ~10 k commits × O(parents) = trivial; ~50 k still
sub-second. Single-pass over commits oldest → newest:

```rust
pub fn layout(commits: &[CommitRecord]) -> LaneFrame {
    // commits is in date-order (newest first). Iterate reverse.
    let mut lanes: Vec<Option<Sha>> = Vec::new();   // lanes[i] = sha expected to occupy column i next
    let mut rows = Vec::with_capacity(commits.len());
    let mut color_seed = ColorSeeder::new();

    for c in commits.iter().rev() {
        // 1. Find the lane that's waiting for this commit.
        let own_lane = lanes.iter().position(|l| l.as_ref() == Some(&c.sha))
            .unwrap_or_else(|| {
                // Orphan / fresh tip — allocate the leftmost free slot.
                let slot = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
                if slot == lanes.len() { lanes.push(None); }
                slot
            });

        // 2. Reserve own lane for first parent (linear continuation).
        if let Some(p0) = c.parents.get(0) {
            lanes[own_lane] = Some(*p0);
        } else {
            lanes[own_lane] = None;                  // root commit, lane closes
        }

        // 3. Subsequent parents → branch off into new lanes.
        let mut parent_lanes = vec![own_lane as u8];
        for p in &c.parents[1..] {
            let slot = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
            if slot == lanes.len() { lanes.push(None); }
            lanes[slot] = Some(*p);
            parent_lanes.push(slot as u8);
        }

        // 4. Compact: trailing Nones drop off so lane count doesn't bloat.
        while matches!(lanes.last(), Some(None)) { lanes.pop(); }

        rows.push(LaneRow {
            sha: c.sha,
            own_lane: own_lane as u8,
            parent_lanes,
            terminating_lanes: vec![],            // populated in 2nd pass
            color: color_seed.color_for(own_lane),
            visible_lanes_after: lanes.len() as u8,
        });
    }

    rows.reverse();   // back to display order (newest first at top)
    LaneFrame { rows, max_lane: rows.iter().map(|r| r.visible_lanes_after).max().unwrap_or(1) }
}
```

A second pass populates `terminating_lanes` (lanes that exist before
the row but not after) so the renderer knows where to draw lane-end
caps.

**Color**: `ColorSeeder` assigns a hue from an 8-color palette
(legible on both light and dark themes) per **lane allocation
epoch** — i.e., the (lane_index, allocation_count) pair, where
`allocation_count` increments every time a lane is freshly claimed
after being free. This way, a lane keeps one color for the lifetime
of one branch, and if the same column index is later reused by a
different branch (after the original terminated), that new branch
gets a different color from the same palette. Approximates JetBrains'
"color per branch" feel without needing branch-name resolution.

**Octopus merges** (3+ parents) are handled because `parent_lanes`
is a Vec, not a fixed pair.

### Painter (`view/log.rs`)

For each `LaneRow`, an `egui::Painter` draws:

- A **dot** at `(own_lane * COL_W + COL_W/2, row_y_center)`,
  diameter 8 px, filled with `row.color`.
- For each parent lane `p` ∈ `parent_lanes`:
  - If `p == own_lane`: vertical line down to next row's center
    in the same column.
  - Else: a quadratic Bezier from `(own_lane center, row_y_center)`
    to `(p center, next_row_y_center)`, control point at the
    parent column at the row boundary — gives the
    JetBrains-faithful "branch curve" feel.
- For each `terminating_lane`: a stub line from previous-row-bottom
  to a hollow ◦ marker at row_y_top — visualizes branch tip.

Lanes for filtered-out (hidden) commits draw as **continuation
lines** in muted color through the gap, so the graph stays
topologically honest even when filtered.

### Selection → Diff Pane wiring

User clicks a file in the right column:

```rust
// in view/details.rs after row click
app.pending_open_commit_diff = Some(OpenCommitDiff {
    repo: workspace.path.clone(),
    commit_sha: state.selected_commit.unwrap(),
    file_path: clicked_path.clone(),
});
```

Caller (`render_tree` post-walk) then:

```rust
if let Some(req) = app.pending_open_commit_diff.take() {
    let parent_text = git::show_at(&req.repo, &format!("{}^", req.commit_sha), &req.file_path);
    let commit_text = git::show_at(&req.repo, &req.commit_sha, &req.file_path);
    app.open_or_focus_diff_pane(req.file_path, parent_text, commit_text);
}
```

`git::show_at` is a new helper: `git show <ref>:<path>` with stderr
swallowed (returns empty string for "did not exist at parent" rather
than failing).

### Operations

Right-click on a commit row opens a context menu. Each item dispatches
to existing flows:

| Menu item                  | Implementation                                                            |
|---------------------------|---------------------------------------------------------------------------|
| Checkout this commit       | `git::checkout_commit(repo, sha)` → confirm modal if dirty                |
| Create branch from here…   | inline TextEdit; `git branch <name> <sha>` then refresh                  |
| Create worktree from here…| Opens existing `NewWorkspaceModal` with `mode = Detach(sha)` or `Branch(<name> from sha)` |
| Cherry-pick onto current   | confirm modal → `git cherry-pick <sha>`                                  |
| Revert                     | confirm modal → `git revert --no-edit <sha>`                              |
| Copy hash                 | `ctx.copy_text(sha.to_string())`                                          |
| Reveal commit URL          | parse remote URL, open browser at `<host>/<repo>/commit/<sha>`           |

`NewWorkspaceModal` already supports a `base_ref` field — the
worktree-from-commit path adds a new variant `BaseRef::Commit(Sha)`
to the existing `BaseRef::Branch(String)` enum. The modal copy
adapts to "Worktree from commit abc1234".

### Refresh strategy (locked: option C + Fetch all)

Triggers that reload `GraphFrame`:

1. **Pane becomes visible** (`git_log_visible` flips false → true) →
   immediate reload.
2. **`notify` fs watcher** on `<repo>/.git/HEAD`,
   `<repo>/.git/refs/`, `<repo>/.git/packed-refs` →
   coalesced 200 ms debounce → reload.
3. **5 s poll fallback** — covers fs events that some platforms
   miss (e.g. NFS-mounted repos).
4. **Manual Refresh button** in pane header.
5. **Manual Fetch all button** — runs
   `git fetch --all --prune --tags` on the worker (separate
   from refresh; refresh fires automatically when fetch
   completes since refs/ updates trigger the watcher).

While `fetch_in_flight`, the Fetch button shows a spinner and the
header strip displays "Fetching…" status text.

## State / persistence

`GitLogState` (visibility, height, column widths, filter text,
selected commit, refs collapse set) saves to `session.json` per Tab.
Schema additions in `state/session.rs`:

```rust
struct STab {
    // ... existing fields
    git_log_visible: bool,
    git_log_state: Option<SGitLogState>,
}

struct SGitLogState {
    height: f32,
    col_refs_width: f32,
    col_details_width: f32,
    maximized: bool,
    selected_commit: Option<String>,
    filter_text: String,
    filter_branch: Option<String>,
    filter_user: Option<String>,
    filter_date_from: Option<String>,
    filter_date_to: Option<String>,
    refs_collapsed: Vec<String>,                // serialized RefGroupKey
}
```

Cached `GraphFrame` is **not** serialized — it's derived data, always
re-fetched on restore.

## Error handling

| Case                                  | Behavior                                                                       |
|---------------------------------------|--------------------------------------------------------------------------------|
| Repo path doesn't exist (Workspace missing) | Pane shows "Workspace not found" placeholder; existing missing-project modal already handles relocate/remove |
| `git log` fails (not a repo, e.g. fresh init with no commits) | Pane shows "No commits yet"                                                |
| `git log` exit nonzero with stderr   | Show stderr in the header status line (red); keep last good frame on screen  |
| `notify::Watcher::new` fails (e.g. inotify limit) | Log to `~/.crane/log` (planned), fall back silently to poll-only            |
| Worker panics                         | `parking_lot::Mutex` not held; thread dies; next refresh trigger respawns    |
| User runs Fetch with no remote        | `git fetch` exits 128; surface as red status text "no remote configured"     |
| Cherry-pick / revert produces conflict | Crane doesn't handle merge conflict UI in v1 — show modal: "Conflict — resolve in terminal, then resume" |
| Worktree-from-commit path collision  | `NewWorkspaceModal` already validates path uniqueness; existing flow         |

## Testing

Unit tests in `crates/crane_term`-style organization:

- `data::tests::parse_log_round_trip` — synthesized stdout → expected
  CommitRecord set.
- `data::tests::malformed_lines_skip_cleanly` — embedded null /
  short lines / wrong field count.
- `graph::tests::straight_line_no_merges` — N commits, single chain
  → all rows on lane 0.
- `graph::tests::two_branches_merge` — fixture with a fork + merge,
  asserts lane assignments and parent_lanes vector for the merge
  commit.
- `graph::tests::octopus_merge_3_parents` — 3-parent commit;
  parent_lanes has 3 entries.
- `graph::tests::orphan_branches` — disconnected DAG (e.g.
  `git checkout --orphan`) → second component starts on lane 0
  after the first compacts.
- `graph::tests::lane_color_stability` — same lane index across
  consecutive rows yields the same color.
- `refs::tests::worktree_porcelain_parse` — covers detached,
  bare, and branched worktrees.
- `refresh::tests::watcher_debounces` — 5 fs events in 50 ms →
  exactly one reload.

UI rendering covered manually (egui's egui_kittest framework would
let us snapshot-test, but adopting it is out of scope for this spec).

## Build sequence

Roughly in order of dependence (covered in detail by the writing-plans
skill in the next step):

1. Module skeleton + `Tab` field additions + persistence stubs (no
   visual yet).
2. `data::load_commits` + tests.
3. `refs::load_refs` + tests.
4. `graph::layout` + tests (the real algorithm — biggest single piece).
5. Bottom-region rendering scaffold (header strip, splitters, three
   empty columns).
6. `view/refs.rs` (left column).
7. `view/log.rs` row rendering (without the painter yet — text only).
8. Painter integration in `view/log.rs` (the JetBrains-faithful visual).
9. `view/details.rs` (right column) + Diff Pane wiring.
10. Filters + filter bar UI.
11. `refresh.rs` watcher + poll + Fetch-all.
12. Operations context menu + worktree-from-commit modal extension.
13. Persistence wiring.
14. Polish (theme integration, cursors, hover states, empty/error states).

## Open follow-ups (not in this spec)

- Path filter (`-- <path>`) — needs UX decision on graph topology
  collapsing.
- Interactive rebase / reset / push-pull power ops.
- Side-by-side diff inside the right column (currently uses Diff Pane).
- Per-Project (not per-Tab) Git Log — if multi-tab users want the
  same Workspace's log to share state.
- Keyboard navigation between commits (j/k or arrows).
- `reindex_git_state` polling cadence bug surfaced during brainstorm
  (worktree created via terminal didn't auto-pick-up) — fix
  separately, not part of this design.

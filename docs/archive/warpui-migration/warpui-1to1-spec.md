# Crane warpui Port — Exhaustive 1:1 Build Spec

> 100% line-by-line audit of the egui Crane (~34,700 lines, 14 subsystems). Every struct/field, UI element (icon/color/dimension), interaction, state, and the concrete warpui port approach + honest port-status %. Companion docs: `warpui-1to1-punchlist.md` (ordered build checklist), `warpui-migration-execution.md` (Phase 0-6 changeset).

## Contents

- App model + state.rs (2822 ln)
- Layout/Node/Pane/Tab + session persistence
- Left Panel (projects tree)
- Right Panel (Changes/Files)
- Center pane view + top bar + status bar + branch picker
- Files Pane / file editor
- Diff / Markdown / Welcome / PDF panes
- Browser pane (wry WKWebView overlay)
- Terminal pane (parity check vs warpui port)
- git.rs (shell-out git API)
- Git Log pane (3-column dock + graph)
- LSP integration
- Modals + toasts
- App shell + infra + theme

---


<!-- ===== app-state ===== -->

## App model + `state.rs` — Build Spec for warpui Port

This file is the **non-visual data backbone** of Crane. It defines the entire project/workspace/tab hierarchy, all modal/picker state, the notification system, and the imperative mutation API (add/remove/reorder/rename projects, dispatch async git ops, drive LSP sync). It renders **no UI directly** — every UI surface (`ui_left`, `ui_right`, `ui_top`, pane views, modals) reads/mutates this `App` struct. For warpui, this is the **state layer**, not a widget. The port approach is: replicate the data model faithfully, then have warpui's render tree read from it and dispatch typed actions back into these methods.

### Core type aliases
```rust
pub type ProjectId   = u64;
pub type WorkspaceId = u64;
pub type TabId       = u64;
```
Monotonic IDs minted from `App::{next_project,next_workspace,next_tab}` (all start at 1). **warpui:** keep `u64` newtype-or-alias IDs; never reuse indices as identity (the tree is reorderable and prunable, so positional identity breaks).

---

### Public structs / enums

#### `PaneNotification` (Debug, Clone)
Toast payload from an OSC 9 / OSC 777 terminal notification.
- `body: String` — notification text
- `urgent: bool` — drives urgent styling + OS-banner fallback
- `project/workspace/tab: {Project,Workspace,Tab}Id`, `pane: u64` — locator quad to jump focus on toast click
- `term_tab: usize` — index into the terminal pane's inner tab list
- `created_at: Instant` — TTL anchor

#### `ATTENTION_PERIOD: f32 = 2.6`
Seconds per breathe-in/out attention pulse cycle (loops forever until tab opened).

#### `AttentionViz` (Clone, Copy, Default)
- `glow: f32` (0..1 pulse intensity), `dot: bool` (persistent unread marker)
- `from_since(Option<Instant>) -> Self`: raised-cosine breathing — `phase = (elapsed/PERIOD)*TAU; glow = (1 - phase.cos())*0.5`. `None` → default (no glow, no dot).
- `animating(Option<Instant>) -> bool`: just `since.is_some()` — caller keeps requesting repaints while true.

**warpui:** Compute `glow` per-frame from `now() - attention_since`; modulate the row's accent alpha by `glow`. Draw a small filled circle (phosphor `CIRCLE` filled, or a painted dot) when `dot`. Keep an animation-active flag so the frame scheduler re-requests paint.

#### `Tab`
- `id: TabId`, `name: String`
- `layout: Layout` — the split tree (defined in `layout.rs`, not here)
- `tint: Option<[u8;3]>` — per-tab accent RGB for sidebar icon+label; `None` → active-tab accent or default fg
- `git_log_visible: bool` (default false; toggled Cmd+9 / top-bar)
- `git_log_state: Option<GitLogState>` — lazy on first show; persisted
- `attention_since: Option<Instant>` — set when a non-active tab's terminal pings; cleared when tab becomes active; **not persisted**

#### `Workspace` (canonical "branch checkout")
- `id`, `name: String` (canonical branch/folder — never UI-mutated except on git rename), `display_name: Option<String>` (user alias)
- `path: PathBuf`, `tabs: Vec<Tab>`, `active_tab: Option<TabId>`, `expanded: bool`
- `git_status: Option<GitStatus>` (hot-path render reads this directly)
- `last_status_refresh: Option<Instant>`, `git_job: Option<JobHandle<Option<GitStatus>>>`
- `tint: Option<[u8;3]>`
- `label() -> String`: `"alias (name)"` when aliased & non-empty, else `name`.

#### `Project`
- `id`, `name`, `path`
- `group_path: Option<PathBuf>`, `group_name: Option<String>` — shared parent for nested-repo grouping (folder header in Left Panel)
- `missing: bool` — folder gone on disk; git/LSP/new-tab no-op; shows "Project Not Found" modal
- `workspaces: Vec<Workspace>`, `expanded: bool`, `last_active_workspace: Option<WorkspaceId>`
- `preferred_location_mode: Option<LocationMode>`, `preferred_custom_path: Option<String>` — remembered new-workspace modal prefs
- `tint: Option<[u8;3]>`
- `files_skip_paths: Vec<PathBuf>` — subdirs to hide in Files tree (already their own Projects)
- `is_loose() -> bool`: exactly one workspace named `"(no git)"` → Left Panel flattens (no workspace row, folder icon not git-branch).

#### `RightTab` { Changes, Files } (Clone, Copy, PartialEq)
Which Right Panel sub-tab is active.

#### `SettingsSection` { Appearance, Editor, Terminal, LanguageServers, Shortcuts, About }
- `ALL: &'static [...]` — render order
- `label() -> &str`: "Appearance", "Editor", "Terminal", "Language Servers", "Keyboard Shortcuts", "About"
- `icon() -> &str` (phosphor regular): Appearance=`PAINT_BRUSH`, Editor=`CODE`, Terminal=`TERMINAL_WINDOW`, LanguageServers=`LIGHTNING`, Shortcuts=`KEYBOARD`, About=`INFO`

**warpui:** Settings nav list = one row per `ALL` entry: phosphor `icon()` glyph (as Text) + `label()`. Selected row → accent bg/border; selected state = `settings_section == this`.

#### `LocationMode` { Global, ProjectLocal, Custom } (+ Debug, Eq)
- `as_str()`/`parse()` for session persistence
- New-workspace path resolution via `NewWorkspaceModal::resolved_parent`:
  - Global → `~/.crane-worktrees/<project_name>`
  - ProjectLocal → `<project_path>/.crane-worktrees`
  - Custom → `shellexpand_home(custom_path)` (expands leading `~/` or `~\`)

#### `NewEntryKind` { File, Folder }
Two-variant enum (not bool) for the Files-Pane inline new-entry editor.

#### `FileOp` (Debug, Clone) — reversible Files-Pane op
- `Move { from: PathBuf, to: PathBuf }` — undo = rename back (refuses if `from` re-occupied)
- `Trash { path: PathBuf }` — undo via `trash` crate restore on Linux/Windows; **macOS no-op** (no programmatic restore)
- `FILE_OP_HISTORY_CAP: usize = 64` — bounded undo stack

#### `GitOpKind` { Commit, CommitAndPush (`#[allow(dead_code)]`), Push, Pull, Fetch }
- `label()`: "Commit" / "Commit & Push" / "Push" / "Pull" / "Fetch"

#### `GitOpStatus` (Clone, Debug; Default = Idle)
- `Idle` | `Running { kind, repo }` | `Done { kind, repo, message }` | `Failed { kind, repo, error }`
- `repo() -> Option<&Path>` — every non-Idle variant carries the worktree path so a failed Push from project A doesn't leak a red pill into project B's view. Owned via `Arc<Mutex<GitOpStatus>>`; worker writes, render loop polls.

#### `PendingNewEntry`
- `parent: PathBuf` (always a dir), `kind: NewEntryKind`, `name: String`, `error: Option<String>`, `focused_once: bool` (first-frame focus latch — **critical**: prevents the TextEdit re-grabbing focus every frame, which would block clicks elsewhere from cancelling).

#### `NewWorkspaceModal`
- `project_id`, `branch: String`, `custom_path: String`, `mode: LocationMode`, `create_new_branch: bool`, `branch_locked: bool` (true when opened from branch picker with an existing branch — hide checkbox, lock text field), `error: Option<String>`
- `resolved_parent(project_path, project_name)` as above.

#### `PendingGoto`
- `server: lsp::ServerKey`, `request_id: i64`, `dispatched_at: Instant` (5s watchdog drops stale requests).

#### `BranchPickerState` (has `Default`)
- `open: bool`, `query: String`, `collapsed: HashSet<String>`, `width: f32 (=420)`, `height: f32 (=360)`, `opened_at: Option<Instant>`, `error: Option<String>`, `loading: bool`
- `job: Option<JobHandle<Vec<(PathBuf, Vec<String>, Vec<String>)>>>` — JobSystem I/O pool, key-deduped
- `repos: Vec<(PathBuf, local_branches, remote_branches)>` (remotes in `remote/branch` form)
- `filter: Option<PathBuf>` — `None` = all-repos aggregate; `Some(root)` = single repo.

#### `TabSwitcherState` (Cmd+~ overlay)
- `entries: Vec<(ProjectId,WorkspaceId,TabId)>` — frozen MRU snapshot (front=most recent)
- `highlight: usize` (wraps), `cmd_was_held: bool` (suppresses stray release-commit on single tap)

#### `PendingDeleteFile { path }`, `PendingRemoveWorktree { project_id, workspace_id, label, path, unpushed_commits, modified_files, has_upstream, is_main }`
Confirmation-modal payloads. `is_main` skips `git worktree remove` (git refuses to remove the primary worktree; only the in-memory entry drops).

---

### `App` (the god-struct)
Field groups (all `pub` unless noted):
- **Tree:** `projects: Vec<Project>`, `active: Option<(ProjectId,WorkspaceId,TabId)>`, `last_workspace: Option<(ProjectId,WorkspaceId)>`
- **Panel visibility:** `show_left/show_right/show_help: bool`, `right_tab: RightTab`, `left_panel_w: f32 (=240)`, `right_panel_w: f32 (=300)`
- **Git/commit UI:** `commit_message: String`, `git_error: Option<String>`, `git_op_status: Arc<Mutex<GitOpStatus>>`
- **Files-Pane state:** `expanded_dirs: HashSet<PathBuf>`, `collapsed_change_dirs: HashSet<String>`, `pending_new_entry`, `selected_file: Option<PathBuf>`, `file_op_history: VecDeque<FileOp>`, `single_click_open: bool`
- **Settings/appearance:** `font_size: f32 (=14)`, `selected_theme: String (="crane-dark")`, `show_settings: bool`, `settings_section`, `custom_mono_font: Option<String>`, `ui_scale: f32 (=1.0)`, `syntax_theme_override: Option<String>`, `editor_word_wrap: bool`, `editor_trim_on_save: bool`
- **Update:** `update_check: UpdateCheck`, `updater: Updater`
- **LSP:** `lsp: LspManager`, `language_configs: LanguageConfigs`, `lsp_install_prompts_disabled: bool`, `pending_gotos: Vec<PendingGoto>`
- **Modals/pickers:** `new_workspace_modal`, `branch_picker: BranchPickerState`, `renaming_tab: Option<(P,W,T,String)>`, `renaming_workspace: Option<(P,W,String)>`, `missing_project_modals: Vec<ProjectId>`, `pending_remove_worktree`, `pending_delete_file`, `pending_close_tab: Option<(P,W,T)>`, `pending_quit_modal: bool`, `confirmed_quit: bool`, `find_in_files: Option<FindInFilesState>`
- **Group state:** `group_tints: HashMap<PathBuf,[u8;3]>`, `group_collapsed: HashSet<PathBuf>`
- **MRU / switcher:** `tab_mru: Vec<(P,W,T)>` (cap 256), `tab_switcher: Option<TabSwitcherState>`
- **Caches/throttles:** `repo_branch_cache: HashMap<PathBuf,(String,Instant)>` (2s TTL), `last_loose_git_probe` (2s), `last_worktree_prune` (3s), `last_cli_agent_poll` (~1s)
- **Background infra (lazy):** `jobs: Option<Arc<JobSystem>>`, `file_watcher: Option<FileWatcher>`, `fs_events: Option<Receiver<ChangeEvent>>`
- **Notifications:** `pending_notifications: VecDeque<PaneNotification>` (cap 64), `active_notification: Option<PaneNotification>`, `window_focused: bool`
- **Misc:** `external_drop_handled: bool`
- **Private counters:** `next_project/next_workspace/next_tab` (accessed via `next_*_id()` getters + `set_id_counters` clamp-max for session restore).

---

### Methods (the imperative API surface warpui must replicate)

**Per-frame ticks (called from main loop):**
- `drain_terminal_notifications()` — walks every project→ws→tab→terminal-pane→inner-tab, `take_notifications()` per terminal, pushes into `pending_notifications` (drop-oldest at cap 64). Sets `tab.attention_since = Some(now)` for non-active source tabs (only if currently `None` — first ping wins).
- `poll_cli_agent_sessions()` — 1 Hz (900ms guard); for each terminal not already flagged, if `foreground_is_cli_agent()` → `enable_full_grid_clear_behavior()` (one-way latch). Skips the `ps` fork once flagged.
- `sync_tab_mru()` — clears active tab's `attention_since`; moves `active` to MRU front (dedup, cap 256).
- `refresh_active_git_status(&wake)` — lazy-inits JobSystem+FileWatcher (watches every non-missing project), drains `fs_events` (invalidates dir_cache parents, force-stales touched workspaces by nulling `last_status_refresh`), then for each workspace: harvest finished `git_job` (`JobOutput::Done|Cancelled`), pull renamed branch forward into `name`, and submit a new `git::status` job when due (active interval 1s / inactive 5s; priority Foreground vs Visible; `Pool::Io`; `JobKey::new(Scope::Workspace(id),"git_status")`).
- `sync_lsp_changes(ctx)` — `lsp.tick`; **active layout only**; `did_open` untracked files, debounced (300ms idle) `did_change` for dirty file tabs.
- `poll_loose_git_init(ctx)` (2s) — if any loose project gained `.git`, `reindex_git_state`.
- `poll_dead_worktrees(ctx)` (3s) — `prune_dead_worktrees` + `scan_new_worktrees`.

**Active accessors:** `active_layout_ref/active_layout(mut)`, `active_workspace_path`, `active_workspace_mut`, `active_tab_ref/mut`, `active_repo_root` (nearest `.git` for active file, else workspace path), `active_repo_branch` (2s cached), `active_project_files_skip`, `active_file_path_str` (private).

**Project lifecycle:** `add_project_from_path` (discovers nested repos via `git::discover_repos(.,5)`; groups them; synthetic loose-files project for non-repo parents with `files_skip_paths`), `add_single_project` (private; `git::list_workspaces` or `(no git)` placeholder; watches via FileWatcher), `reindex_git_state` (prune→scan-roots→discover new sub-projects→refresh worktrees→promote standalone→group→consolidate→rebalance), `remove_project` (unwatch, cancel jobs at Project/Workspace/Tab scope, fix `active`/`last_workspace`, rebalance group: 0 members→drop tints/collapse, 1 member→flatten), `remove_group`, `init_git_for_project`.

**Workspace/Tab ops:** `new_tab_in_active_workspace`, `add_tab_to_loose_project`, `push_tab` (private), `close_active_tab`, `set_active`, `toggle_git_log`, `open_new_workspace_modal` (sanitizes name for path safety; seeds from preferred mode/path), `create_workspace_from_modal` (validates branch, `git::workspace_add`, remembers prefs).

**Files ops:** `open_file_into_active_layout`, `open_external_file` (read-only iff outside workspace), `close_file_tabs_for_path`, `rename_file_tabs_for_path`, `refresh_diff_panes_for_path`, `refresh_diff_panes_after_hunk_stage`, `undo_last_file_op`.

**Git async:** `dispatch_git_op(kind, repo, wake, commit_message)` — sets `Running` immediately (returns early if already Running); short-circuits Push(ahead=0)/Pull(behind=0) via `git::ahead_behind` into `Done` with plain-language message ("Nothing to push…", "Already up to date"); else spawns `std::thread` running git op, writes `Done|Failed`, calls `wake()`.

**Reordering (drag-drop):** `move_in_vec` (private; downward-drag index correction), `root_blocks` (private; maximal contiguous same-`group_path` runs), `move_block` (private), `reorder_root_project`, `reorder_root_group`, `reorder_project_in_block` (sub-projects can't escape their group), `consolidate_groups`, `rebalance_groups` (private; demote groups with ≤1 live member), `reorder_workspace`, `reorder_tab`.

**Notification routing:** `focus_notification_source(n)` (set layout focus + active tab + inner term_tab; silent no-op on stale locators), `notification_source_names(n)` (resolves IDs→display names with `"—"` fallback).

**Misc:** `breadcrumb()` → `"project / workspace / tab"` (ASCII `/` separator — U+203A is tofu; caret would be phosphor `CARET_RIGHT`), `ensure_initial` (empty — first launch is empty state).

---

### warpui port approach (concrete)

1. **State struct:** Port `App` as the central app-state object warpui's frame fn mutably borrows. Keep field-for-field parity (panel widths, font_size, theme strings, all `Option`/`Vec`/`HashSet`/`HashMap`/`VecDeque` collections). The "11 flat fields → grouped struct" refactor (BranchPickerState) is the pattern: group cohesive subsystem state.

2. **No UI here.** Nothing in this file paints. Every visual described elsewhere (sidebar rows, settings nav, toasts, modals) reads these fields. For warpui: the render tree queries `app.projects`, `app.active`, `app.show_left`, etc., and on interaction calls the typed mutation methods (`set_active`, `reorder_root_project`, `dispatch_git_op`, …). Reuse the **hit-Rect-on-top + `dispatch_typed_action` + `ctx.notify`** pattern: a sidebar row's click dispatches e.g. `Action::SetActive(p,w,t)` → handler calls `app.set_active(...)`.

3. **Icons:** Wherever a glyph is named (`SettingsSection::icon`, breadcrumb caret note, attention dot) render via phosphor as **Text**, never Unicode literals.

4. **Recursive tree:** Project→Workspace→Tab→Layout(Node tree) is the recursive structure; the Layout/Node recursion lives in `layout.rs`. Here the tree is `Vec<Project>` with `Vec<Workspace>`/`Vec<Tab>` — render as a flat walk respecting `expanded`/`group_collapsed`/`is_loose()`/`missing`.

5. **Background work:** `JobSystem` + `FileWatcher` + `std::thread` git ops + `Arc<Mutex<GitOpStatus>>` polling. warpui must keep the same "writer thread, poll-on-frame, wake()" pattern (no async runtime). The `wake`/repaint handle is `crate::state::WakeHandle`.

6. **Animation:** `AttentionViz` raised-cosine glow — port directly; drive repaint while `animating()`.

7. **ID discipline:** Monotonic `u64` minted from `next_*`; `set_id_counters` clamps on restore. Never positional.

---

### Honest port-status: **~8%**

This is foundational data-layer code. The structs and IDs are trivially portable, but almost none of it is "done" in warpui terms:
- The struct definitions could be transcribed quickly (~mechanical), but the **behavioral machinery** — nested-repo discovery/grouping/consolidate/rebalance, the two-directional worktree sync, FileWatcher+JobSystem wiring, debounced LSP sync, async git-op dispatch with short-circuits, MRU+tab-switcher, notification drain/route, file-op undo (with platform-split trash), drag-drop block reordering — is deep, stateful, and tightly coupled to sibling crates (`git`, `jobs`, `lsp`, `file_watcher`, `crane_term`, `state::layout`). None of that is reusable from warpui's existing widgets; it must be reimplemented or the whole `App` ported wholesale.
- warpui presumably has no equivalent `App`/`Project`/`Workspace`/`Tab` model yet, no JobSystem, no FileWatcher integration, no LSP manager. Until those land, only the plain enums (`RightTab`, `SettingsSection`, `LocationMode`, `NewEntryKind`, `GitOpKind`) and the value-types (`PaneNotification`, `AttentionViz`, modal payloads) are realistically reusable as-is.

Source file: `/Users/rajpootathar/ideaProjects/crane/src/state/state.rs` (2823 lines). Sibling types referenced but defined elsewhere (must be read for a full port): `crate::state::layout::{Layout, PaneContent, TabKind}`, `crate::git::{GitStatus, WorkspaceInfo}`, `crate::jobs::*`, `crate::lsp::*`, `crate::file_watcher::*`, `crate::git_log::GitLogState`, `crate::update::*`, `crate::modals::find_in_files::FindInFilesState`, `crate::state::WakeHandle`.


---


<!-- ===== layout-session ===== -->

## Layout / Node / Pane / Tab + Session Persistence

This section documents the in-memory layout tree, the pane/tab content model, and the on-disk session + settings + per-project-cache persistence for 1:1 reproduction in warpui. Files covered: `src/state/layout.rs`, `src/state/session.rs`, `src/state/settings.rs`, `src/state/project_cache.rs`.

---

### 1. Public types — `layout.rs`

This module is **pure data + tree algebra**. It renders nothing itself; `pane_view.rs` walks the `Layout` to draw. All UI dimensions/colors live in the renderer, not here — but the data model below dictates exactly what the renderer must support.

#### 1.1 `PaneId = u64`
Monotonic per-`Layout` pane identifier. Allocated from `Layout::next_id`, starts at `1`, never reused within a Tab's lifetime (only grows). Doubles as the `HashMap<PaneId, Pane>` key **and** the leaf payload in the `Node` tree. The `Pane.id` field is redundant with the map key (`#[allow(dead_code)]`) — kept only for session round-trip.

#### 1.2 `enum Dir { Horizontal, Vertical }`
`Copy, PartialEq, Eq`. `Horizontal` = side-by-side split (first = left, second = right). `Vertical` = stacked split (first = top, second = bottom). Serialized to/from the strings `"h"` / `"v"` (see SNode).

#### 1.3 `enum Node` (the Layout tree — recursive)
```
Leaf(PaneId)
Split { direction: Dir, first: Box<Node>, second: Box<Node>, ratio: f32 }
```
- `Clone`. The split tree inside one Tab.
- `ratio` ∈ clamped `[0.05, 0.95]` (enforced in `set_split_ratio`), default `0.5` on every new split. `ratio` is the fraction allotted to `first`; `second` gets `1 - ratio`.
- Splits are always **binary**. A three-pane row is `Split(a, Split(b, c))` — nested, not flat. The renderer must recurse.

#### 1.4 `struct FileTab` (open file inside a Files pane — "File Tab")
Large struct; only `path`/`name`/`content`/`preview` are persisted (via `SFile`), the rest is runtime. Fields and what they hold:
- `path: String` — absolute path on disk (intentionally `String`, not `PathBuf`, to round-trip through session JSON unchanged; ~40 LSP/format/UI callsites depend on `&str`).
- `content: String` — current editor buffer.
- `original_content: String` — content as last read/saved; `dirty()` = `content != original_content`.
- `name: String` — display name (basename).
- `last_lsp_content: String` + `last_lsp_sent_at: Option<Instant>` — debounce state for `textDocument/didChange`.
- `preview_mode: bool` — Markdown files: render HTML preview vs source editor (eye toggle).
- `pending_cursor: Option<(u32,u32)>` — goto-definition target (line,char) applied next render.
- `image_texture: Option<egui::TextureHandle>` — lazy GPU texture for image files.
- `find_query: Option<String>` — None = find bar closed; Some = open+filtered.
- `find_scroll_to_line: Option<u32>` — scroll target after next/prev jump; cleared after scroll.
- `disk_mtime: Option<SystemTime>` + `external_change: bool` — external-edit detection; banner Reload/Overwrite/Cancel.
- `last_cursor_idx: usize` — primary cursor char index (transient, for Ln/Col status strip).
- `line_changes: Option<crate::git::FileDiff>` + `line_changes_key: u64` — cached per-line git change classification, keyed by content hash, for gutter/scrollbar markers.
- `goto_line_active: bool` + `goto_line_input: String` — Ctrl+G modal.
- `replace_query: String` + `show_replace: bool` — replace bar.
- `selection_info: Option<(usize,usize)>` — (selected_chars, selected_lines) for status strip.
- `save_error: Option<String>` — last save error toast.
- `preview: bool` — **preview tab** (single-click open, auto-replaced by next single-click, promotes to permanent on edit/re-open). Distinct from `preview_mode`.
- `read_only: bool` — opened from outside the workspace; locked until explicit unlock.
- `pdf_state: Option<Box<PdfTabState>>` — lazy PDF viewer page cache.
- `fn dirty(&self) -> bool`.

#### 1.5 `enum DiffMode { Unified, SideBySide }`
`Copy, PartialEq, Eq`. Default `Unified`.

#### 1.6 `struct DiffTabData` (diff inside a Files pane — **never persisted**)
- `title, left_path, right_path, left_text, right_text: String`.
- `error: Option<String>`, `image_texture: Option<TextureHandle>`.
- `diff_mode: DiffMode`.
- `repo_path: Option<String>` — absolute git root, used for `git apply --cached` per-hunk staging.
- `pending_hunk_stage: bool` — set by diff view when a hunk is staged; main loop refreshes.
- `sbs_h_scroll_left / sbs_h_scroll_right: f32` — side-by-side horizontal scroll offsets.
- Cache machinery: `computed: Option<Arc<DiffComputed>>`, `compute_job: Option<JobHandle<DiffComputed>>`, and three u64 versions `inputs_version` / `computed_for_version` / `job_for_version`. `inputs_version` starts at `1` on construction so the first render sees a fresh version vs `computed_for_version: 0`.
- `fn invalidate(&mut self)` — `inputs_version = inputs_version.wrapping_add(1)`. **Every mutator of left/right text/path/repo_path MUST call it** or the diff renders stale.
- `fn reload_left_text(&mut self)` — resolves repo root from `repo_path` (NOT `right_path` — `right_path` is repo-relative and canonicalising against CWD fails when Crane wasn't launched from the repo). Handles `left_path` prefixes `"staged:"` (→ `git::staged_content` falling back to `head_content`) and `"HEAD:"` (→ `git::head_content`), then `invalidate()`.

#### 1.7 `enum TabKind { File(FileTab), Diff(DiffTabData) }`
Helpers: `name() -> &str`, `is_dirty()` (Diff always false), `is_read_only()` (Diff always false), `as_file()/as_file_mut()/as_diff_mut()`.

#### 1.8 `struct FilesPane`
- `tabs: Vec<TabKind>`, `active: usize`.
- `input_buf: String`, `error: Option<String>` — dead, kept for session schema compat.
- `pending_close: Option<usize>` — index of tab awaiting close confirmation (× or middle-click on dirty tab); modal. Not persisted.
- `empty()` — all-default.
- `open(path, content, name, preview, read_only)` — if a tab with same `path` exists, focus it and clear its `preview` flag (promote); else push a new `FileTab` (reads `disk_mtime` via `fs::metadata`), set active to last.
- `open_diff(title, left_path, right_path, left_text, right_text, repo_path)` — dedup by `(left_path,right_path)`: if found, refresh fields + `invalidate()` + focus; else push new `DiffTabData` (with `inputs_version:1`, others 0).
- `close(idx)` — if a Diff tab with an in-flight `compute_job`, `cancel_token().cancel()` first; `remove(idx)`; fix `active` (clamp down, or shift left if `active > idx`).
- `close_preview_tab()` — retain non-preview file tabs; clamp active.

#### 1.9 `struct MarkdownPane`
`path, content, input_buf, error` — only `path` persisted; content re-read on restore.

#### 1.10 Browser types
- `struct BrowserTab { id: u32, url: String, input_buf: String, title: String }` — `id` keys the native webview host, starts at 1, stable for tab lifetime.
- `struct BrowserPane { tabs: Vec<BrowserTab>, active: usize, next_tab_id: u32 }`.
  - `new_with(url, input_buf)` — single tab id=1, `next_tab_id=2`.
  - `new_tab()` / `new_tab_with(url)` — allocates `next_tab_id` (then ++); empty url → `input_buf = "https://"`; pushes + sets active.
  - `close_tab(idx) -> Option<u32>` — refuses if only one tab remains (`len() <= 1`); returns removed id; fixes active.
  - `active_tab_mut()`.

#### 1.11 `struct WelcomePane` (`#[derive(Default)]`, zero-size)
Landing-page content. Stateless. View renders welcome buttons + shortcut cheat-sheet and bubbles a `PaneAction` to replace itself in-slot with Terminal/Browser/Files.

#### 1.12 Terminal types
- `struct TerminalTab { terminal: Terminal, name: Option<String> }` — `name` = user display-name override; `None` → cwd-basename default. `new(terminal)`.
- `struct TerminalPane { tabs: Vec<TerminalTab>, active: usize, renaming: Option<(usize,String)> }` — `renaming` = inline rename buffer for tab `idx` (double-click; Enter commits, Esc cancels; not persisted).
  - `single(term)`, `active_terminal()/_mut()`, `add(term)` (sets active to last).
  - `close(idx)` — `remove`; cancels/shifts in-flight `renaming` (if `rid==idx` → None; if `rid>idx` → `rid -= 1`); fixes `active`.

#### 1.13 `enum PaneContent`
```
Terminal(TerminalPane) | Files(FilesPane) | Markdown(MarkdownPane) | Browser(BrowserPane) | Welcome(WelcomePane)
```
`kind_label() -> &'static str`: `"Terminal" | "Files" | "Markdown" | "Browser" | "New Tab"` (Welcome → "New Tab").

#### 1.14 `struct Pane { id: PaneId, title: String, content: PaneContent }`
`title` is shown in the pane header. Default new-pane title = `"{kind_label} {id}"` (e.g. `"Terminal 3"`); root/welcome title = `"New Tab"`.

#### 1.15 `struct Layout`
- `root: Option<Node>` — None = empty Tab.
- `panes: HashMap<PaneId, Pane>`.
- `focus: Option<PaneId>` — the focused leaf (2px accent border in renderer).
- `cwd: PathBuf` — working dir new terminals spawn in.
- `next_id: PaneId` (private).
- `maximized: Option<PaneId>` — when Some, that pane renders full-size over the layout. **Runtime-only, never serialized.**

Key methods (all pure tree ops, ported to warpui as the same recursive functions):
- `new(cwd)`.
- `ensure_initial_welcome()` — if `root.is_none()`, `add_root(Welcome, "New Tab")`. Every fresh Tab/Workspace/Project gets a Welcome pane, **not** an auto-spawned shell.
- `replace_focused_content(content, title)` — in-place swap of the focused pane's content+title (Welcome buttons → Terminal/Browser without reflowing the tree).
- `add_root(content, title)` (private) — alloc id, insert, `root = Leaf(id)`, `focus = Some(id)`.
- `add_pane(content, split: Option<Dir>)` — alloc id, title `"{kind_label} {id}"`; if no root → root becomes the leaf; if `Some(root)+Some(focus)+Some(dir)` → `split_at(root, focus, id, dir)`; else split at `first_leaf` horizontally; set focus to new pane.
- `split_focused_with_terminal(ctx, dir)` — spawn `Terminal::spawn(ctx,80,24,Some(cwd))`; on Ok, `add_pane(Terminal(single), Some(dir))`.
- `next_pane_id()` / `set_next_pane_id(id)` (latter takes `id.max(next_id)`).
- `open_diff_in_files_pane(...)` / `open_file_in_files_pane(...)` — reuse the **first** existing `Files` pane if any (and focus it); else `add_pane(Files, Horizontal)`. For files: if `preview`, `close_preview_tab()` first. For diff into a new pane, sets the pane title to the diff title.
- `close_focused()` — remove focused pane from map; `remove_node(root, focus)`; new focus = returned sibling or `first_leaf`.
- `focus_next()` / `focus_prev()` — over `collect_leaves` (in-order DFS), wrap-around. Prev: idx 0 → last.
- `set_split_ratio(path: &[usize], ratio)` — walk `path` (0=first,1=second) and set, clamped `[0.05,0.95]`.
- `swap_panes(a,b)` — swap two leaf ids in place (drag-drop reorder).
- `dock_pane(src, target, edge: DockEdge)` — remove `src`, then `wrap_target` it adjacent to `target` on the edge; focus = src.

#### 1.16 `enum DockEdge { Left, Right, Top, Bottom, Center }`
`Copy, PartialEq, Eq`. Maps to split direction + src-first:
- Left → (Horizontal, src first), Right → (Horizontal, src second), Top → (Vertical, src first), Bottom → (Vertical, src second), Center → no-op (returns the target leaf unchanged — drop-on-center = cancel).

#### 1.17 Free tree functions (port verbatim — recursive)
- `wrap_target(node, target, src, edge)` — finds the `Leaf(target)`, wraps it in a new `Split{ratio:0.5}` with `src` on the requested edge; recurses into splits.
- `swap_leaves(&mut node, a, b)` — swaps ids a↔b wherever they appear.
- `split_at(node, target, new_pane, dir)` — wraps `Leaf(target)` into `Split{dir, first:target, second:new_pane, ratio:0.5}`; recurse.
- `prune_leaf` (pub) = `remove_node`.
- `remove_node(node, target) -> (Option<Node>, Option<PaneId>)` — removes target leaf; when a split loses a child, the surviving sibling **collapses up** (replaces the split); returns the new sibling's `first_leaf` as suggested focus.
- `contains`, `first_leaf` (leftmost), `collect_leaves` (in-order DFS), `set_ratio`.
- 9 unit tests pin: split-on-leaf, missing-target no-op, remove-promotes-sibling, remove-last→None, first_leaf leftmost, collect order, contains, set_ratio. **Port these tests.**

---

### 2. Session persistence — `session.rs`

On disk at `~/.crane/session.json` (atomic write with `.json.bak` fallback). Holds the **per-session, project-tree-shaped** state. User preferences moved to `settings.json` (see §3) but `Session` still carries duplicate fields for older installs.

#### 2.1 `struct Session` (`#[derive(Serialize,Deserialize)]`, `version: u32 = 1`)
Fields: `projects: Vec<SProject>`, `active: Option<(ProjectId,WorkspaceId,TabId)>` (cursor), `last_workspace: Option<(ProjectId,WorkspaceId)>`, `show_left/show_right: bool`, `right_tab: String` ("changes"/"files"), `font_size: f32`, `collapsed_change_dirs: Vec<String>`, `expanded_dirs: Vec<PathBuf>`, `commit_message: String`, `next_project/next_workspace/next_tab` id counters. `#[serde(default)]` extras: `update_prompts: HashMap<String,PromptState>`, `selected_theme` (default `"crane-dark"`), `custom_mono_font: Option<String>`, `ui_scale` (default 1.0), `syntax_theme_override`, `left_panel_w` (default 240), `right_panel_w` (default 300), `language_configs`, `group_tints: Vec<(PathBuf,[u8;3])>` (Vec because JSON maps need string keys), `group_collapsed: Vec<PathBuf>` (only collapsed groups stored — empty = all expanded).

#### 2.2 `struct SProject`
`id, name, path: PathBuf, expanded: bool, workspaces: Vec<SWorkspace>`. `#[serde(default)]`: `last_active_workspace: Option<WorkspaceId>`, `preferred_location_mode: Option<String>`, `preferred_custom_path: Option<String>`, `group_path: Option<PathBuf>`, `group_name: Option<String>`, `tint: Option<[u8;3]>`, `files_skip_paths: Option<Vec<PathBuf>>`.

#### 2.3 `struct SWorkspace`
`id, name, path: PathBuf, expanded, active_tab: Option<TabId>, tabs: Vec<STab>`. `#[serde(default)]`: `display_name: Option<String>`, `tint: Option<[u8;3]>`.

#### 2.4 `struct STab`
`id, name, layout: Option<SNode>, focus: Option<PaneId>, next_pane_id: PaneId, panes: Vec<SPane>`. `#[serde(default)]`: `tint: Option<[u8;3]>`, `git_log_visible: bool`, `git_log_state: Option<SGitLogState>`.

#### 2.5 `struct SGitLogState` (`Default`)
`height` (def 320), `col_refs_width` (def 220), `col_details_width` (def 360), `maximized: bool`, `selected_commit: Option<String>`, `selected_file: Option<String>`, `col_refs_collapsed/col_details_collapsed: bool`, `col_log_meta_width` (def 220). All `#[serde(default*)]`.

#### 2.6 `enum SNode` — serialized form of `Node`
`Leaf(PaneId)` | `Split { direction: String ("h"/"v"), first: Box<SNode>, second: Box<SNode>, ratio: f32 }`. `from_node`/`into_node` convert Dir↔string ("v"→Vertical, else Horizontal).

#### 2.7 `struct SPane` + `enum SPaneContent`
`SPane { id, title, content: SPaneContent }`. `SPaneContent` variants:
- `Terminal { cwd: PathBuf (legacy, skip if empty), history_text (legacy, skip if empty), history_b64 (legacy, skip if empty), tabs: Vec<STerminalTab> (skip if empty), active: usize }`.
- `Files { files: Vec<SFile>, active: usize }`.
- `Markdown { path: String }`.
- `Browser { url: String (legacy, skip if empty), tabs: Vec<SBrowserTab>, active }`.
- `Welcome`.
- `SBrowserTab { url }`, `STerminalTab { cwd: PathBuf, history_text: String (default), name: String (default; empty = no override) }`, `SFile { path, name }`.

#### 2.8 Save / load
- `session_file()` → `~/.crane/session.json`.
- `load()` — tries `session.json`, then `session.json.bak`; returns first that parses. A corrupt live file falls back to last-known-good (without the fallback, a partial write wiped every project). On parse failure, eprintln + try next.
- `SAVE_DEBOUNCE = 2s`.

#### 2.9 `Session::from_app(app) -> Session`
Walks `app.projects → workspaces → tabs`, building S-structs. `right_tab` maps `RightTab::Changes→"changes"`, `Files→"files"`. Pulls id counters via `app.next_*_id()`. `files_skip_paths` → `None` when empty.

`STab::from_tab(t)`: panes = `t.layout.panes` mapped via `SPane::from_pane`; layout = `root.map(SNode::from_node)`; carries focus, next_pane_id, tint, git_log_visible, and git_log_state (snapshotting only the persistable fields; `selected_file: PathBuf → String` via `to_string_lossy`).

`SPane::from_pane(id, p)`:
- **Terminal** → snapshots each tab as `STerminalTab { cwd: terminal.cwd, history_text: terminal.snapshot_ansi(), name }`. **Critical: persists the rendered-grid ANSI snapshot, not the raw PTY byte log** — raw bytes don't survive width changes (prompts use absolute cursor escapes); ANSI form preserves every cell's color/SGR. Writes empty legacy fields.
- **Files** → persists only `TabKind::File` tabs (diffs are ephemeral); decrements `adjusted_active` for each skipped diff tab that precedes the active index.
- **Markdown** → just `path`.
- **Browser** → `tabs: Vec<SBrowserTab>{url}`, active; empty legacy url.
- **Welcome** → `Welcome`.

#### 2.10 `Session::restore(self, ctx) -> App`
- `App::new()`, then apply show_left/right, right_tab, font_size, commit_message, collapsed_change_dirs (→HashSet), expanded_dirs (→HashSet).
- For each `SProject`: rebuild workspaces → tabs (`STab::into_tab`). `missing = !sp.path.exists()` — **missing projects still load** (keep ids/expanded) but git/LSP/terminal/worktree actions skip them; pushes `sp.id` to `app.missing_project_modals`.
- **Cursor sanitization**: if `active (pid,wid,tid)` references a now-nonexistent project/workspace/tab → set `active=None` (dangling ids used to cause blank panels/panics). Same for `last_workspace (pid,wid)`.
- Restore id counters, update_check, theme, custom_mono_font, `ui_scale.clamp(0.75,1.5)`, syntax_theme_override, `left_panel_w.clamp(180,600)`, `right_panel_w.clamp(200,700)`, language_configs, group_tints (→map), group_collapsed (→set).
- `app.reindex_git_state(ctx)` — re-probe disk for repos/worktrees added outside Crane.

`STab::into_tab(ctx, cwd)`: builds `Layout::new(cwd)`, inserts every pane (`SPane::into_pane`), sets `root` (from SNode), `focus`, `next_pane_id`; constructs the full runtime `GitLogState` (mostly defaults: `last_poll: Instant::now()`, atomics, None job/watcher), `attention_since: None`.

`SPane::into_pane(ctx, cwd)`:
- **Terminal** — new `tabs` vec wins; else fall back to legacy single tab (history_text, or base64-decoded+ANSI-stripped history_b64). For each saved tab: spawn cwd = saved cwd or fall-back `cwd`; if `history_text` empty → `Terminal::spawn`, else `Terminal::spawn_with_text_history(...)`. Filter spawn failures. If **all** spawns fail → fall back to `Files(FilesPane::empty())`. Else `TerminalPane{ tabs, active: active.min(len-1), renaming: None }`.
- **Files** — for each `SFile`, `fs::read_to_string` (default on error) + `fs::metadata` mtime → fresh `FileTab` (preview=false, read_only=false, all runtime fields default). `active.min(len-1)` or 0 if empty.
- **Markdown** — read content, build `MarkdownPane`.
- **Browser** — empty tabs → `new_with(url,url)`; else `new_with(tabs[0].url)` + `new_tab_with` for each extra; `active.min(len-1)`.
- **Welcome** — `WelcomePane`.

#### 2.11 base64 + strip_ansi helpers (legacy read path)
Stdlib-only base64 enc/dec (alphabet `A-Za-z0-9+/`). `strip_ansi(&[u8])` strips CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ESC\`), DCS/SOS/PM/APC (`ESC P|X|^|_ … ESC\`), two-byte ESC, and bell `0x07`. **Keeps CR and LF** (stripping CR collapses lines). Only needed for migrating ancient `history_b64` sessions.

---

### 3. User settings — `settings.rs`

On disk at `~/.crane/settings.json`. **User-level preferences that follow the user across projects** — split out of session.json (old installs migrated on first read).

#### 3.1 `struct Settings` (`Clone, Serialize, Deserialize`)
All `#[serde(default*)]`:
- `selected_theme: String` (def `"crane-dark"`), `syntax_theme_override: Option<String>`.
- `font_size: f32` (def 14.0), `custom_mono_font: Option<String>`, `ui_scale: f32` (def 1.0).
- `left_panel_w: f32` (def 240), `right_panel_w: f32` (def 300).
- `editor_word_wrap: bool`, `editor_trim_on_save: bool`, `single_click_open: bool`.
- `show_left/show_right: bool` (def true via `t()`).
- `right_tab_files: bool` — serialized as bool (not the enum) to stay stable across enum changes; true → `RightTab::Files`.
- `language_configs: LanguageConfigs`, `lsp_install_prompts_disabled: bool`.

#### 3.2 Methods
- `settings_file()` → `~/.crane/settings.json`. `Default` impl mirrors the default fns.
- `load()` — read + parse, else `Default`.
- `save()` — `create_dir_all(parent)`; `to_vec_pretty`; write to `.json.tmp` then `crate::util::replace_file(tmp, path)` (atomic).
- `from_app(app)` — snapshot the preference slice; `right_tab_files = matches!(app.right_tab, RightTab::Files)`.
- `apply(self, app)` — push back into App, with clamps: `font_size.clamp(9.0,28.0)`, `ui_scale.clamp(0.75,1.5)`, `left_panel_w.clamp(180,600)`, `right_panel_w.clamp(200,700)`. Call after `App::new`.

Note: there is **field overlap** between `Session` and `Settings` (theme, font, panel widths, ui_scale, show_left/right, right_tab, language_configs). Settings is now the source of truth; Session retains them for backward-compat read of old files.

---

### 4. Per-project cache — `project_cache.rs`

Per-project cache dir under `~/.crane/projects/<slug>/`. Generic hook for project-keyed persistence (branch-picker collapse, commit-tree index, file index, search caches, per-repo LSP artifacts).
- `root()` → `~/.crane/projects`.
- `slug_for(path)` → `"{sanitized-basename}-{8-hex-digest-of-abs-path}"` (e.g. `api-1a2b3c4d`). Sanitizer keeps `[A-Za-z0-9_-]`, replaces the rest with `-`. Digest = `crate::util::hash64(path) as u32`, formatted `:08x`. Disambiguates two projects with the same name in different dirs.
- `ensure_project_dir(path)` (`#[allow(dead_code)]`) — `root()/slug`, `create_dir_all`.
- `file(path, name)` (`#[allow(dead_code)]`) — path to a named file in the project cache dir; caller does its own IO.
Both consumers are currently dead code (planned: commit-tree index, fuzzy-finder recents).

---

### 5. warpui port approach

- **Node tree / Layout algebra** → port the recursive tree functions verbatim (`split_at`, `remove_node`, `wrap_target`, `swap_leaves`, `set_ratio`, `collect_leaves`, `first_leaf`, `contains`) — they're pure and UI-agnostic; carry the 9 unit tests across unchanged. `PaneId = u64`, `next_id` monotonic from 1.
- **Rendering the tree** (in `pane_view`, not this module): recurse `Node`; at each `Split` carve the rect by `ratio` (clamped `[0.05,0.95]`) along `direction`, draw a draggable splitter between halves → on drag, compute the new ratio and call `set_split_ratio(path, ratio)`. At each `Leaf`, draw the pane header (`Pane.title`) + body (dispatch on `PaneContent`). `maximized` short-circuits: render only that pane full-rect.
- **Focus border** = 2px accent on `focus` leaf, subtle border elsewhere (per CLAUDE UI rules).
- **Tab strips** (Files/Browser/Terminal multi-tab): each chip is a hit-Rect-on-top + `dispatch_typed_action`; the active chip uses the selected token, inactive use subtle; the `×` close button (phosphor `X`, `min_size ≥ 22×22`, pinned right via `Layout::right_to_left`) dispatches a close action → `FilesPane::close` / `TerminalPane::close` / `BrowserPane::close_tab`; `+` (phosphor `PLUS`) appends a tab; double-click a terminal chip enters `renaming`; dirty file tab shows a dot (use phosphor, never a unicode glyph) and routes close through `pending_close` confirm modal.
- **DockEdge drag-drop** → on pane-header drag, show 5 drop zones (Left/Right/Top/Bottom/Center rects), and on drop call `dock_pane(src, target, edge)`; Center = cancel. `swap_panes` for simple reorder.
- **Welcome pane** → stateless view; buttons (Terminal / Browser / Files) dispatch a typed action that calls `replace_focused_content(...)`; show the keyboard cheat-sheet from CLAUDE's canonical shortcut list.
- **Persistence** → mirror the S-struct shapes exactly (same JSON field names, same `#[serde(default)]`, same `version:1`) so existing `~/.crane/session.json` files load unchanged. Keep the `.json.bak` fallback in `load()` and atomic tmp-replace in save. **Terminal history must be the rendered ANSI snapshot, not raw PTY bytes.** Keep the legacy `history_b64` + `strip_ansi` read path. Diff tabs are never persisted (prune + adjust active). Split `Settings` from `Session`, with Settings authoritative for prefs and the same clamps on `apply`. Keep cursor sanitization on restore (drop dangling `active`/`last_workspace`). Reproduce `project_cache::slug_for` (basename + 8-hex of abs-path hash) byte-for-byte if cache files are to interoperate.

---

### 6. Port status

- **Layout/Node tree algebra**: ~0% ported in warpui as a faithful copy (this is the spec). The algorithms are trivial-to-port and fully test-covered — estimate **low effort, high confidence**.
- **Pane/Tab content model**: data model fully specified above; the heavy runtime fields (`Terminal`, `DiffComputed`, `PdfTabState`, `egui::TextureHandle`, `JobHandle`) are dependencies on other areas (terminal, diff, pdf, jobs) — those gate completion, not this module.
- **Session/Settings persistence**: schema fully specified; straightforward serde port. The only subtle pieces are (a) ANSI terminal snapshot (depends on the terminal crate's `snapshot_ansi` / `spawn_with_text_history`), (b) the legacy base64 path, (c) cursor sanitization, (d) git-log-state runtime reconstruction (depends on `git_log` area).
- **Honest current warpui port status for this AREA: ~10%.** The pure-data structs and tree algebra are clearly defined and easy to land, but nothing here is independently renderable — every variant of `PaneContent` and most of `restore()` depends on sibling areas (terminal, diff, files, browser, git_log, jobs, lsp) being ported first. The data/algebra layer is the realistic near-term deliverable; full session round-trip is blocked on those dependencies.


---


<!-- ===== left-projects ===== -->

## Left Panel — Projects Tree

This documents `src/ui/projects.rs` (the Left Panel projects tree) and its rendering primitives in `src/ui/util.rs`. The Left Panel is a vertically-scrolling, recursively-indented tree of Projects → Workspaces (branches) → Tabs, with a folder-group layer above standalone Projects, a sticky footer "Add Project" button, full drag-drop reordering, inline rename, per-row tint context menus, git change badges, and pending-notification "attention" glow/dot animation.

---

### 1. Public / module structs & enums

These live in `projects.rs` (all `enum`/`struct` here are private to the module; only `render` is `pub`). They are still load-bearing for the port because they encode the drag-drop scoping model.

#### `TreeDrag` (private enum — drag payload)
Stored in egui's drag-and-drop state via `dnd_set_drag_payload`. Identifies what is being dragged and its scope:
- `Project { id: u64, in_group: Option<u64> }` — `in_group = None` ⇒ standalone (root-level) project; `Some(anchor_id)` ⇒ sub-project inside a folder group, where `anchor_id` is the id of the first Project of the containing contiguous block. Sub-projects can only drop on same-block rows (matched by anchor) so they can't escape their group.
- `Group { anchor: u64 }` — a folder-group header dragged as a root-level unit; identified by its block anchor (first Project's id), because two distinct blocks may share a `group_path` until consolidation merges them.
- `Workspace { project_id: u64, id: u64 }` — a branch row.
- `Tab { project_id: u64, workspace_id: u64, id: u64 }` — a tab row.

#### `DropScope` (private enum — per-row hit scope)
Tags a row's hit region so the post-walk dispatcher knows which rows are siblings of the in-flight drag:
- `Root` — top-level row (standalone project or folder header). Root drags match here.
- `InBlock { anchor: u64 }` — sub-project inside a folder group, anchored at first Project of block. Sub-project drags only match same-anchor zones.
- `Workspace { project_id: u64 }`
- `Tab { project_id: u64, workspace_id: u64 }`

#### `DropZone` (private struct)
`{ rect: Rect, scope: DropScope }` — one per visible row, collected during the walk; drop dispatch runs once after the walk.

#### Module constants
- `HEADER: Color32 = rgb(140,146,162)` — the "PROJECTS" section title color (hardcoded, not a theme token).
- `PROJECT_TINT_PALETTE: &[(&str,[u8;3])]` — 8 hand-picked tints for the right-click "Highlight color" picker, in this exact order:
  - Red `[239,83,80]`, Orange `[255,152,0]`, Yellow `[255,202,40]`, Green `[102,187,106]`, Teal `[38,166,154]`, Blue `[66,165,245]`, Purple `[171,71,188]`, Pink `[236,64,122]`.

#### External state structs referenced (from `crate::state`)
- `App` — owns `projects: Vec<Project>`, `active: Option<(u64,u64,u64)>` (project,workspace,tab), `last_workspace: Option<(u64,u64)>`, `group_collapsed: HashSet<PathBuf>`, `group_tints: HashMap<PathBuf,[u8;3]>`, `renaming_tab: Option<(u64,u64,u64,String)>`, `renaming_workspace: Option<(u64,u64,String)>`, `pending_remove_worktree: Option<PendingRemoveWorktree>`, `pending_close_tab: Option<(u64,u64,u64)>`. Methods called: `add_project_from_path`, `set_active`, `new_tab_in_active_workspace`, `open_new_workspace_modal`, `add_tab_to_loose_project`, `init_git_for_project`, `remove_project`, `remove_group`, `reorder_root_project`, `reorder_root_group`, `reorder_project_in_block`, `reorder_workspace`, `reorder_tab`.
- `Project` — `id`, `name`, `path`, `expanded: bool`, `missing: bool`, `tint: Option<[u8;3]>`, `group_path: Option<PathBuf>`, `group_name: Option<String>`, `workspaces: Vec<Workspace>`; methods `is_loose()` (no `.git`), `is_loose` flips between FOLDER/CUBE icon.
- `Workspace` (the branch struct, code-named `Worktree`) — `id`, `name`, `path`, `expanded: bool`, `display_name: Option<String>`, `tint: Option<[u8;3]>`, `active_tab: Option<u64>`, `git_status: Option<GitStatus>` (fields `added`, `deleted`, `changes`), `tabs: Vec<Tab>`; methods `label()` (display_name or branch/folder name), `attention_since` via tabs.
- `Tab` — `id`, `name`, `tint: Option<[u8;3]>`, `attention_since: Option<Instant>`.
- `AttentionViz { glow: f32 (0..1), dot: bool }` — with assoc fns `from_since(Option<Instant>) -> AttentionViz` and `animating(Option<Instant>) -> bool`.
- `PendingRemoveWorktree { project_id, workspace_id, label, path, unpushed_commits, modified_files, has_upstream, is_main }`.

#### `util.rs` public API
- Tokens: `TEXT_BTN_H=28.0`, `BTN_TEXT_SIZE=12.5`, `ICON_BTN_SIZE=(28,24)`, `ROW_H=26.0`, `INDENT_W=10.0`, `CHEVRON_W=14.0`.
- Color accessors (read live theme each call): `text()`, `muted()`, `header_fg()`, `accent()`, `row_hover()`, `row_active()` (accent at alpha 55), `trailing_hover()` (`surface_alt`).
- `CheckState` enum: `Unchecked | Checked | Indeterminate` (Changes tree only — not used by Left Panel, but `RowConfig.checkbox` is always `None` here).
- `RowConfig<'a>` — see field table below.
- `RowResult { rect, main_clicked, double_clicked, hovered, checkbox_clicked, response }`.
- Functions: `icon_button`, `full_width_primary_button`, `section_header`, `draw_row`, `draw_trailing`.

---

### 2. `render` — top-level layout

`render(ui, app, ctx)`:
- `full = available_rect_before_wrap()`. Splits into `scroll_rect` (top, full height minus 44px footer) and `footer_rect` (bottom **44px**).
- Scroll area child UI clipped to `scroll_rect`; calls `render_tree`.
- Footer child UI clipped to `footer_rect`:
  - 1px top divider line at `footer_rect.min.y`, color hardcoded `rgb(36,40,52)`.
  - `add_space(8.0)`, then a horizontal row: `add_space(8)`, item_spacing.x = 0, a `full_width_primary_button` with icon `FOLDER_PLUS`, label `"Add Project…"`, tooltip `"Choose a folder"`, then `add_space(8)`.
  - Click ⇒ `rfd::FileDialog` "Choose project folder" `pick_folder()`; on pick ⇒ `app.add_project_from_path(path, ctx)`.

`full_width_primary_button`: filled `egui::Button`, text `"{icon}  {label}"` at size 12.5, `min_size = (available_width, 28.0)`.

---

### 3. `render_tree` — the tree walk

Header: `add_space(10)`, horizontal `add_space(12)` + label `"PROJECTS"` size **10.5**, color `HEADER` (rgb 140,146,162), `.strong()`. Then `add_space(4)`.

A `ScrollArea::vertical().id_salt("left_projects").auto_shrink([false,false])` wraps the walk.

The walk collects a large set of deferred mutation `Option`s (e.g. `set_active`, `toggle_project`, `toggle_worktree`, `close_tab`, `new_tab_for_worktree`, `new_workspace_for_project`, `add_tab_to_loose`, `init_git_for`, `remove_project`, `remove_worktree`, tint setters, `toggle_group_collapsed`, reorder targets, rename start/commit/cancel). **All mutation is deferred to after the walk** to avoid borrowing `app` mutably while iterating — this is the canonical "collect intent → apply after" pattern.

`pointer_pos = ctx.input(|i| i.pointer.hover_pos())` — read once; used for drop-line preview because egui suppresses per-widget `contains_pointer` during an active captured drag.

`group_counts: HashMap<PathBuf,usize>` precomputed so a single-member group's project still shows its individual "Remove", but multi-member groups hide it (atomic-group removal rule).

Rename state is snapshotted into local buffers (`rename_buffer`, `rename_wt_buffer`) so the walk only needs `&app`; flushed back after.

`pulse_animating` bool — set true if any visible row's `AttentionViz::animating(...)`; if true, `ctx.request_repaint()` at the end to keep the glow breathing.

#### Row order and depth model
For each `project` in `app.projects` (a flat Vec; grouping is by contiguous `group_path` runs):
- **Folder-group header** rendered when entering a new `group_path` (`project.group_path != last_group`). Depth 0.
- **Project row**: depth `1` if in a group else `0`. Icon `CUBE` (git) or `FOLDER` (loose). `tree_guides = in_group`.
- **Workspace (branch) rows** (only if `project.expanded`): depth `2` (in group) / `1`. Skipped entirely for loose projects (placeholder workspace hidden — tabs flatten up one level). Icon `GIT_BRANCH`.
- **Tab rows** (only if `wt.expanded || is_loose`): depth `3`/`2` normally, or `2`/`1` for loose. Icon `TERMINAL_WINDOW`.

Indent math (in `draw_row`): `cursor_x = rect.min.x + 12 + depth*INDENT_W(10)`; then chevron `CHEVRON_W(14)+2`; leading icon adds 18.

---

### 4. `draw_row` — the row primitive (the heart of the spec)

Allocates `(width, ROW_H=26)` with `Sense::click_and_drag()`. Paints via `painter_at(rect)`. Z-order of paint:

1. **Background**:
   - If `is_active`: `rect.shrink2((4,1))`, corner-radius 4, fill `row_active()` (accent @ alpha 55).
   - Else if hovering: `hover_t = animate_bool_with_time(id."row_hover_t", hovered, 0.09)` (90ms ease). Fill `row_hover()` token scaled by `hover_t` on its alpha. Same shrink/radius.
2. **Active bar**: if `active_bar`, a 2px-wide vertical accent bar at `x=rect.min.x+4`, `y=min.y+3`, height `rect.h-6`, radius 1, color `accent()`.
3. **Attention glow**: if `attention.glow > 0.01`, accent wash over `rect.shrink2((4,1))`, alpha `110*glow` clamped 0..255. Painted over hover/active.
4. **Tree guides**: if `tree_guides && depth>0`, for each ancestor depth `d` draw a 1px vertical line at `x = rect.min.x + 12 + d*INDENT_W + CHEVRON_W/2`, full row height, color = theme `divider`.
5. **Chevron** (if `expanded: Some`): cross-fade `CARET_RIGHT`↔`CARET_DOWN` via `animate_bool_with_time(id."row_chev_t", expanded, 0.11)` (110ms). Drawn centered at `cursor_x + CHEVRON_W/2`, font 12 Proportional, color `text()` if active else `muted()`, alpha-faded across the swap.
6. **Checkbox** (Left Panel: always `None`, skip).
7. **Leading icon** (if set): centered at `cursor_x+8`, font **13.5** Proportional, color = `leading_color` or `muted()`. `cursor_x += 18`.
8. **Label**: `Align2::LEFT_CENTER` at `cursor_x`, font **13.0** Proportional, color `label_color` or `text()`. Right-edge reserve: badge (16px if dot `(0,0)`, else 64px, else 0) + `trailing_count*22`. Label is **clipped** (not ellipsized) to `label_right = rect.max.x - 8 - badge_reserve - trailing_reserve`.
9. **Badge** (git change counts, `(added, deleted, add_color, del_color)`): drawn right-to-left from `rect.max.x - 10 - trailing_count*22`. If `(0,0)` ⇒ a 3px filled circle "dirty dot" in `add_color` at `bx-4`. Else `-{deleted}` then `+{added}` text at font **10.5** in del/add colors, each preceded by `galley.width + 4` left-shift.
10. **Unread dot**: if `attention.dot && !hovered`, a 3.5px filled `accent()` circle at `x=rect.max.x - 12 - trailing_count*22`. Hidden on hover so it doesn't fight trailing buttons.
11. Hover ⇒ `set_cursor_icon(PointingHand)`.

`main_clicked = response.clicked() && !checkbox_clicked`; `double_clicked` similarly gated.

#### `RowConfig` fields (build spec)
| field | type | meaning |
|---|---|---|
| `depth` | usize | indent level |
| `expanded` | Option<bool> | Some ⇒ draw chevron; None ⇒ leaf (tabs) |
| `leading` | Option<&str> | phosphor glyph |
| `leading_color` | Option<Color32> | tint icon |
| `label` | &str | row text |
| `label_color` | Option<Color32> | tint text (used for project/branch/tab tint) |
| `is_active` | bool | active-row background |
| `active_bar` | bool | left accent bar |
| `badge` | Option<(usize,usize,Color32,Color32)> | git +/- counts |
| `trailing_count` | usize | reserve right space for N trailing buttons |
| `tree_guides` | bool | vertical ancestor guides |
| `checkbox` | Option<CheckState> | unused in Left Panel |
| `attention` | AttentionViz | glow + dot |

#### `draw_trailing` — hover action buttons
`draw_trailing(ui, rect, row_hovered, &[(icon, tip, slot)]) -> [bool;4]`. Right-aligned from `rect.max.x - 8`, each button **20×20**, 2px gap, laid out in reverse so slot order reads left→right. Each registers a hit `ui.interact(btn_rect, id, Sense::click())` **always** (so the click works on the first hover frame) with `id = ui.id().with(("trailing", rect.min.x as i32, rect.min.y as i32, slot))`. Paints only when `row_hovered || resp.hovered()`: hovered ⇒ `trailing_hover()` fill, radius 4, PointingHand cursor; icon centered, font **13**, color `text()`. Returns per-slot click bools.

---

### 5. Per-row UI elements, interactions, states

#### Folder-group header (depth 0)
- **draw_row**: `expanded = Some(!group_is_collapsed)`, leading `FOLDER`, leading+label color = group tint (`app.group_tints`) or `muted()`, `is_active=false`, `active_bar=false`, `trailing_count=0`, `tree_guides=false`. Attention: only when collapsed, aggregated over all member projects' tabs (`project_attention`).
- **Click** (`main_clicked`) ⇒ `toggle_group_collapsed` (toggles membership in `app.group_collapsed`).
- **Drag**: payload `TreeDrag::Group { anchor=project.id }`. Drop-line preview painted when a root-scoped drag (`Group` or `Project{in_group:None}`) hovers and isn't the same anchor; above/below by pointer vs `rect.center().y`.
- **Right-click context menu**: "Highlight color" label (size 11 muted) + a horizontal row of 8 frameless `FOLDER`-glyph swatch buttons (size 14, min 22×22, colored by palette, `on_hover_text(label)`) ⇒ `set_group_tint=Some((gp,Some(rgb)))`; then `ARROW_COUNTER_CLOCKWISE  Default color` ⇒ `set_group_tint=Some((gp,None))`; separator; `X  Remove folder group` ⇒ `remove_group_pending`.
- When collapsed, member project rows are `continue`d (skipped).

#### Project row (depth 0 or 1)
- **Icon**: `FOLDER` if loose else `CUBE`. Color = project tint or `accent()`.
- **Label color**: project tint (else default text).
- **trailing_count**: 2 if `allow_individual_remove` else 1. `allow_individual_remove = !in_multi_group || project.missing`.
- **Trailing buttons** (`draw_trailing`): `[PLUS, plus_tooltip, 0]` always; plus `[X, "Remove project", 1]` when allowed. `plus_tooltip` = "New tab" (loose) or "New worktree" (git).
- **Interactions** (priority order):
  - trailing[0] (PLUS): loose ⇒ `add_tab_to_loose`; git ⇒ `new_workspace_for_project` (opens New Workspace modal).
  - trailing[1] (X): `remove_project`.
  - else `main_clicked` ⇒ `toggle_project` (flip `expanded`).
- **Drag**: payload `Project { id, in_group = current_block_anchor (if in group) else None }`. Drop-line preview when scope matches (other project same `in_group`, or a `Group` drag onto a root row).
- **Right-click menu**: `FOLDER_OPEN  Reveal in File Manager` (calls `reveal_in_file_manager` → `open`/`xdg-open`/`explorer` on canonicalized path); `COPY  Copy Path` (`ctx.copy_text`); separator; "Highlight color" + 8 `CUBE`-glyph swatches ⇒ `set_tint`; `ARROW_COUNTER_CLOCKWISE  Default color` ⇒ `set_tint None`; if loose: separator + `GIT_BRANCH  Initialize Git` ⇒ `init_git_for`; if `allow_individual_remove`: separator + `X  Remove Project` ⇒ `remove_project`.

#### Workspace / branch row (depth 1 or 2) — only when `project.expanded` and not loose
- **Icon**: `GIT_BRANCH`. Color priority: explicit tint > (accent if active) > none.
- **Label**: `wt.label()`; color = workspace tint.
- **is_active / active_bar**: true when `app.active`'s workspace == this id.
- **badge**: from `git_status` — if `added>0||deleted>0` ⇒ `(added, deleted, diff_added, diff_deleted)`; else if `changes` non-empty ⇒ `(0,0,…)` dirty dot; else None.
- **Attention**: only when `!wt.expanded`, aggregated over its tabs.
- **trailing_count**: 1 → `[PLUS, "New tab", 0]`.
- **Interactions** (priority): trailing[0] PLUS ⇒ `new_tab_for_worktree` (sets active workspace, `new_tab_in_active_workspace`); else `double_clicked` ⇒ `start_rename_wt` (seeded with `display_name`); else `main_clicked` ⇒ `toggle_worktree` (flip expanded; if it has `active_tab`, set `app.active`).
- **Drag**: payload `Workspace { project_id, id }`. Drop-line when same-project workspace drag.
- **Right-click menu**: `PENCIL_SIMPLE  Rename` ⇒ start_rename_wt; `FOLDER_OPEN  Reveal in File Manager`; `COPY  Copy Path`; separator; "Highlight color" + 8 `GIT_BRANCH`-glyph swatches ⇒ `set_workspace_tint`; `ARROW_COUNTER_CLOCKWISE  Default color`; separator; `X  Remove Worktree` ⇒ `remove_worktree`.
- **Rename mode** (`wt_renaming`): instead of `draw_row`, allocate `(avail_width, 26)` hover-sized, child UI shrunk by `(32,2)`, a singleline `TextEdit` id `("rename_wt", wt.id)`, hint = `wt.name`, full-width. Focus requested **once** (memory flag `("rename_wt_focus_done", wt.id)`). Enter ⇒ commit; Esc ⇒ cancel; `lost_focus` ⇒ commit if non-empty else cancel. Empty trimmed ⇒ clears alias (`display_name=None`).

#### Tab row (leaf; depth 2/3 or 1/2 loose) — only when `wt.expanded || is_loose`
- **Icon**: `TERMINAL_WINDOW`. Color priority: tint > (accent if active) > none.
- **Label**: `tab.name`; color = tab tint.
- **is_active / active_bar**: true when `app.active` matches workspace+tab.
- **expanded**: None (no chevron).
- **Attention**: carries its own `tab.attention_since` directly (leaf).
- **trailing_count**: 1 → `[X, "Close tab", 0]`.
- **Interactions** (priority): trailing[0] X OR `middle_clicked` ⇒ `close_tab` (staged into `pending_close_tab`, routed through confirm modal); else `double_clicked` ⇒ `start_rename`; else `main_clicked` ⇒ `set_active`.
- **Keyboard**: when active and no rename in progress, `F2` or `Cmd/⌘+R` (not Shift) ⇒ `start_rename`.
- **Drag**: payload `Tab { project_id, workspace_id, id }`. Drop-line when same project+workspace tab drag.
- **Right-click menu**: `PENCIL_SIMPLE  Rename`; separator; "Highlight color" + 8 `TERMINAL_WINDOW`-glyph swatches ⇒ `set_tab_tint`; `ARROW_COUNTER_CLOCKWISE  Default color`.
- **Rename mode** (`is_renaming`): same inline `TextEdit` pattern as workspace, id `("rename_tab", tab.id)`, no hint, focus-once memory `("rename_tab_focus_done", tab.id)`. Enter/Esc/lost_focus semantics identical; commit writes `tab.name`.

---

### 6. Drag-drop dispatch (global, post-walk)

Per-row, only a **drop-line preview** is painted during drag (via `pointer_pos` + `DragAndDrop::payload::<TreeDrag>` peek + `paint_drop_line` — a 2px accent line inset 6px horizontally at the row's top or bottom edge depending on pointer vs center).

The actual drop is dispatched **once** after the walk:
1. Read `release_pos` from `pointer.any_released() ? interact_pos()`.
2. **Peek** `DragAndDrop::payload::<TreeDrag>` first (return early if absent) — so `take_payload` doesn't steal a non-tree drag (e.g. pane drag's `DragPayload`).
3. `take_payload::<TreeDrag>`, filter `drop_zones` to scope-matching candidates (Project/None↔Root, Group↔Root, Project/Some(anchor)↔InBlock same anchor, Workspace↔same project, Tab↔same project+workspace).
4. Require `release_pos.y ∈ [first.min.y - 8, last.max.y + 8]`.
5. `new_index = count of candidate row centers with center.y ≤ release_pos.y`.
6. Dispatch to `reorder_root_project` / `reorder_root_group` / `reorder_project_in_block` / `reorder_workspace` / `reorder_tab`.

---

### 7. warpui port approach

This area maps cleanly onto the established warpui port idioms:

- **Recursive tree walk** → keep the same flat-`Vec<Project>` + contiguous-`group_path` model; emit rows in the same order. Each row is one widget allocation of fixed height (`ROW_H=26`). Depth → left indent `12 + depth*10`.
- **Row = `draw_row` analog**: implement as a single allocated `Rect` with a click+drag sense, then paint layers in the exact z-order above. Reuse the **hit-Rect-on-top + `dispatch_typed_action` + `ctx.notify`** idiom: each row's primary click and each trailing button maps to a typed action (`ToggleProject(id)`, `SetActive(p,w,t)`, `CloseTab(...)`, `NewWorkspace(p)`, etc.). The deferred-mutation `Option` collection here becomes a list of dispatched actions — no need to mutate state mid-walk.
- **Icons = phosphor `Text`**: every glyph (`FOLDER`, `CUBE`, `GIT_BRANCH`, `TERMINAL_WINDOW`, `CARET_RIGHT`/`CARET_DOWN`, `PLUS`, `X`, `FOLDER_PLUS`, `FOLDER_OPEN`, `COPY`, `PENCIL_SIMPLE`, `ARROW_COUNTER_CLOCKWISE`) drawn as a phosphor `Text` at the documented font sizes (13.5 leading, 13 label, 12 chevron, 13 trailing, 14 swatch).
- **Animations**: hover fade (90ms), chevron cross-fade (110ms), attention glow breathing, checkbox pop — all `animate_bool_with_time`-style eased toggles keyed by a per-widget id. Port via the same animated-bool helper keyed by row id. Keep `request_repaint` while any attention burst is animating.
- **Colors = theme tokens**: `text`, `text_muted`, `text_header`, `accent`, `row_hover`, `surface_alt` (trailing hover), `divider` (tree guides), `diff_added`/`diff_deleted` (badges). Note two **hardcoded** colors to preserve: section title `rgb(140,146,162)` and footer divider `rgb(36,40,52)`. `row_active` = accent @ alpha 55.
- **Context menus**: right-click on the row's response → menu with the documented items; the tint swatch row is 8 frameless colored-glyph buttons. Dispatch tint as a typed action.
- **Inline rename**: a singleline text field overlaying the row rect, focus-once via a memory flag, with Enter/Esc/blur commit/cancel — direct port.
- **Drag-drop**: payload-typed (`TreeDrag`), scoped candidate filtering, drop-line preview, post-frame index computation. Port the **peek-before-take** discipline so it coexists with the pane drag system.
- **Footer**: fixed 44px bottom strip with a full-width primary button + native folder picker.

---

### 8. Honest port-status

**~20%.** The shared row primitive shape (fixed-height row, indent, leading icon, label, hover/active background, trailing hover buttons) and basic project/branch/tab nesting are the kind of scaffolding likely stubbed in warpui, but the full feature surface of this area is largely unported:

- Folder-group layer (group headers, group collapse, group tints, atomic-group removal, contiguous-block anchoring) — **not ported**.
- Full drag-drop with scoped candidate filtering, drop-line preview, peek-before-take coexistence, and index-by-center computation — **not ported** (the most complex piece).
- Per-row tint context menus (4 variants with glyph-swatch rows) — **not ported**.
- Inline rename (workspace alias + tab) with focus-once + F2/Cmd+R chord — **not ported**.
- Attention glow/dot animation with collapsed-row aggregation — **not ported**.
- Git change badges (+N/-M, dirty dot) wired to `git_status` — **not ported**.
- Loose-project flattening (placeholder workspace hidden, tabs shifted up one indent, Initialize Git) — **not ported**.
- Confirm-modal routing for tab close and worktree removal (`pending_*`) — **partially**, depends on modal infra.
- Footer "Add Project" with native folder picker — **likely present in some form**.

Estimate reflects that the visual row scaffold may exist but essentially all of the Left Panel's distinguishing interaction logic (groups, drag-drop, tints, rename, attention, badges) still needs to be built.


---


<!-- ===== right-explorer ===== -->

## Right Panel — Changes / Files (`src/ui/explorer.rs`)

The Right Panel is one of the three top-level regions. It is a single column rendered by `explorer::render(ui, app)` that owns a 2-tab strip ("Changes" / "Files") plus the body for whichever tab is active. Changes shows the active Workspace's git working tree with stage/unstage/commit/push/pull/fetch. Files shows the on-disk filesystem tree of the Workspace root with full create/move/copy/trash/drag-drop. Both reuse the shared `draw_row` widget from `src/ui/util.rs`.

### 1. Backing data types (port these structs/enums verbatim)

**`RightTab`** (`state/state.rs:193`) — `enum { Changes, Files }`. Stored on `App.right_tab`. Selects the body.

**`git::ChangeStatus`** (`git.rs:45`) — `enum { Added, Modified, Deleted, Renamed, Untracked }`. Drives the per-file glyph + color.

**`git::FileChange`** (`git.rs:53`):
- `path: String` — for renames this is the NEW path (tree groups/sorts on it).
- `old_path: Option<String>` — source side of a rename (only set when staged status is `Renamed`).
- `status: ChangeStatus` — combined status.
- `has_staged: bool` / `has_unstaged: bool` — index vs worktree presence.
- `staged_status: Option<ChangeStatus>` / `unstaged_status: Option<ChangeStatus>` — per-side statuses.

**`git::GitStatus`** (`git.rs:73`): `branch: String`, `changes: Vec<FileChange>`, `added: usize`, `deleted: usize`, `ahead_behind: Option<AheadBehind>`. **`AheadBehind`** (`git.rs:669`) has `ahead`/`behind` counts. Held per-Workspace on `Worktree.git_status: Option<GitStatus>`.

**`GitOpKind`** (`state/state.rs:296`) — `enum { Commit, CommitAndPush (dead_code, reserved for ⌘⇧↩), Push, Pull, Fetch }`. `.label()` → `"Commit"`/`"Commit & Push"`/`"Push"`/`"Pull"`/`"Fetch"`.

**`GitOpStatus`** (`state/state.rs:330`) — async op state, lives behind `App.git_op_status: Arc<Mutex<GitOpStatus>>`:
- `Idle`
- `Running { kind, repo: PathBuf }`
- `Done { kind, repo, message: String }`
- `Failed { kind, repo, error: String }`
- `.repo() -> Option<&Path>` returns the worktree path for every non-Idle variant. **Critical scoping rule:** all UI reads compare `op_status.repo() == active repo_path`; a status belonging to another repo is ignored so a Push failure in project A never bleeds into project B's footer.

**`NewEntryKind`** (`state/state.rs:269`) — `enum { File, Folder }`.

**`PendingNewEntry`** (`state/state.rs:381`) — inline new-entry editor state: `parent: PathBuf`, `kind: NewEntryKind`, `name: String`, `error: Option<String>`, `focused_once: bool` (first-frame focus latch; without it the TextEdit steals focus every frame and blocks click-away cancel). Held on `App.pending_new_entry: Option<PendingNewEntry>`.

**`FileOp`** (`state/state.rs:278`) — `enum { Move { from, to }, Trash { path } }`, pushed onto `App.file_op_history: VecDeque<FileOp>` (LIFO undo, cap `FILE_OP_HISTORY_CAP = 64`).

**`PendingDeleteFile`** (`state/state.rs:489`) — `{ path: PathBuf }`, set on `App.pending_delete_file` to trigger the confirm-delete modal.

**`FsDropOp`** (local to explorer.rs:638) — resolved drag-drop op: `{ src: PathBuf, dst_dir: PathBuf, copy: bool }`. Built once on pointer release, consumed at top of next `render_files`.

**`DirNode`** (local, explorer.rs:735) — recursive change-tree node: `dirs: BTreeMap<String, DirNode>` (sorted), `files: Vec<(String, FileChange)>`. Built by `build_tree` from the flat `Vec<FileChange>`.

**Relevant `App` fields read/written here:** `right_tab`, `commit_message: String`, `git_error: Option<String>`, `git_op_status`, `collapsed_change_dirs: HashSet<String>`, `expanded_dirs: HashSet<PathBuf>`, `selected_file: Option<PathBuf>`, `single_click_open: bool`, `pending_new_entry`, `pending_delete_file`, `file_op_history`, `external_drop_handled: bool`. Helper methods: `active_workspace_path()`, `active_workspace_mut()`, `active_layout()`, `active_project_files_skip()`, `dispatch_git_op(kind, repo, wake, msg)`, `open_file_into_active_layout(...)`, `rename_file_tabs_for_path(src,dst)`, `is_loose()` (Project without `.git`).

### 2. Shared row widget — `draw_row` (`util.rs:157`)

Both tabs render every line through `draw_row(ui, RowConfig)` returning `RowResult`. Port this once; it is the spine of the whole panel.

**`RowConfig`** fields: `depth: usize`, `expanded: Option<bool>` (Some → render animated caret; None → no chevron), `leading: Option<&str>` (phosphor glyph or status letter), `leading_color: Option<Color32>`, `label: &str`, `label_color: Option<Color32>`, `is_active: bool`, `active_bar: bool` (2px left accent bar), `badge: Option<(added, deleted, add_color, del_color)>`, `trailing_count: usize`, `tree_guides: bool`, `checkbox: Option<CheckState>`, `attention: AttentionViz`.

**`CheckState`** (`util.rs:107`) — `enum { Checked, Indeterminate, Unchecked }`.

**`RowResult`**: `rect: Rect`, `main_clicked: bool` (click NOT on checkbox), `double_clicked: bool`, `hovered: bool`, `checkbox_clicked: bool`, `response: Response` (sensed `click_and_drag`).

**Layout / dimensions:** row height `ROW_H = 26.0`, full available width. Cursor starts at `rect.min.x + 12.0 + depth*INDENT_W (10.0)`. Order: chevron (`CHEVRON_W = 14.0` + 2) → checkbox (18×18 box, +20 advance) → leading icon (font 13.5, +18 advance) → label (font 13.0, LEFT_CENTER, clipped, not elided, reserving badge 16/64 + trailing 22·n on the right). Active rows fill `rect.shrink2(4,1)` rounded 4 with `row_active()`; hover fills with `row_hover()` faded by a 90ms `animate_bool_with_time` ease. Cursor → `PointingHand` on hover.

**Chevron:** cross-fades `CARET_RIGHT`↔`CARET_DOWN` over 110ms (`util.rs:222`), color `text()` if active else `muted()`, font 12.0.

**Checkbox glyphs:** Checked = `CHECK_SQUARE` in `accent()` with a 130ms "pop" scale (sin bump to ~1.12×) crossfading from `SQUARE`; Indeterminate = `SQUARE` (accent 0.6α) + `MINUS` (accent); Unchecked = `SQUARE` in `muted()`. Checkbox has its own 80ms hover tint (`trailing_hover()`) and is hit-tested via `rect_contains_pointer` + `pointer.primary_clicked()` so it steals the click from the row (then `main_clicked = response.clicked() && !checkbox_clicked`).

**Badge** (Changes uses None here, but supported): `+N` / `-M` right-aligned in add/del colors (font 10.5); `(0,0,…)` renders a 3px filled dirty-dot.

**`AttentionViz`** (`state/state.rs:39`): `{ glow: f32, dot: bool }`. `glow>0.01` washes the row with accent at `110*glow` alpha (breathing pulse); `dot` paints a 3.5px accent dot at the right edge (hidden while hovered). Not used by explorer rows (always `Default`), but port for parity since draw_row is shared.

**Color tokens** (`util.rs`, all theme-derived): `text()`, `muted()`, `accent()`, `row_hover()`, `row_active()`, `trailing_hover()`, `header_fg()`. Plus `theme::current()` accessors: `.diff_added()`, `.diff_modified()`, `.diff_deleted()`, `.sidebar_bg`, `.divider`, `.accent`, `.text`, `.extreme_bg_color` (egui visual).

### 3. Tab strip (top of panel) — `render` + `tab_chip`

- **Strip rect:** full width × `STRIP_H = ui::top::TOPBAR_H (34.0)` — must equal the Main Panel top bar so the two strips align horizontally (a 40px strip floated 6px high in a prior bug; do not deviate).
- **Bottom divider:** full-width 1px line at `strip_rect.max.y`, hard-coded `Color32::from_rgb(36,40,52)`.
- **Inner ui:** `strip_rect.shrink2(10,4)`, `left_to_right(Center)`.
- **Chips:** `tab_chip("Changes", …)`, 4px gap, `tab_chip("Files", …)`. Each chip is a frameless `egui::Button` (all inactive/hovered/active bg fills forced TRANSPARENT, no stroke), text `RichText size 12.5`. Color: active → `text()`, inactive → `muted()`, disabled → `muted().linear_multiply(0.6)`. `min_size (0, 26)`.
- **Active underline:** 2px `accent()` line segment from `rect.min.x+6` to `rect.max.x-6` at the chip's bottom (drawn only when active && !disabled).
- **Loose-project rule:** if active Project `.is_loose()` (no `.git`), the Changes chip is disabled with tooltip `"No git in this project"`; and if `right_tab == Changes` while loose, it auto-switches to `Files`.
- Click on enabled chip runs the `on_click` closure (`app.right_tab = …`). After strip, `ui.add_space(2.0)`, then dispatch to `render_changes` / `render_files`.

### 4. Changes tab — `render_changes` (explorer.rs:216)

Empty states (via `dim_row`: 6px space + 12px indent + `muted()` size-11.5 label): `"No active worktree"` (no active path), `"(not a git repo)"` (no git_status), `"working tree clean"` (changes empty).

**Branch toolbar** (horizontal, 4px top/bottom space):
- 10px indent, then `RichText("{GIT_BRANCH}  {branch}")` color `text()`, size 12, strong.
- If `ahead_behind`: `ARROW_UP {ahead}` and/or `ARROW_DOWN {behind}` in `muted()` size 11 (each only when >0).
- Right side (`right_to_left(Center)`, 8px pad), insertion-reverse so visual order is **Fetch, Pull, Push**:
  - **Fetch** — `toolbar_button(ARROW_COUNTER_CLOCKWISE, "Fetch")` → `dispatch_git_op(Fetch, repo, wake, None)`.
  - **Pull** — `ARROW_DOWN`, tooltip "Pull" → `dispatch_git_op(Pull,…)`.
  - **Push** — `ARROW_UP`, tooltip "Push" → `dispatch_git_op(Push,…)`.

**`toolbar_button`** (explorer.rs:41): `egui::Button` with `RichText size 13`, `min_size (28,22)`, on_hover_text tooltip. Enabled = `!any_running || running`. While `running`: glyph swapped to `ARROW_COUNTER_CLOCKWISE` (placeholder "spinner" — phosphor has no notch glyph in the set) and a 1px `accent()` rounded-4 stroke is painted around `resp.rect`. `any_op_running` (= status matches repo && Running) disables all buttons so a double-click can't queue a competing op. While any op runs, `request_repaint_after(150ms)` to animate.

**Changes tree** (scroll area + pinned footer):
- Footer height = `128.0 + (40.0 if git_error else 0.0)`, pinned to bottom; scroll area gets the rect above. ScrollArea `id_salt("right_changes")`, `auto_shrink([false,false])`, rendered into a child ui with explicit `set_clip_rect`.
- `build_tree` → `DirNode`; `render_change_tree` → recursive `render_change_node`.
- **Directory row:** `draw_row` with `expanded: Some(!collapsed)`, `leading: FOLDER` (muted), `label_color: muted()`, `checkbox: Some(tri-state)`. Tri-state from `dir_staged_state(child)`: all files fully-staged → Checked; some staged → Indeterminate; none → Unchecked. `checkbox_clicked` → collect every descendant path (`collect_paths`) and push to `stage_paths` (or `unstage_paths` if all_staged). `main_clicked` → toggle `toggle_dir` (collapse/expand via `collapsed_change_dirs`).
- **File row:** leading is a **status letter** not an icon — `A`/`M`/`D`/`R`/`?` colored by `status_color` (Added/Untracked→diff_added, Modified/Renamed→diff_modified, Deleted→diff_deleted). Label = file name, or `"{old_name} -> {new_name}"` when `old_path` is set. Checkbox: `has_staged && !has_unstaged` → Checked; both → Indeterminate; else Unchecked. `checkbox_clicked` toggles stage/unstage of that single path. `main_clicked` → `open_diff = path` (opens HEAD↔worktree diff in Files Pane).
- **File row context menu** (`row.response.context_menu`): `PLUS Stage` (if unstaged), `MINUS Unstage` (if staged), `GIT_DIFF Open Diff`, `FILE Open as File`, separator, `COPY Copy Path` (→ `ctx.copy_text`).

**Footer** (commit area):
- Painted `sidebar_bg` fill + 1px top `divider` line.
- Multiline `TextEdit` bound to `app.commit_message`, hint `"Commit message"` (or `"Stage files to commit"` if nothing staged), `desired_rows(2)`, full width, font 12.5 Proportional. `Cmd+Enter` while focused + `can_commit` + not running → keyboard commit.
- 8px space, then **primary Commit button**: full-width × 30, rounded 6, `accent` fill (hover ×1.15 gamma, active ×0.9), white bold text size 13. Label = `"{CHECK}  Commit to {branch}"`, or `"{ARROW_COUNTER_CLOCKWISE}  Committing…"` while a Commit/CommitAndPush op runs. Enabled = `has_staged && has_message && !any_op_running`. Click → `dispatch_git_op(Commit, repo, wake, Some(msg))` then clears `commit_message`.
- **Status pill** (only when status matches repo): Running → `"{kind.label()}…"` muted italic size 11; Done → `"{kind.label()}: {message}"` in `diff_added()` size 11; Failed → `render_op_error`. If Idle and a legacy `git_error` exists → red (`diff_deleted()`) size 11.

**`render_op_error`** (explorer.rs:650): horizontal_wrapped, `"{op} failed:"` (red, strong, size 11) + first non-empty error line (red size 11). If multi-line, a frameless chevron button (`CARET_UP`/`CARET_DOWN`, muted, size 11, min 16×16, PointingHand on hover) toggles expansion; state stored in `ctx.data` temp keyed `("crane.git_op_error_expanded", repo)`. Expanded → `Frame` filled `extreme_bg_color`, inner margin (8,6), rounded 4, full error in red monospace size 10.5.

**Post-render side effects:** apply `toggle_dir`, `stage_paths`/`unstage_paths` (call `git::stage`/`unstage`, set `git_error` on failure else `force_status_refresh`), `open_diff` → `open_file_diff`, `open_file` → read file + `open_file_into_active_layout`, commit dispatch. On any `GitOpStatus::Done` → `force_status_refresh` (clears `last_status_refresh` to force a re-poll).

### 5. Files tab — `render_files` (explorer.rs:990)

Empty state: `"No active worktree"`. Builds `git_status_map: HashMap<String,(ChangeStatus,bool,bool)>` (rel-path → status/staged/unstaged) so file rows can show git colors and dirs can show "has descendant changes."

ScrollArea `id_salt("right_files")`, `auto_shrink([false,false])`. Recursion via `render_fs_dir` (depth cap 64). Listing comes from `crate::dir_cache::global().entries(path)` (one stat/dir/frame, self-invalidating on mtime). Skips `.git`, `target`, `node_modules`, `.DS_Store`, and `active_project_files_skip()` paths (nested repos exposed as their own Project).

**FS row** (`draw_row`):
- Dir: `expanded: Some(is_expanded)`, leading `FOLDER`, color `diff_modified()` if any descendant has changes else `muted()`.
- File with git status: leading = **status letter** (`status_glyph`) colored `status_color`, label_color same.
- Plain file: leading `FILE` muted, default label color.
- `is_active = is_selected` (matches `app.selected_file`).

**FS row interactions:**
- `main_clicked` on dir → toggle `expanded_dirs`. On file → set `selected_file`; if `single_click_open`, also open as **preview** (`opened_preview = true`).
- `double_clicked` on file → open as **non-preview** (pinned) tab.
- **Drag source:** `row.response.dragged()` → `dnd_set_drag_payload(entry_path)` (any row, file or folder).
- **Drop target resolution:** drop on folder row → into that folder; drop on file row → into the file's PARENT dir (Finder/VS Code behavior). While a drag is in-flight and the pointer is over a *valid* target row, paint accent highlight: `rect.shrink(1)` filled accent at alpha 80 (copy/Alt) or 60, + 1px accent stroke, and set cursor `Copy` (Alt) or `Grabbing`. Validity = not same, not into-self, not into-descendant, not same-parent. On `dnd_release_payload` with valid target → build `FsDropOp { src, dst_dir, copy: alt_held }`.
- **Context menu:** `FILE Open` (files), `GIT_DIFF Open Diff` (files with unstaged changes only), `FILE New File…`, `FOLDER_PLUS New Folder…`, separator, `FOLDER_OPEN Reveal in File Manager` (`open -R` mac / `xdg-open` linux / `explorer /select` win), `COPY Copy Path`, separator, `TRASH Move to Trash` (→ `pending_delete_file`).
- New File/Folder land in the dir itself (folder row) or the file's parent (file row).

**Empty-space sink** (below entries): `allocate_exact_size` of remaining height with `Sense::click()`; its context menu offers `FILE New File…` / `FOLDER_PLUS New Folder…` rooted at the Workspace root.

**Pending new-entry editor** — `render_pending_editor_row` (explorer.rs:1698): rendered in the matching dir only (the `&mut pending` is split so child recursions don't see it). A horizontal row indented `(depth+1)*14`, leading glyph `FILE`/`FOLDER` muted, then a singleline `TextEdit` (id keyed by depth, hint `"filename.ext"`/`"folder-name"`, full width). `request_focus()` once (gated by `focused_once`). `Escape` → cancel; `lost_focus() + Enter` → commit (or cancel if name empty); `lost_focus()` with empty name → cancel (JetBrains parity). Error (if any) renders below at `indent+18` in size 10.5 `rgb(220,100,100)`.

**`try_commit_pending`** (explorer.rs:1760): validates name (rejects `/ \ . ..`, rejects existing path → sets `error`), then `File::create` / `create_dir`. Success → expand parent, clear pending. Failure → set error + reset `focused_once` for re-focus.

**Other side effects:** `delete_request` → `pending_delete_file`; `drop_request` → `copy_into`/`move_path` then invalidate dir_cache for both endpoints + `force_status_refresh` + repaint; `toggled` → toggle `expanded_dirs`; `opened` → `open_file_into_active_layout`; `open_diff` → `open_file_diff`; external Finder drops (`ctx.input.raw.dropped_files`) copied into root (refuses folder-into-self, sets `external_drop_handled`); `new_entry` → expand parent + set `pending_new_entry`.

**In-flight drag chip** (explorer.rs:1183): an `Area` (Order::Tooltip, non-interactable) at `pointer + (14,14)` showing a `Frame` filled `extreme_bg_color` with 1px accent stroke, rounded 5, margin (8,5): glyph (`FOLDER`/`FILE`) + name in `text` color size 11.5, plus a `COPY` pill (accent fill, luminance-picked legible text — gray-20 if accent is light, else white) when Alt held. Requests repaint each frame while dragging.

**FS mutation helpers** (port as-is): `copy_into` (Finder-style ` (n)` dedupe, refuses copy-into-self), `move_path` (`fs::rename`, refuses overwrite, updates selected_file + renames open File Tabs, pushes `FileOp::Move` undo), `copy_dir_recursive` (canonicalize guard against copy-into-descendant, depth cap 32, skips symlinks), `dst_inside_src` guard, `push_file_op`.

### 6. Helper functions reference

- `status_color(ChangeStatus) -> Color32` (explorer.rs:16) — maps to diff_added/modified/deleted tokens.
- `status_glyph(ChangeStatus) -> &str` (explorer.rs:27) — `A/M/D/R/U` letters.
- `dim_row(ui, text)` — empty/placeholder row.
- `dir_staged_state(&DirNode) -> (all_staged, any_staged)`.
- `collect_paths(&DirNode, &mut Vec<String>)` — flatten subtree paths.
- `open_file_diff(app, repo, rel)` — reads HEAD content + worktree, opens diff pane (`open_diff_in_files_pane`) with title `"diff: {filename}"`, left label `"HEAD:{rel}"`.
- `force_status_refresh(app)` — nulls `last_status_refresh` to force re-poll.
- `reveal_in_file_manager(path)` — platform reveal.

### 7. Concrete warpui port approach

- **Rows:** port `draw_row` once as a Rect-allocating widget (`allocate_exact_size(width, 26)`, `click_and_drag`). All clickability = the returned `Response` + checking `clicked()/double_clicked()/dragged()`; in warpui's dispatch model wrap each as a hit-Rect-on-top and `dispatch_typed_action` for stage/unstage/open/toggle, with `ctx.notify` for repaints. Keep `RowResult` so callers branch on `main_clicked`/`checkbox_clicked`/`double_clicked` without re-deriving.
- **Icons:** every glyph is a phosphor `Text` painted via `painter.text(...)` (Proportional font, sizes 12–14). Status letters (`A/M/D/R/?`) are literal chars, not phosphor — keep them as text. Named glyphs used: `GIT_BRANCH`, `ARROW_UP`, `ARROW_DOWN`, `ARROW_COUNTER_CLOCKWISE`, `CHECK`, `FOLDER`, `FILE`, `FOLDER_OPEN`, `FOLDER_PLUS`, `PLUS`, `MINUS`, `GIT_DIFF`, `COPY`, `TRASH`, `CARET_UP`, `CARET_DOWN`, `CARET_RIGHT`, `CARET_DOWN`, `SQUARE`, `CHECK_SQUARE`.
- **Trees:** both are recursive `Node`-tree walks — Changes over `DirNode` (BTreeMap-sorted, built from the flat change list each frame), Files over the live `dir_cache` listing. Port the recursion and the `&mut pending` split for the inline editor. Expansion state is external sets (`collapsed_change_dirs: HashSet<String>` for Changes, `expanded_dirs: HashSet<PathBuf>` for Files) — keep them on App, not the node.
- **Async git:** mirror `Arc<Mutex<GitOpStatus>>` + the **repo-scoping filter** exactly; every pill/spinner/disable derives from `status.repo() == active_repo`. Port `dispatch_git_op` to the warpui background-task primitive, waking the UI on completion.
- **Drag-drop:** reuse warpui's DnD payload (`PathBuf`), resolve `target_dir` at row level, paint the same accent highlight overlay, and emit a deferred `FsDropOp` consumed next frame. Keep all the safety guards (into-self/descendant/same-parent/canonicalize/depth-cap) — they prevent the multi-GB recursive-copy blowup.
- **Footer pinning:** reserve a bottom Rect (height grows with error), render scroll body above it, paint the footer manually. Replicate the `Cmd+Enter` commit shortcut and the accent primary button styling.
- **Animations:** all via `animate_bool_with_time` (hover 90ms, chevron 110ms, checkbox 130ms pop, cb-hover 80ms) + attention glow breathing. Port the easing helpers (`fade` alpha-mult closures).

### 8. Port status

**0% — not started in warpui.** This is a build spec, not a record of a partial port. The entire Right Panel (tab strip, Changes tree + commit footer + async git toolbar/pills, Files tree + drag-drop + inline new-entry editor + context menus) and its dependency `draw_row`/`RowConfig`/`CheckState` are unported. The shared `draw_row` widget and the `GitOpStatus` repo-scoping pattern should be ported first since the rest depends on them.


---


<!-- ===== center-bars ===== -->

## Center Pane View + Top Bar + Status Bar + Branch Picker

Source files: `src/ui/pane_view.rs`, `src/ui/top.rs`, `src/ui/status.rs`, `src/ui/branch_picker.rs`. This area covers the Main Panel layout-tree renderer (recursive splits, pane headers, splitters, drag-dock), the top bar (panel toggles + breadcrumb + split buttons), the bottom status bar (branch + active file path + settings/help), and the floating branch-picker popup.

---

### 1. Public types & key structs/enums

#### `pane_view.rs`

- **Constants (public):** `HEADER_H: f32 = 26.0` (pane header strip height). Private: `BORDER_W = 1.0`, `SPLITTER_W = 4.0`.
- **`struct DragPayload(PaneId)`** — *private*, but central. The egui drag-and-drop payload set when a pane header drag starts. Carries the source `PaneId`.
- **`enum PaneAction`** (public) — the single return value of `render_layout`. Variants:
  - `None`
  - `Focus(PaneId)` — make this pane the focused one.
  - `Close(PaneId)` — close pane.
  - `ResizeSplit { path: Vec<usize>, ratio: f32 }` — splitter drag; `path` is the index path (0/1 children) into the layout tree to the `Split` node, `ratio` is new split ratio.
  - `SwapPanes { a: PaneId, b: PaneId }` — drop on center zone: swap two panes' positions.
  - `DockPane { src: PaneId, target: PaneId, edge: DockEdge }` — drop on an edge zone: re-dock `src` to a new split beside `target`.
  - `ToggleMaximize(PaneId)` — maximize/restore a single pane to full rect.
  - `ReplaceWithTerminal(PaneId)` / `ReplaceWithBrowser(PaneId)` — Welcome-pane buttons; deferred to `main.rs` (needs `ctx` for PTY spawn).
  - `ShowFilesPanel` — Welcome pane → reveal Right Panel.
  - `OpenFile(PathBuf)` — terminal-clicked in-workspace path → open in editor.
  - `OpenFileExternal(PathBuf)` — external OS file drop → open read-only.
- **`enum DockEdge`** (defined in `state::layout`, used here): `Center`, `Left`, `Right`, `Top`, `Bottom`.
- **Layout tree** (from `state::layout`): `Layout { root: Option<Node>, panes: HashMap<PaneId, Pane>, focus: Option<PaneId>, maximized: Option<PaneId> }`. `Node::Leaf(PaneId)` or `Node::Split { direction: Dir, first: Box<Node>, second: Box<Node>, ratio: f32 }`. `Dir::Horizontal | Vertical`.
- **`Pane`** has `title: String`, `content: PaneContent`. `PaneContent` enum: `Terminal(TerminalPane)`, `Files(FilesPane)`, `Markdown(MarkdownPane)`, `Browser(BrowserPane)`, `Welcome(_)`. `PaneContent::kind_label()` returns a short type label string used in the header.

#### `top.rs`
- Constants: `TOPBAR_H: f32 = 34.0`, `TOTAL_H: f32 = TOPBAR_H`. No structs — operates on `&mut App`.

#### `status.rs`
- Constant: `HEIGHT: f32 = 28.0`.
- Reads `App::branch_picker` (`BranchPicker` struct, defined in state): fields touched here — `open: bool`, `query: String`, `opened_at: Option<Instant>`, `loading: bool`, `repos: Vec<(PathBuf, Vec<String>, Vec<String>)>` (root, locals, remotes), `filter: Option<PathBuf>`, `job: Option<JobHandle>`.
- `TabKind::File(FileTab)` with `FileTab.path: String`; `FilesPane { tabs: Vec<TabKind>, active: usize }`.

#### `branch_picker.rs`
- Constants: `MIN_WIDTH = 280.0`, `MIN_HEIGHT = 200.0`, `CORNER_HANDLE = 14.0`.
- **`enum RowAction`** (public): `None`, `Primary` (row body click — open existing worktree or open new-worktree modal), `InPlace` (the "Switch" pill — `git switch` in place).
- `BranchPicker` additional fields used: `width: f32`, `height: f32`, `collapsed: HashSet<String>` (section collapse keys), `error: Option<String>` (last in-place switch error).

---

### 2. Rendered UI elements

#### 2a. Center pane view (`pane_view.rs`)

**Theme tokens (helper fns):** `focus_border`→`focus_border`, `inactive_border`→`inactive_border`, header active bg→`surface`, header inactive bg→`topbar_bg`, header fg→`text`, header dim fg→`text_muted`, close-hover bg→`error`, splitter→`divider`, pane body bg→`bg`, maximize-hover bg→`row_hover`.

**Render flow:** `render_layout` → if `layout.maximized` is a live pane, render only that pane at full `rect` (Esc consumes → `ToggleMaximize`). Else `render_node` recurses the tree.

**Splitter** (`render_splitter`): a 4px-wide bar (`SPLITTER_W`) filled with `divider`. Drawn between the two child rects. `split_rect` carves the parent: for `Horizontal`, vertical bar at `min.x + width*ratio ± 2px`; for `Vertical`, horizontal bar at `min.y + height*ratio ± 2px`.
- Interaction: `Sense::click_and_drag()` on id `("splitter", path)`. Hover/drag → cursor `ResizeHorizontal` or `ResizeVertical`. On drag → emit `ResizeSplit { path, ratio }` where ratio = pointer offset / parent dimension.

**Pane** (`render_pane`):
- `inner = rect.shrink(1px)`. Header strip = top 26px (`HEADER_H`). Body = remainder.
- Body bg filled with `bg`. Content rendered into a clipped child UI pushed under id `("pane_body", id)` (avoids ScrollArea id-collisions/red flash).
- **No visible pane border** (Warp-style); `border_color` is computed but unused. Active/inactive distinction is a **dim overlay**: if `!is_focus` and not a drop target, paint `rect` with `Color32::from_rgba_unmultiplied(0,0,0,45)`, corner radius 4.0 — a translucent black scrim that darkens inactive panes.
- Content dispatch by `PaneContent`: Terminal→`terminal::view::render_terminal_pane` (returns clicked path → `OpenFile`); Files→`file_view::render` (returns close-bool → `Close`; emits external dropped files → `OpenFileExternal`); Markdown→`markdown_view::render`; Browser→`browser_view::render`; Welcome→`welcome_view::render` (returns `WelcomeAction::{OpenTerminal,OpenBrowser,ToggleFilesPanel}` → mapped to `ReplaceWithTerminal/ReplaceWithBrowser/ShowFilesPanel`).
- **External drop**: before content match, if `!external_drop_handled`, reads `ctx.input().raw.dropped_files`; first path → `OpenFileExternal`. Works on ANY pane type.
- **Focus on press**: `pointer.primary_pressed()` inside `rect` and not already focused → `Focus(id)` (press, not click-release, so a drag-select in a sibling still transfers focus).

**Pane header** (`render_header`), 26px tall, left→right layout but hand-painted:
- BG: `surface` (active) or `topbar_bg` (inactive), filled flat.
- **Close button**: rightmost square (size = header height = 26px) at `rect.max.x - 26`. Glyph `icons::X` (phosphor `X`), FontId 13.0 Proportional, color `text`. On hover, fill square with `error` (red). Click → `Close(id)`.
- **Maximize/restore button**: square immediately left of close. Glyph `icons::ARROWS_IN_SIMPLE` when maximized else `icons::ARROWS_OUT_SIMPLE`, 13.0 Proportional, `text`. Hover → fill `row_hover` + cursor `PointingHand`. Tooltip "Restore (Esc)" / "Maximize". Click → `ToggleMaximize(id)`.
- **Title region**: from `rect.min.x + 10` to `max_rect.min.x - 6`. `Sense::click_and_drag()`. Text painted LEFT_CENTER: `"{title}  ·  {kind_label}"`, FontId 12.5 Proportional, color `text` (focused) / `text_muted` (unfocused).
  - `drag_started` → set DnD payload `DragPayload(id)`.
  - hover → cursor `Grab`; dragged → cursor `Grabbing`.
  - clicked → `Focus(id)`.

**Drag-dock zones** (`dock_zone` + `zone_rect`): while a `DragPayload` is in flight and pointer is over a different pane (`is_drop_target`), compute `drop_edge`. Center 30%×30% square (rel 0.35–0.65 both axes) → `Center`; else nearest edge by which of dx/dy dominates → `Left/Right/Top/Bottom`. `zone_rect` is the highlighted area: full rect (Center), left/right/top/bottom half.
- **Drop overlay** (painted last, above content): fill `zone` with `Color32::from_rgba_unmultiplied(96,140,220,90)` (translucent blue), corner radius 4.0, plus a 2px stroke `Color32::from_rgb(96,140,220)` inside. (Hardcoded blue — NOT a theme token.)
- On pointer release over a valid target: `Center`→`SwapPanes{a:payload,b:id}`, edge→`DockPane{src,target,edge}`.

#### 2b. Top bar (`top.rs`), 34px tall

- BG filled `topbar_bg`; 1px bottom border line `divider`.
- Inner UI shrunk by (10,4), left_to_right centered.
- **Left Panel toggle** (leftmost): `icon_button` 16px. Glyph `icons::SIDEBAR_SIMPLE` when `show_left` else `icons::SIDEBAR`. Tooltip "Toggle Left Panel (Cmd+B)". Click → flips `app.show_left`.
- `add_space(6)` then **breadcrumb**: `app.breadcrumb()` label, size 12.5, color `text` (`primary()`).
- **Right cluster** (right_to_left): from the right edge inward —
  1. **Right Panel toggle**: `icon_button` 16px, glyph `SIDEBAR_SIMPLE`/`SIDEBAR` by `show_right`, tooltip "Toggle Right Panel (Cmd+/)". Click → flips `show_right`.
  2. **Git Log toggle**: `icon_button` 16px, glyph `icons::GIT_BRANCH`, tooltip "Toggle Git Log (Cmd+9)". Click → `app.toggle_git_log(ctx)`.
  3. `ui.separator()`.
  4. **Browser split button**: framed `egui::Button`, text `"{GLOBE}  Browser"` (phosphor `GLOBE`) size 12.5, tooltip "Split active pane with browser". Click → split active layout with a new `BrowserPane::new_with("", "https://")`, `Dir::Horizontal`.
  5. **Terminal split button**: framed button, text `"{TERMINAL_WINDOW}  Terminal"` size 12.5, tooltip "Split active pane with terminal (Cmd+T or Cmd+D)". Click → `active_layout().split_focused_with_terminal(ctx, Dir::Horizontal)`.

#### 2c. Status bar (`status.rs`), 28px tall, bottom-anchored

- BG `topbar_bg`; 1px top border `divider`.
- left_to_right centered, `add_space(10)`.
- **Branch label** (left): if a branch exists, `Label` (sensed clickable) text `"{GIT_BRANCH}  {branch}"` (phosphor `GIT_BRANCH`) size 13.0, color `text`. Hover → `PointingHand`. Click → toggles `branch_picker.open`, clears query; on open, calls `load_branch_picker` and stamps `opened_at`.
- **Right cluster** (right_to_left), `add_space(10)`:
  - **Help button**: frameless `Button`, glyph `icons::QUESTION` size 15.0 color `text_muted`, min_size 26×24, tooltip "Keyboard shortcuts". Click → flips `show_help`.
  - **Settings button**: frameless, glyph `icons::GEAR` size 15.0 `text_muted`, min 26×24, tooltip "Settings". Click → flips `show_settings`.
  - `add_space(4)`; **1px vertical divider** (14px tall, color `divider`); `add_space(6)`.
  - **Active file path**: if a focused (or any) Files pane has an active File tab, show its path made relative to the workspace root (`relative_to_workspace`), label size 13.0 color `text_muted`.

`active_file_path` prefers the focused pane's active File tab; else scans all panes for the first Files pane with an active File tab.

#### 2d. Branch picker (`branch_picker.rs`) — floating bottom-left popup

- Gated by `branch_picker.open`. Polls `poll_branch_picker` each frame. If no active project/workspace/tab → force-close.
- **Sizing/position**: bottom-anchored. `max_h = screen.height - status::HEIGHT - 40`, `max_w = screen.width - 24`. `width`/`height` clamped to [MIN, max] and persisted on `App`. Bottom edge = `screen.max.y - status::HEIGHT - 6`; left = `screen.min.x + 12`; top = bottom - height.
- **Area**: `egui::Area` id "branch_picker", `Order::Foreground`, fixed at `outer.min`.
- **Frame**: single `Frame` fill `surface`, 1px stroke `divider`, corner radius 8, inner margin 8. Sets min+max size to `outer.size()`.
- **Title row**: label `"{GIT_BRANCH}  Switch branch"` size 12.0 color `text`. Right-aligned **Close ×**: frameless button glyph `icons::X` size 13.0, min 22×22, tooltip "Close (Esc)" → `close = true`.
- **Dirty warning banner** (conditional): if active workspace has >0 changes (`changes.len() + added + deleted`), an amber frame: fill `Color32::from_rgba_unmultiplied(226,192,80,36)`, 1px stroke `rgb(226,192,80)`, radius 4, margin sym(8,4). Text `"{WARNING}  {n} uncommitted change(s) — in-place switch (Switch) will be refused. Worktree switching is fine."` size 10.5 color `text`.
- **Repo filter chips** (only when >1 repo): horizontal scroll (`id_salt "branch_picker_repos"`, max_h 30, scrollbar AlwaysHidden). First chip "All repos", then one per repo (`repo_display`). `chip()`: `Button` size 10.5; selected → fill `accent@55` + 1px `accent` stroke + `text` color; unselected → transparent fill + `divider` stroke + `text_muted`. min_size (0,22). Click → set `new_filter`.
- **Query field**: `TextEdit::singleline` bound to `branch_picker.query`, id "branch_picker_query", hint "Filter branches…", full width. **One-time focus**: gated by memory flag id "branch_picker_focused" (request_focus once, not per-frame); flag cleared on close.
- **Error label** (conditional): if `branch_picker.error`, a wrapped clickable `Label` size 11.0 color `error`; click → clears the error.
- **Body states**:
  - loading + no visible repos → `Spinner` (size 14) + "Loading branches…" size 11.5 `text_muted`.
  - empty → "No repos found under this Workspace" size 11.5 `text_muted`.
  - else → vertical `ScrollArea` (`id_salt "branch_picker_list"`, auto_shrink [false,true]); renders each visible repo via `render_repo_section`.

**`render_repo_section`** builds remote groups (`BTreeMap<remote, Vec<branch>>` by splitting `remote/branch`), filters by lowercase query. Renders (when multi-repo) a repo `section_header` (indent 0), then a "Local" header (indent 1 if multi else 0) + local branch rows, then per-remote headers + remote branch rows.

**`section_header`** (22px): `Sense::click()`, hover→PointingHand. Caret glyph `icons::CARET_RIGHT` (collapsed) / `icons::CARET_DOWN` (expanded), 11.0, `text_muted`, at x=`min.x+4+16*indent`. Name text at +18px, size 11.5, color `text` (indent 0) / `text_muted` (nested). Right-aligned count string size 10.5 `text_muted`. Returns clicked → `toggle(collapsed, key)`.

**`row`** (24px) — a branch entry, single click source via `allocate_exact_size`:
- BG: active→`accent@45` rounded 4; hovered→`row_hover`; else transparent.
- Branch name LEFT_CENTER at x=`min.x+8+16*indent`, FontId 12.0 proportional, color `text`.
- **Badge** (right edge -8px, RIGHT_CENTER, size 10.5): text "current" (active, color `accent`) / "open" (has worktree, `text_muted`) / "create" (`text_muted`). Width estimated as `len*5.5px` (no `fonts_mut` — avoids a documented RwLock-deadlock from font-atlas write-lock inside deep layout).
- **"Switch" pill** (only when hovered & not active): rect placed left of the badge (`max.x - 16 - badge_w - pill_width`, 18px tall). Pill width = `len*5.8 + 12`. Fill/stroke: if pointer over pill → `accent@70` + `accent` stroke; else `white@18` + `divider` stroke. Text "Switch" centered, 10.5, `text`. Tooltip when over pill: "Switch in place (git switch) — requires a clean tree".
- Hover → `PointingHand`. Click dispatch by pointer position: over pill → `InPlace`, else → `Primary` (only when not active).

**Resize corner handle**: 14px square at outer top-right (`outer.max.x - 14`, `outer.min.y`). `Sense::drag()`, id "branch_picker_resize_corner". Two diagonal tick `line_segment`s (1.75px) hugging the corner; color `accent` when hover/drag else `text_muted`. Cursor `ResizeNeSw`. Drag: up/left grows (`height -= dy`, `width += dx`), clamped.

---

### 3. Interactions & dispatch summary

- **Splitter drag** → `ResizeSplit`. **Header click** → `Focus`. **Header drag** → set DnD payload; drop center → `SwapPanes`, drop edge → `DockPane`. **Close ×** → `Close`. **Maximize** → `ToggleMaximize`. **Esc (maximized)** → restore. **Press inside unfocused pane** → `Focus`. **Welcome buttons** → Replace/ShowFilesPanel. **Terminal path click** → `OpenFile`. **External OS file drop** → `OpenFileExternal`.
- **Top bar**: toggles for Left/Right/GitLog panels, split-with-browser, split-with-terminal.
- **Status bar**: branch label click → open/close picker; settings/help toggles.
- **Branch picker**: chip click → filter; query type → filter; section header click → collapse; row body → `Primary` (switch to existing worktree via `set_active`, OR open new-workspace modal pre-filled & branch-locked); pill → `InPlace` (`git::checkout_branch`, on success rename active workspace + refresh git status + close; on failure set `error`); close via × / Esc / outside-click (with 150ms grace window keyed off `opened_at`); corner drag → resize.

### 4. States & animations

- **Pane focus**: focused = no scrim + active header bg (`surface`) + bright title; unfocused = black@45 scrim + inactive header bg (`topbar_bg`) + dim title. No border, no animation.
- **Drop-target**: blue@90 zone fill + blue 2px stroke, scrim suppressed while targeted.
- **Hover**: close→red fill, maximize→`row_hover`, splitter→resize cursor, header→grab cursor, branch row→`row_hover` + pill appears.
- **Selected**: filter chip selected (accent fill+stroke); branch row active (accent@45 + "current" badge).
- **Collapsed/expanded**: caret right/down, section body hidden; collapse state persisted in `branch_picker.collapsed` HashSet.
- **Loading**: `Spinner`. **Disabled**: none explicit. No tweened animations anywhere — pure state-driven repaint.

---

### 5. warpui port approach

- **Recursive layout tree**: port `Node`/`Split`/`Leaf` as a recursive enum; `render_node` walks it carving rects with the same `split_rect` (4px splitter gutter). Each leaf becomes a panel with a hand-painted header. Return a `PaneAction` analog up the stack (or push typed actions into a per-frame queue) and apply after the walk — matches the current "compute action, mutate after" pattern.
- **Clickable hand-painted regions** (header buttons, splitter, branch rows, section headers, chips, resize handle): reuse the warpui idiom — allocate the hit-`Rect`, place an interaction sense on top, and on click `dispatch_typed_action(...)` + `ctx.notify(...)` instead of mutating `App` inline. The current code already uses `allocate_exact_size`/`ui.interact` + pointer-position dispatch (e.g. the row's pill-vs-body split); keep the single-click-source pattern to avoid double-fire.
- **Icons**: every glyph is `egui_phosphor::regular::*` rendered as `painter.text` or `Button`/`RichText`. In warpui, port as phosphor `Text` widgets/painter calls. Glyph inventory: `X`, `ARROWS_IN_SIMPLE`, `ARROWS_OUT_SIMPLE`, `SIDEBAR`, `SIDEBAR_SIMPLE`, `GIT_BRANCH`, `GLOBE`, `TERMINAL_WINDOW`, `QUESTION`, `GEAR`, `WARNING`, `CARET_RIGHT`, `CARET_DOWN`.
- **Colors**: all but the hardcoded drop-zone blue (`96,140,220`) and amber dirty-banner (`226,192,80`) are theme tokens — map `surface/topbar_bg/bg/text/text_muted/divider/accent/error/focus_border/inactive_border/row_hover` to warpui theme tokens. Promote the two hardcoded colors to tokens (`drop_zone`, `warning`) during port.
- **Drag-and-drop**: reuse warpui's DnD payload channel for `DragPayload(PaneId)`; compute `dock_zone`/`zone_rect` identically; paint overlay last (above content).
- **Branch picker**: a foreground `Area`/overlay anchored bottom-left; persisted width/height on app state; one-time-focus memory flag; background branch discovery via the job system (`discover_repos` + `list_local_branches`/`list_remote_branches`), result drained non-blocking via `poll_branch_picker`. Keep the estimated text-width hack (`len * ~5.5px`) to avoid the font-atlas write-lock deadlock unless warpui's text layout is lock-free.

### 6. Honest port-status

**~5–10%.** These are Crane (egui) source files; warpui is a separate target. The structural recipes are clear and several patterns already map 1:1 (action-enum-then-apply, hand-painted clickable rects, phosphor glyphs, theme tokens). Not yet ported: layout-tree renderer, splitter drag, pane header/buttons, drag-dock zones+overlay, top bar, status bar, and the entire branch-picker popup (filter chips, query, repo/local/remote sections, branch rows with badge + Switch pill, dirty-warning banner, corner resize, outside-click grace window). Two colors and the text-width estimation hack need decisions during port. Behavior is fully specified above for a 1:1 rebuild.


---


<!-- ===== file-editor ===== -->

## Files Pane / File Editor

This section documents the Files Pane — the in-Crane file-tab editor surface (`src/views/file_view.rs` + helpers `file_status.rs`, `file_util.rs`, `file_find.rs`, `highlight.rs`). It renders a horizontal tab bar, a path/action toolbar, an optional find/replace bar, an optional go-to-line bar, the syntax-highlighted code editor (or image / markdown-preview / PDF surface), a left line-number gutter with git-change markers, a right-edge scrollbar minimap (diagnostics + git), and a bottom status strip. It also owns inline editor behaviors (auto-indent, bracket pairing, line move/duplicate, comment toggle, line cut/copy, goto-definition, Cmd-hover underline).

---

### 1. Public types & key state

These render functions are pure views over state that lives in `crate::state::layout` (`FilesPane`, `FileTab`, `TabKind`). The render layer never owns this state; it mutates it through `&mut`.

**`FilesPane`** (in `state::layout`, consumed here) — key fields referenced:
- `tabs: Vec<TabKind>` — open tabs (file or diff).
- `active: usize` — active tab index (clamped to `len-1` each frame).
- `pending_close: Option<usize>` — index awaiting the "Unsaved changes" confirm modal.
- methods: `close(idx)`, `tabs[i].is_dirty()`, `tabs[i].name()`, `tabs[i].is_read_only()`, `as_file()/as_file_mut()`, `as_diff_mut()`.

**`TabKind`** enum — `File(FileTab)` or `Diff(DiffTab)`. The active-tab dispatch in `render_scoped` branches on this: a `Diff` tab delegates entirely to `diff_view::render_diff_body`.

**`FileTab`** (consumed here) — the per-open-file editor state. Fields touched by this area:
- `path: String`, `name: String`, `content: String`, `original_content: String`.
- `read_only: bool`, `preview: bool` (one-click-preview tab, promoted to permanent on first edit), `preview_mode: bool` (markdown rendered-vs-source toggle).
- `external_change: bool`, `disk_mtime: Option<SystemTime>`, `save_error: Option<String>`.
- `find_query: Option<String>` (None = find bar closed), `replace_query: String`, `show_replace: bool`, `find_scroll_to_line: Option<u32>`.
- `goto_line_active: bool`, `goto_line_input: String`.
- `pending_cursor: Option<(u32,u32)>` (line,col to apply next frame), `last_cursor_idx: usize`, `selection_info: Option<(usize chars, usize lines)>`.
- `line_changes: Option<FileDiff>`, `line_changes_key: u64` (content-hash guard so git diff only reparses on edit).
- `image_texture: Option<TextureHandle>`, `pdf_state: Option<Box<PdfTabState>>`.
- methods: `dirty()`, `name`.

**`EditorPrefs { word_wrap: bool, trim_on_save: bool }`** (`Clone, Copy`) — editor prefs threaded in from `App`/`Settings`. `word_wrap` switches `desired_width` between `available_width()` and `f32::INFINITY`; `trim_on_save` is read in `file_save`.

**`CachedGalley { key: u64, galley: Arc<Galley> }`** (private) — cross-frame galley cache stored in egui memory keyed `("file_view_layouter", path)`. `key = hash64((text, layout_salt))`. Returns the cached galley untouched when the key matches, keeping syntect off the hot path.

**`LineHL`** (`highlight.rs`, public) — per-line incremental highlight entry: `text_hash: u64`, `parse_state: ParseState`, `highlight_state: HighlightState`, `segments: Vec<(Style, String)>` (owned).

**`LineHighlightCache { context_hash: u64, lines: Vec<LineHL> }`** (`highlight.rs`, public, `Default`) — stored in egui memory keyed `("file_view_lines", path)`. `context_hash` = hash of (requested theme name, syntax name) — any mismatch wipes all lines.

**`FindBarOutcome { close, next, prev, replace, replace_all: bool }`** (`file_find.rs`, public) — returned by `render_find_bar`; the caller acts on each flag.

---

### 2. The `render` entry point

`render(ui, pane_id, pane, font_size, title, syntax_theme_override, diagnostics_for, notify_saved, format_before_save, goto_request, workspace_root, prefs, external_drop_handled, dropped_external_files) -> bool`. Returns `true` when the pane should close (last tab closed). Wraps everything in `ui.push_id(("files_pane", pane_id), …)` then delegates to `render_scoped`. The closures are dependency-injected callbacks:
- `diagnostics_for(&path) -> Vec<Diagnostic>` — LSP diagnostics for the file.
- `notify_saved(path, msg)` — toast on save.
- `format_before_save(path, content) -> Option<String>` — prettier/format hook.
- `goto_request(path, line, col)` — F12 / Cmd+click goto-definition.

**External drag-drop:** if `!external_drop_handled`, dropped file paths from `ctx.input().raw.dropped_files` are appended to `dropped_external_files` (opened as read-only tabs by the caller).

---

### 3. Empty state

When `pane.tabs.is_empty()`: centered (`vertical_centered`), 8px top space, +20px, label **"No files open"** (size 14, color `text`), +4px, label **"Click a file in the Files sidebar to open it here"** (size 11.5, color `text_muted`). Returns `false`.

---

### 4. Tab bar (`draw_file_tab`)

A `ScrollArea::horizontal` (`id_salt("file_tab_bar")`, `auto_shrink([false,true])`, scrollbar `AlwaysHidden`) inside `ui.horizontal` with `item_spacing.x = 2.0` and a 4px leading space. Each tab is hand-painted by `draw_file_tab`, not an egui Button.

**Label prefix glyph** (built in `render_scoped`):
- dirty → `icons::CIRCLE` + two spaces + name
- diff tab → `icons::GIT_DIFF` + name
- read-only → `icons::LOCK` + name
- otherwise plain name

**Geometry:** font = Proportional 12; close font = Proportional 13. `text_w` measured via `fonts_mut().layout_no_wrap`. `padding_x=10`, `gap=6`, `close_size=16`, `height=26`, `width = padding_x + text_w + gap + close_size + padding_x − 2`. Rounded corners 5.0. Allocated with `Sense::click_and_drag()`. Close hit-rect is a 16×16 square pinned `rect.max.x − padding_x − close_size + 2`, vertically centered.

**Colors / states** (theme tokens):
- active → bg = `accent` @ alpha 55 (`accent_tint`), fg = `text`; plus a 2px `accent` bottom border inset 4px each side.
- hover (tab or close hovered) → bg = `row_hover`, fg = `text`.
- idle → transparent bg, fg = `text_muted`; plus a 1px `border` bottom border inset 4px.
- diff tab idle → blue tint overlay `(100,180,255,30)`.
- read-only tab → red tint overlay `(220,80,80,35)` (added on idle and on active).
- preview/read-only label (when not active) dimmed to `text_muted`.

**Close button:** shown when `is_active || hovered || pointer-over-close-rect` (`close_rect_contains` peeks `pointer.hover_pos`). Glyph `icons::X` (font 13), centered. Hover paints an `error`-colored rounded (4px) fill behind it.

**Interactions / return `(clicked, close_clicked)`:**
- body click (when not over close) → `activate_idx` → sets `pane.active`.
- close click **or middle-click on tab** → `close_idx`.
- hover (tab or close) → `CursorIcon::PointingHand`.

**Close routing:** dirty tab → `pane.pending_close = Some(idx)` (confirm modal). Clean tab → `pane.close(idx)` immediately; returns `true` if that emptied the pane.

**Unsaved-changes modal** (`render_close_confirm`): `egui::Window` "Unsaved changes", id `("file_close_confirm", idx)`, non-collapsible, non-resizable, anchored CENTER_CENTER, min width 340. Body: `"<name>" has unsaved changes.` + muted (11.5) `Discard them and close the tab?` + 12px gap + horizontal **Cancel** / **Discard** buttons. Esc = Cancel. Discard → `pane.close(idx)`. Save-then-close is intentionally not offered (saving needs the injected formatter/notify closures not threaded here).

---

### 5. Active-tab dispatch & keyboard

After clamping `active`, `render_scoped` reads kind flags. Diff tabs set `*title = "Files · <title>"` and call `diff_view::render_diff_body`, then return.

Keyboard (file tabs, gated on `active_is_file` and mostly `!read_only`):
- **Cmd+S** → `save_pressed` (blocked read-only); triggers `file_save::save_tab(... force=false)` if dirty.
- **Cmd+F** → opens find bar; seeds `find_query` with the current TextEdit selection (extracted via `TextEdit::load_state` cursor char_range → byte range into `content`) or empty string. Never closes.
- **Cmd+H** → toggles `show_replace` (and opens find bar if closed); consumes the key.
- **Esc** → closes find bar (in `render_find_bar`).
- Editor-scoped (only when TextEdit focused & not read-only), all `consume_key`'d: **Tab** insert indent unit; **Shift+Tab** outdent one level; **Enter** auto-indent (computes `auto_indent_context` → bump/dedent, special-cases next-char `}` `)` `]` to open a blank line at brace level); **Cmd+/** toggle line comment; **Ctrl+G** open go-to-line; **Alt+Up/Down** move line/selection; **Alt+Shift+Down** duplicate line; bracket pairs `{}()[]` auto-close with skip-over; **Cmd+X** on empty selection cuts whole line (with discrete undo entry); **Cmd+C** on empty selection copies whole line; **F12 / Cmd+click** goto-definition (`goto_request`); **Cmd+hover** underlines the identifier token and shows hand cursor. Indentation unit comes from `crate::format::discover(path).indent_unit()` (nearest `.prettierrc`/package.json prettier field).

---

### 6. Toolbar & banners

**External-change banner** (`tab.external_change`): `Frame::NONE` fill `(220,100,100,28)`, 1px `error` stroke, corner 4, margin (10,6). Text `icons::WARNING + "This file changed on disk outside Crane."` (size 11.5, color `text`). Right-to-left small buttons: **Dismiss** (clears flag, refreshes mtime), **Overwrite** (`save_tab(... force=true)`), **Reload** (`reload_tab`).

**Save-error banner** (`tab.save_error`): same frame, text `icons::WARNING + "Save failed: <err>"`.

**Path/action row** (`ui.horizontal`, 4px lead): left — `short_path(path, workspace_root)` label (size 10.5, `text_muted`, hover tooltip = full path). Right-to-left:
- read-only → **`icons::LOCK_OPEN` Unlock** button (min height 24) → clears read_only, resets `original_content`, refreshes mtime.
- editable → **`icons::FLOPPY_DISK` Save** button, `add_enabled(tab.dirty())`, min height 24.
- markdown files → toggle button: `icons::EYE` "Preview" / `icons::PENCIL_SIMPLE` "Edit" → flips `preview_mode`.

---

### 7. Find / replace bar (`file_find.rs`)

Rendered only when `find_query.is_some()`. Closing (query None) clears the `("find_focused", path)` memory flag and forces `show_replace=false`.

**Find row** (`ui.horizontal`, 4px lead): label `icons::MAGNIFYING_GLASS + "Find"` (11, muted); singleline `TextEdit` id `("find_input", path)`, width `available − 180`, hint `"type to search…"`. First frame auto-`request_focus` gated by `("find_focused", path)` memory flag (fires once). Hit count label = `content.matches(query).count()` (10.5, muted). Nav buttons (each `Button` 22×22, glyph size 14, color `text`): `ARROW_UP` "Previous (Shift+Enter)", `ARROW_DOWN` "Next (Enter)". Right-to-left: `X_CIRCLE` "Close (Esc)".

**Replace row** (when `show_replace`): label `icons::PENCIL_SIMPLE + "Replace"`; singleline `replace_query` id `("replace_input", path)` width `available − 260`, hint `"replace with…"`; buttons "Replace" and "Replace All" (min height 22).

**Keys:** Enter on lost-focus → next; Esc → close; Shift+Enter → prev.

**Find logic (in file_view):** next/prev compute the target byte (`content[after..].find` wrapping, or `rfind`), set `pending_cursor` and `find_scroll_to_line`. Replace-at-cursor replaces the next match after cursor then repositions cursor; Replace-all does `content.replace(q, replace_query)`.

**Match highlight** (`paint_find_matches`): soft amber `(220,180,50,90)` rect (corner 2) behind every occurrence in the galley; iterates `text.find(query)`, maps char ranges to `galley.pos_from_cursor`, skips multi-row matches.

---

### 8. Go-to-line bar (`Ctrl+G` → `goto_line_active`)

`Frame::NONE` fill `bg`, 1px `border` stroke, corner 4, margin (10,6). Label `"Go to Line:"` (11.5, `text`) + singleline `goto_line_input` (Monospace 12, width 80). Auto-focus once via `("goto_focused", path)` memory flag. Enter → parse u32, clamp to line count, set `pending_cursor=(target,0)` + `find_scroll_to_line`, close. Esc → close. Both clear the input and the focus flag.

---

### 9. The editor body (text path)

Short-circuits: **PDF** (`pdf_view::is_pdf_path`) → lazily build `PdfTabState`, render via `pdf_view::render_pdf`, return. **Image** (`file_util::is_image_path`; exts png/jpg/jpeg/gif/bmp/webp/ico) → lazy `load_texture` (LINEAR), shown in `ScrollArea::both` at original size. **Markdown + preview_mode** → `ScrollArea::vertical` rendering `markdown_view::render_md`.

**Syntax setup:**
- `syntaxes()` — two-face extra-newlines syntax set + user `~/.crane/syntaxes` folder, once-init.
- `find_syntax_for_ext(ext)` — extension lookup with flavour fallbacks (tsx→TypeScript, jsx→JavaScript, vue/svelte/astro→HTML, zsh/fish/bash→bash, h→C, hpp/cc/...→C++, else Plain Text).
- `themes()` — syntect defaults + two-face embedded themes keyed by Debug variant name (e.g. "VisualStudioDarkPlus"). `available_syntax_themes()` returns a priority-ordered list. `fallback_theme()` is a panic-proof empty theme.
- Requested theme = `syntax_theme_override` else `theme::current().syntax_theme`; fallback chain depends on light/dark (light→InspiredGithub; dark→OneHalfDark→base16 variants).

**Incremental highlight** (`highlight::rehighlight`): syntect's `ParseState`+`HighlightState` carried per line. Walks the cache resuming state while `text_hash` matches; truncates at `first_diff`; re-highlights from there to EOF. Common keystroke at bottom of file = one line reparsed. A `context_hash` mismatch (theme/syntax change) clears the cache. The layouter builds a `LayoutJob` from cached segments (foreground from syntect Style, or `text` token when alpha==0), caches the galley in egui memory under `CachedGalley`. The cache `layout_salt` includes `font_size`, requested theme, and **UI theme name** (omitting the theme name previously returned a stale galley → scrambled glyphs after theme switch).

**Layout geometry:** bottom 22px reserved for the status strip; editor height = `available − 22` (min 80). Gutter width = `gutter_char_w * digits + 16` where `digits = max(2, line_count_digit_len)`, `gutter_char_w` measured at `font_size*0.7` Monospace and cached in memory per size. Two-column manual layout: gutter `Rect` on the left, a child `Ui` for code starting at `gutter_w + 6px` pad. Code lives in `ScrollArea::both` (`id_salt ("file_scroll", active_idx)`, `auto_shrink([false;2])`, `max_height editor_h`).

**TextEdit:** `multiline(&mut content)`, id `("file_editor", path).with("body")` (path-scoped so undo/cursor don't leak across tabs), `interactive(!read_only)`, `.code_editor()`, `lock_focus(true)`, `frame(Frame::NONE)`, `desired_rows(30)`, `desired_width` per `word_wrap`, custom `layouter`. `actual_row_h` captured from `galley.rows[1].min.y − rows[0].min.y` for exact gutter alignment.

**Post-render overlays (painted in order):** identifier underline on Cmd-hover (1.5px `accent`); diagnostics overlay (`diagnostics_overlay::paint`); find matches (amber); current-line highlight (`(0,0,0,18)` light / `(255,255,255,18)` dark, full code-area width); gutter line numbers + git markers; deletion-gap red bars; diff tooltips; scrollbar diagnostic + git minimap. `pending_cursor` is applied (sets cursor, requests focus, scrolls). `last_cursor_idx` and `selection_info` are stashed from `out.state.cursor`.

**Context menu** (`out.response.context_menu`): read-only → `LOCK_OPEN` "Unlock for Editing"; else `FLOPPY_DISK` "Save". Always `FOLDER_OPEN` "Reveal in Finder" (`reveal_in_file_manager`) and `COPY` "Copy Path" (`copy_text`). Plumbed via `Rc<Cell<bool>>` to act after the menu closure.

**Scroll-to-line:** `find_scroll_to_line` rewrites the ScrollArea's stored `state.offset.y` (id = `code_ui_id.with(("file_scroll", active_idx))`) so the target line is in view next frame.

---

### 10. Gutter & git change markers

Line numbers right-aligned at `gutter_rect.max.x − 8` (Monospace `font_size*0.7`, color `text_muted`), clipped to gutter, scrolling with `v_offset` but pinned horizontally. Gutter bg = `bg`, 1px right border = `border`.

Git per-line markers (from `git::parse_file_diff`, refreshed only when content hash changes): 3px-wide vertical bar at gutter left edge — **Added** = `diff_added()` green, **Modified** = `diff_modified()` blue. Modified lines are hoverable: hovering shows a tooltip ("Lines X-Y") listing old lines (`-`, color dark `(200,120,120)`/light `(160,50,50)`) and new lines (`+`, dark `(120,200,140)`/light `(30,140,60)`), built from the diff `block` (full hunk, not truncated). Deletions render as a 3px red (`diff_deleted()`) horizontal bar in the gutter between lines, also hoverable (shows removed head lines).

---

### 11. Scrollbar minimap

`paint_scrollbar_diag_markers` (file_status.rs): for each diagnostic, a 4px-tall dash at `x ∈ [max.x−8, max.x−2]`, y = `(line/total)*h`, color by severity (1=`error`, 2=`(226,192,80)`, else `accent`). No backdrop band. The git-change scrollbar dashes mirror this (3px tall for line changes, 2px for deletions) using the same x band.

---

### 12. Status strip (`file_status.rs`)

22px-high strip with a 1px `divider` top line. Left: three severity pills via `sev_button` — `X_CIRCLE`+count (error), `WARNING`+count (`(226,192,80)`), `INFO`+count (`accent`); each is a `Label` with `Sense::click()`, colored `text_muted` when count 0, active color when >0, hand cursor on hover-with-count, click → `jump_to_next_diagnostic` (next diag after current line of that severity, wrapping; sets `pending_cursor`). Right-to-left: optional `LOCK` "Read Only", language label (Monospace 11 via `language_label` — extension/basename table, e.g. Dockerfile/Makefile/Rust/TSX…), indent label ("Tabs" or "Spaces: N" from `format::discover`), `Ln {line}, Col {col}` (1-based, from `char_idx_to_line_col(content, last_cursor_idx)`), and optional `(N sel, M ln)` selection info. All right-side labels size 11, color `text_muted`.

---

### 13. Helpers (`file_util.rs`)

`is_image_path`, `short_path` (workspace-relative → `~`-relative → absolute), `line_col_to_char` / `char_idx_to_line_col` (char-index ⇄ line/col), `trim_trailing_whitespace`, `reveal_in_file_manager` (macOS `open -R` / Linux `xdg-open` / Windows `explorer /select`), `reveal_label`, `comment_prefix` (ext→`//`/`#`/`--`/`;;`), `toggle_line_comments` (adds prefix if any line in range uncommented, else removes; operates over full lines intersecting the selection char range).

---

### 14. WarpUI port approach

- **Tab bar:** reuse the hit-Rect-on-top pattern — for each tab allocate the same hand-computed `Rect` (`Sense::click_and_drag`), paint bg/border/label/close exactly per the color table, and on body-click/middle-click/close-click `dispatch_typed_action` with `ActivateFileTab{idx}` / `CloseFileTab{idx}`. Close hit-test = nested 16×16 `ui.interact`. Drift watch: tabs are NOT egui Buttons — replicate the rounded-5 painter fills and 2px/1px accent/border bottom strokes, not button frames.
- **Icons:** every glyph is `egui_phosphor::regular` painted as a `Text`/`RichText` run — port verbatim (`CIRCLE`, `GIT_DIFF`, `LOCK`, `LOCK_OPEN`, `X`, `FLOPPY_DISK`, `EYE`, `PENCIL_SIMPLE`, `MAGNIFYING_GLASS`, `ARROW_UP/DOWN`, `X_CIRCLE`, `WARNING`, `INFO`, `FOLDER_OPEN`, `COPY`). Never substitute Unicode.
- **Editor core:** keep `egui::TextEdit::multiline` + the custom `layouter`; port `LineHighlightCache`/`rehighlight` 1:1 (it is the perf-critical piece) and the `CachedGalley` memory keying. Path-scope the TextEdit id (`("file_editor", path).with("body")`) — non-negotiable to avoid cross-tab undo leakage.
- **Keyboard:** the `consume_key` / `Event::Cut`/`Event::Copy` interception pattern must be ported faithfully (macOS synthesizes Cut/Copy without Key events). Each editing command mutates `content` + repositions `TextEdit` cursor via `load_state`/`store`.
- **Overlays:** all painters (current-line highlight, diagnostics, find matches, gutter, minimap, diff tooltip) are independent `ui.painter()` passes keyed off `out.galley` + `out.galley_pos` + `scroll_out.state.offset` — reuse directly with theme tokens (`accent`, `error`, `border`, `divider`, `text_muted`, `diff_added/modified/deleted`).
- **Modals/banners:** `egui::Window` (confirm) and `Frame::NONE` banners port directly; route button results through `dispatch_typed_action` rather than the local bool/`Rc<Cell>` plumbing where WarpUI prefers action dispatch, and `ctx.notify` for the save toast (`notify_saved`).
- **Injected closures:** map `diagnostics_for`, `notify_saved`, `format_before_save`, `goto_request` onto WarpUI typed actions / queries; `git::parse_file_diff` and `format::discover` stay as direct calls.

---

### 15. Port status (this area): ~0%

This is a Crane-side spec for reproduction in WarpUI; no WarpUI implementation of the Files Pane editor exists yet. The component is fully specified above and self-contained except for these external Crane dependencies that must be ported or stubbed first: `crate::format` (indent discovery, char/byte conversion, auto-indent), `crate::git::parse_file_diff` (`FileDiff`/`DiffLineKind`/blocks/deletions), `crate::lsp::Diagnostic`, `crate::theme` token set, `diagnostics_overlay`, `diff_view`, `markdown_view`, `pdf_view`, `file_save` (save/reload/poll-external-change), and the `two_face` syntax/theme bundle. The hardest fidelity risks are the incremental highlight cache and the macOS Cut/Copy event interception — both are documented above and should be ported before the surrounding chrome.


---


<!-- ===== doc-panes ===== -->

## Diff / Markdown / Welcome / PDF Panes

These four panes share a common DNA in Crane: they are largely **immediate-mode painter-driven** (not built from egui widgets), they read theme tokens through `theme::current()`, and they use `egui_phosphor::regular` glyphs for every icon. Diff and PDF are heavy (subprocess `git`, pdfium FFI) and offload/cache; Markdown and Welcome are cheap and stateless. This section documents each for 1:1 reproduction in warpui.

---

### 1. Diff Pane (`src/views/diff_view.rs`)

The most complex of the four. A side-by-line unified diff view with syntax highlighting, per-hunk stage/unstage controls, a hunk navigator, and a minimap. The expensive compute runs off-thread via the JobSystem; the render path is allocation-light.

#### 1.1 Public structs / enums + key fields

**`Row`** (public) — one rendered diff line:
- `tag: ChangeTag` (`similar` enum: `Delete` / `Insert` / `Equal`)
- `old_ln: String` — right-justified old line number, pre-padded to `ldigits` width (or all-spaces for inserts)
- `new_ln: String` — right-justified new line number, pre-padded to `rdigits` (or spaces for deletes)
- `content: String` — line text, trailing `\n` trimmed
- `new_lno: Option<usize>` — 1-based new line number (None on deletion). Used to match `similar` row-hunks against `git diff` line-range hunks.
- `old_lno: Option<usize>` — 1-based old line number (None on insertion)

**`DiffComputed`** (public, `#[allow(dead_code)]`) — the pure diff result, built on the job thread, read by render with zero alloc:
- `rows: Vec<Row>`
- `tags: Vec<ChangeTag>` — parallel to rows, cached for minimap + hunk scanning
- `hunk_starts: Vec<usize>` — row indices where a contiguous non-Equal run begins (visual hunks)
- `hunk_patches: Vec<Option<String>>` — per-visual-hunk git unified patch text (None if no git match / no repo). Deduped by git-hunk identity so two visual hunks sharing one git hunk don't both stage.
- `hunk_staged: Vec<bool>` — per-hunk: true if already in index (action = unstage), probed via `git apply --reverse --cached --check`
- `row_to_hunk: Vec<Option<usize>>` — per-row hunk index
- `row_in_shared_group: Vec<bool>` — true for downstream rows of a multi-visual-hunk git group (drives the vertical connector line)
- `ldigits: usize`, `rdigits: usize` — gutter widths (≥3)
- `left_lines_count`, `right_lines_count: usize`

`DiffComputed::empty()` — sentinel returned on mid-compute cancellation; render treats it as a cache miss.

**`DiffTabData`** (from `state::layout`, mutated here) — relevant fields touched: `left_path`, `right_path` (String, may carry `staged:` / `HEAD:` prefixes), `left_text`, `right_text`, `repo_path: Option<String>`, `inputs_version: u64` (cache key — mutators MUST call `invalidate()` to bump), `computed: Option<Arc<DiffComputed>>`, `computed_for_version`, `compute_job: Option<JobHandle>`, `job_for_version`, `error: Option<String>`, `pending_hunk_stage: bool`, `image_texture: Option<TextureHandle>`.

#### 1.2 Color constants (hardcoded, NOT theme tokens — important port note)

These are literal `Color32` constants in the file, not theme tokens:
- `ADD_BG = rgb(25, 55, 35)` — insertion row background (dark green)
- `DEL_BG = rgb(60, 28, 32)` — deletion row background (dark red)
- `CTX_FG = rgb(180, 186, 198)` — context/equal text foreground; also fallback for transparent syntect colors
- `ADD_FG = rgb(140, 220, 150)` — addition accent (green) — `+` sign, stage glyph, minimap inserts, matched-path header
- `DEL_FG = rgb(230, 130, 130)` — deletion accent (red) — `-` sign, minimap deletes, error text, left-path header
- `MUTED = rgb(140, 146, 160)` — line-number gutters, `->` arrow, hunk counter, "Computing diff…"
- `MINIMAP_W = 10.0`

The only theme-token usages: `theme::current().text_muted` (empty-state stage glyph), `theme::current().error` (image-decode failure). Syntax theme comes from `theme::current().syntax_theme` (see `resolve_theme`).

#### 1.3 Compute pipeline (`compute_diff`) — runs on `Pool::Io`, `Priority::Foreground`

1. `TextDiff::from_lines(left, right)` → iterate all changes → build `rows` (with line numbers formatted at this stage).
2. Scan tags for `hunk_starts` (transition Equal→changed).
3. If `repo_path` set: `git::file_diff_raw(repo, right_path)` → `git::parse_hunks_detailed` → for each visual hunk, probe its first ≤5 rows for a `new_lno`/`old_lno`, match against parsed git hunks by line-range containment (`new_start..new_start+new_count` or old equivalent), dedupe matched git-hunk indices via a `HashSet`. Produces `hunk_patches`.
4. `hunk_staged`: for each patch, `git::is_hunk_staged(repo, patch)`.
5. Build `row_to_hunk` (fill each hunk's row span).
6. Build `row_in_shared_group`: walk hunk groups; when ≥2 consecutive patch-bearing visual hunks form a git group, mark rows from `anchor+1` to the last shared hunk's change-end.

Cancel checks at 4 phase boundaries (cooperative, never mid-syscall).

**Cache discipline**: cache hit = one `u64` compare (`computed_for_version == inputs_version`) + `Arc::clone`. On version bump while an old `Arc` exists, render keeps showing the stale diff (no spinner flash); only the very first compute shows the "Computing diff…" label (size 11, MUTED, monospace, indented 8+8px).

#### 1.4 Rendered UI elements

`render_diff_body(ui, tab, font_size, _tab_index)`:

**Image short-circuit**: if `is_image_path(right_path)`, calls `render_image_block` and returns. That block: 4px space → header row (6px indent, left_path in DEL_FG, " -> " in MUTED, right_path in ADD_FG, all size 11 monospace) → 4px space → separator → `ScrollArea::both` (id_salt `("diff_image_scroll", active_idx)`, auto_shrink off) showing the decoded texture at original size (or "Couldn't decode image" in `theme.error`). Texture lazily loaded from `repo_path.join(right_path)` (or raw path), keyed `crane_diff_img:{right_path}`, LINEAR.

**Header** (4px top space, `ui.horizontal`):
- 6px left indent.
- If left/right bare paths (after stripping `staged:`/`HEAD:`) equal → show just the filename in ADD_FG, size 11, monospace.
- Else → left_path (DEL_FG) + " -> " (MUTED) + right_path (ADD_FG), all size 11 monospace.
- Right-to-left layout block (8px right pad): hunk navigator —
  - `ARROW_DOWN` button (size 12, min_size 22×22), enabled iff hunks exist. Click → advance `hunk_idx` (clamped), set `jump_to_row`.
  - `ARROW_UP` button (size 12, 22×22). Click → decrement, set jump.
  - Counter label: `"{n+1} / {total}"` or `"- / {total}"` (size 11, MUTED, monospace, 6px space before). (Note: laid R-to-L so order on screen reads counter, up, down.)
- 4px space → `ui.separator()`.

**Error banner** (only if `tab.error` set): horizontal row, 8px indent, `WARNING` glyph + message in DEL_FG size 11 monospace; right-anchored `X` `small_button` ("Dismiss" hover) clears `tab.error`. 2px space after.

**Scroll body** — `ScrollArea::both`, `auto_shrink([false;2])`, `ScrollBarVisibility::AlwaysVisible`, rendered via `show_rows(row_h, rows.len(), …)` (virtualized). `row_h = ceil(font_size * 1.25)`. Item spacing y forced to 0. Optional `vertical_scroll_offset(jump_y)` where `jump_y = max(0, jump_row*row_h - 2*row_h)`.

Per-row layout (left→right), font = `FontId::new(font_size, Monospace)`, `char_w` measured from "0":
- **Stage-button gutter** `stage_btn_w = 28.0`
- **Old line-number gutter** `gutter_old_w = char_w*ldigits + 10`, right-aligned text, MUTED
- **New line-number gutter** `gutter_new_w = char_w*rdigits + 10`, right-aligned, MUTED
- **Sign column** `sign_w = char_w*2 + 8`, centered `-`/`+`/` ` in `sign_fg` (DEL_FG/ADD_FG/CTX_FG)
- **Content**: syntect-highlighted galley. Each row gets a fresh `HighlightLines`; segments built into a `LayoutJob`; syntect color used unless alpha==0 (then CTX_FG). The total row width is computed (including the laid-out content galley width) then `allocate_exact_size((total_w, row_h), Sense::hover())`.

Row background (`rect_filled`, corner 0): ADD_BG / DEL_BG / transparent — but the bg rect **excludes** the 28px stage gutter (`bg_rect` starts at `rect.min.x + stage_btn_w`).

**Stage-hunk affordance** (only at `is_hunk_start` rows whose hunk has a patch):
- Interaction registered *before* painting via `ui.interact(btn_rect, btn_id, Sense::click())` where `btn_rect` = `(cursor.min, (28, row_h))`, id = `("stage_hunk", left_path, right_path, hi)`.
- Hover → `PointingHand` cursor + hover text "Stage this hunk" / "Unstage this hunk".
- Click → `git::stage_hunk` / `git::unstage_hunk`; on Ok set `pending_hunk_stage = true` + insert temp bool at id `("diff_hunk_staged", left, right)`; on Err set `tab.error = "{verb} hunk failed: {e}"`.
- Four visual states (glyph size in Proportional family):
  - unstaged idle → `CIRCLE`, color `theme.text_muted`, size 16
  - unstaged hover → `CHECK_CIRCLE`, ADD_FG, size 16, with a filled `ADD_BG` disc (radius 0.42*row_h) behind it
  - staged idle → `CHECK_CIRCLE`, ADD_FG, size 16
  - staged hover → `CHECK_CIRCLE`, ADD_FG, size 18, with disc
- **Connector line**: for rows in `row_in_shared_group` (or the anchor whose next row is in-group), draw a 1.5px vertical line at `x = rect.min.x + 14` in ADD_FG @ alpha 90, from top (`rect.min.y` if in-group, else `rect.center().y`) to `rect.max.y`.

**Minimap** (right edge, width 10): painted over the scroll area's `inner_rect`. For each non-Equal tag, a marker `rect_filled` (corner 1) at `y = min.y + i*track_h/total`, height `max(2, track_h/total)`, x inset 1px, width 8 — ADD_FG for inserts, DEL_FG for deletes. Interaction: `ui.interact(minimap_rect, id.with("diff_minimap"), click_and_drag)`. Hover → PointingHand. Click/drag → compute fraction → store pending scroll offset in temp data `("diff_pending_jump", hunk_state_id)`, request repaint.

**Hunk-counter sync**: when not jumping, derive `hunk_idx` from current scroll top row (`offset.y/row_h` rounded, +2 probe), `rposition` in hunk_starts. Persisted to temp data `hunk_state_id`.

#### 1.5 State / persistence

State lives in egui temp data keyed by `(label, left_path, right_path)`: `diff_hunk_staged` (refresh flag), `diff_hunk_idx` (`Option<usize>`), `diff_pending_jump`. Diff result lives in `DiffTabData.computed` as `Arc`. Syntax theme via `resolve_theme()`: tries `theme.syntax_theme` name, falls back to InspiredGithub (light bg) or OneHalfDark (dark bg) then first available.

#### 1.6 warpui port approach

- **Compute**: port `compute_diff` verbatim onto warpui's background job pool; keep the `Arc<DiffComputed>` + `inputs_version` cache contract. Shell `git diff` / `git apply --cached --check` identically.
- **Rendering**: this is painter-driven, not widget-driven, so it ports cleanly — replicate per-row `allocate_exact_size` + `painter.text/galley/rect_filled`. Keep the hardcoded ADD/DEL/CTX/MUTED colors as warpui constants (do NOT substitute theme tokens — the diff colors are intentionally fixed).
- **Stage buttons / minimap**: use warpui's hit-Rect-on-top idiom — allocate the interact rect with a stable id, then `dispatch_typed_action(StageHunk{repo, patch})` / `UnstageHunk` on click and `ctx.notify` on success to trigger a diff refresh. Icons = phosphor `Text` (`CIRCLE`, `CHECK_CIRCLE`, `ARROW_UP`, `ARROW_DOWN`, `WARNING`, `X`).
- **Syntax highlighting**: reuse warpui's syntect glue (`find_syntax_for_ext`, `syntaxes`, `themes`, `fallback_theme`).
- **Port status: ~10%.** Diff data model + compute likely not yet ported; minimap, connector lines, and 4-state stage glyph are bespoke and unported. This is the highest-effort area of the four.

---

### 2. Markdown Pane (`src/views/markdown_view.rs`)

A pulldown-cmark → egui `LayoutJob` renderer. Stateless rendering; the pane carries a path + content buffer.

#### 2.1 Public struct

**`MarkdownPane`** (from `state::layout`): `input_buf: String` (path entry), `content: String`, `path: String`, `error: Option<String>`.

#### 2.2 Colors (hardcoded in `render_md`, not theme tokens)
- `fg = rgb(210,214,224)` — body text
- `dim = rgb(140,146,160)` — blockquote marker
- `accent = rgb(120,170,230)` — headings, list bullets
- `code_fg = rgb(210,180,120)` — inline + block code text
- `code_bg = rgb(28,32,44)` — inline code background
- Bold simulated as `Color32::WHITE` (shipped proportional font has no bold face); italic honored natively via `TextFormat.italics`.
- Error label color `rgb(220,100,100)`.

#### 2.3 Rendered UI

`render(ui, pane, font_size, title)`:
- **Path bar** (`ui.horizontal`): `"Path:"` label + `text_edit_singleline(&mut input_buf)` + `"Load"` button. Load triggers on button click OR (lost_focus + Enter). `load_md` reads the file, sets content/path/error, and sets `*title` to the filename.
- If `error` → `colored_label(rgb(220,100,100), err)` and return.
- If `content` empty → "Enter a markdown path and press Load." and return.
- Else `ScrollArea::vertical` (auto_shrink off) → `render_md`.

`render_md(ui, src, font_size)` — single shared `LayoutJob` per block (paragraph/list-item/heading/blockquote) so a run wraps as one continuous line (avoids `item_spacing` gaps from smart-punctuation splitting text into many events). `mono`/`prop` fonts at `font_size`. Event handling:
- **Heading start/end**: flush; set/clear `heading` level; 6px space after.
- **Emphasis/Strong**: toggle italic/bold flags.
- **CodeBlock**: flush, set `in_code_block`; on text, render each line as its own `ui.label(RichText.font(mono).color(code_fg))`; 4px space after.
- **List/Item**: flush on list start; `"•  "` bullet in accent (prop) at item start; flush at item end; 4px after list.
- **Paragraph end**: flush + 4px.
- **BlockQuote**: `"▍ "` marker in dim (prop) at start; flush at end.
- **Inline Code**: mono, code_fg on code_bg background.
- **Text**: if heading, scale font (H1 1.8, H2 1.5, H3 1.3, else 1.15) in accent; else `plain_fmt` (WHITE if bold else fg, italics honored).
- **Soft/HardBreak**: append a space (prop, fg).
- **Rule**: flush + `ui.separator()`.

#### 2.4 Notable issue for the port
The bullet `"•"` (U+2022) and blockquote `"▍"` (U+258D) are **literal Unicode glyphs** — this directly violates the project's "NEVER use Unicode glyphs" rule and will tofu-box on fonts lacking those ranges. In warpui, replace with phosphor equivalents (e.g. a small `CIRCLE`/`DOT` for bullets, a painted vertical rule for blockquotes) or a bundled font that covers them. Flag this on port.

#### 2.5 warpui port approach
- Port `render_md` as a pure function taking warpui's `Ui` equivalent. Keep the single-`LayoutJob`-per-block strategy — it's load-bearing for spacing.
- Path bar: reuse a text-input widget + button; dispatch a `LoadMarkdown{path}` action rather than reading the file inline if file IO must be off the UI thread (current code reads synchronously — acceptable for small md files).
- **Port status: ~20%.** The cmark event loop is self-contained and easy to port; main work is wiring `MarkdownPane` state and replacing the two Unicode glyphs.

---

### 3. Welcome Pane (`src/views/welcome_view.rs`)

A stateless landing page: logo + wordmark + subtitle + three action buttons + a shortcut cheat-sheet. Fully painter-driven with hand-computed centering (egui layout inheritance pushes top-left, so all geometry is explicit math).

#### 3.1 Public enum / signature

**`WelcomeAction`**: `OpenTerminal` | `OpenBrowser` | `ToggleFilesPanel`. `render(ui) -> Option<WelcomeAction>` — returns what was clicked; the caller (main.rs) translates into a `PaneAction` because handlers need `ctx`/`App`.

#### 3.2 Layout constants & geometry
`LOGO_H=84, LOGO_W=82` (crane.png 800×820 aspect), `GAP_LOGO=16, TITLE_H=44, SUBTITLE_H=22, BUTTONS_H=96, SHORTCUTS_H=180, GAP_TITLE=6, GAP_BLOCK=28`. `total_h` = sum. `top_y = rect.min.y + max(20, (height-total_h)*0.42)` (sits slightly above center). `content_w = min(rect.width, 620)`, centered.

#### 3.3 Rendered elements
- **Background**: full-rect `rect_filled` in `theme.surface` (prevents transparent-tint inheritance).
- **Logo**: `painter.image` of cached `crane.png` texture (`crane_welcome_logo`, decoded once via `include_bytes!`, stored in temp data), 82×84, centered above wordmark, full UV, tint WHITE.
- **Title** "Crane": Proportional size 34, `theme.text`, centered.
- **Subtitle** "Pick a surface to begin, or use a shortcut below.": Proportional 13, `theme.text_muted`, centered.
- **Three buttons** (170w × 96h, 16px gap, row centered): each `welcome_button(rect, glyph, label, hint, chord)`:
  - Terminal → `TERMINAL_WINDOW`, "Spawn a shell in this pane", "⌘ T splits", `OpenTerminal`
  - Browser → `CUBE`, "Embedded WebKit tab", "⌥ ⌘ T new tab", `OpenBrowser`
  - Files → `FOLDER_OPEN`, "Show the workspace tree", "⌘ /  toggles", `ToggleFilesPanel`
- **Shortcut cheat-sheet** (`draw_shortcuts`): header "SHORTCUTS" (Proportional 10.5, rgb(140,146,162)) at top-left; then 10 chord/desc pairs in a 2-column grid (col_w = half width, row_h 18, chord_w 110), chords in Monospace 11.5 `theme.accent`, descriptions in Proportional 11.5 `theme.text_muted`. Chords use literal `⌘ ⇧ ⌥` glyphs.

#### 3.4 `welcome_button` states & interaction
`ui.interact(rect, Id::new(("welcome_btn", label)), Sense::click())`. Painter-drawn:
- idle bg `theme.topbar_bg`, border `theme.inactive_border`
- hover bg `theme.row_hover`, border `theme.accent`, cursor `PointingHand`
- corner radius 8, 1px stroke (Inside)
- glyph (Proportional 24, `theme.accent`) at y+26; label (Proportional 14, `theme.text`) at y+56; hint (Proportional 11, `theme.text_muted`) at y+74; chord (Monospace 10.5, `theme.text_muted`) at bottom-14.
- returns `resp.clicked()`.

#### 3.5 warpui port approach
- Port `render` returning `Option<WelcomeAction>`; caller maps to warpui actions (`dispatch_typed_action`). All geometry is explicit math — ports directly.
- Buttons: hit-Rect-on-top via `interact` + painter; icons = phosphor `Text`. Theme tokens (`surface`, `text`, `text_muted`, `accent`, `topbar_bg`, `inactive_border`, `row_hover`) map to warpui's theme.
- Logo: cache the decoded texture once in warpui's texture store.
- Note: the `⌘/⇧/⌥` chord glyphs are intentional and render fine in monospace on macOS.
- **Port status: ~25%.** Self-contained, no IO, no off-thread work — among the easiest to port; mostly geometry transcription + theme-token mapping.

---

### 4. PDF Pane (`src/views/pdf_view.rs`)

A pdfium-backed PDF viewer rendered as a **file tab inside the Files Pane** (not a top-level Pane). Continuous vertical scroll of page textures, zoom presets, single-page text selection + copy, external-open fallback.

#### 4.1 Public structs

**`PdfTabState`**: `path: PathBuf`, `doc: Option<PdfDocument<'static>>`, `page_count`, `current_page`, `zoom: f32`, `texture_cache: HashMap<(usize, u32), TextureHandle>` (key = page_idx + zoom bucket `(zoom*100)`), `page_text: HashMap<usize, PageText>` (lazy, for hit-testing), `page_size_pts: Vec<(f32,f32)>` (points = 1/72"), `selection: Option<Selection>`, `drag: Option<(usize, Pos2)>`, `error: Option<String>`. `new(path)` constructs + calls `try_load`.

**`Selection`**: `page`, `start_char`, `end_char` (usize char indices into the page's char list).
**`PageText`**: `chars: Vec<CharInfo>`.
**`CharInfo`**: `unicode: char`, `left/right/bottom/top: f32` (PDF points, bottom-left origin).

#### 4.2 Constants & FFI binding
`ZOOM_PRESETS = [0.50,0.75,1.00,1.25,1.50,2.00,3.00,4.00]`, `DEFAULT_ZOOM=1.00`, `PAGE_GAP=12.0`, `TEXTURE_KEEP_RADIUS=5`. `get_pdfium()` lazy `OnceLock`: tries bundled `../Frameworks/libpdfium.dylib`, then dev `vendor/pdfium/<arch>/libpdfium.dylib`, then system library; fails closed → error string → Open-Externally-only UI. `is_pdf_path` routes here from `file_view`.

#### 4.3 Rendered UI

`render_pdf(ui, state)`:
- **Toolbar** (`render_toolbar`, `ui.horizontal`): all buttons disabled unless `error.is_none() && page_count>0`.
  - `CARET_LEFT` prev (size 13, min 28×24) → `current_page -= 1`
  - page indicator `"{n} / {total}"` (size 11.5, `theme.text`) or `"—"` (`theme.text_muted`)
  - `CARET_RIGHT` next (13, 28×24) → `current_page += 1`
  - 8px space
  - `MINUS` zoom-out (13, 28×24) → `step_zoom(-1)`
  - zoom % label `"{n}%"` (11.5, `theme.text`)
  - `PLUS` zoom-in (13, 28×24) → `step_zoom(+1)`
  - right-anchored: `ARROW_SQUARE_OUT` + "Open Externally" button (11.5, min h24) → `open_externally`
- 4px space.
- **Error panel** (`render_error_panel`, if error): 40px space, vertically centered — message (13, `theme.text`) + hint "Use \"Open Externally\" above…" (11, `theme.text_muted`).
- If `doc.is_none()` → "Loading...".
- `handle_keyboard`.
- **Scroll body**: `ScrollArea::both`, id_salt `("pdf_scroll", path)`, auto_shrink off, max_height = available. Inner layout `top_down(Align::Center)` (centers pages narrower than pane). For each page: `render_page` + `PAGE_GAP` space. Then `evict_textures`.

**`render_page`**: scale = `96/72 * zoom` (render at 96 DPI base). Lazy-render texture into cache (`render_page_to_texture` via pdfium `render_with_config` → rgba → `load_texture`, name `crane_pdf:{path}:{page}:{width}`, LINEAR). `allocate_exact_size(display_size, click_and_drag)`. Paints placeholder bg (`rgb(40,40,44)` dark / `rgb(245,245,248)` light, corner 2), then the texture image, then a 1px `theme.border` frame (corner 2). Then `handle_selection`, `paint_selection_for_page`. Tracks `current_page` = topmost page intersecting viewport above its center.

#### 4.4 Interaction & states
- **Text selection (drag)**: `handle_selection` — on `drag_started`, hit-test char under pointer (`screen_to_pdf_pts` → `char_hit_test`, which does exact-bbox then nearest-center fallback), set `drag` + start selection; on `dragged` (same page), update `end_char`; on `drag_stopped`, clear `drag`. Selection highlight = `rgba(80,140,240,80)` filled rects per char (corner 1), flipping PDF bottom-left coords to egui top-left.
- **Keyboard** (`handle_keyboard`): PageDown/PageUp → ±page; Home → page 0; End → last page; Cmd+= / Cmd+Plus → zoom in; Cmd+- → zoom out; Cmd+0 → reset zoom; Cmd+C → copy `selection_text`; Cmd+A → select all chars on current page (lazy-loads page text).
- **Texture eviction**: when cache >16, retain only pages within `TEXTURE_KEEP_RADIUS` (5) of `current_page`; same for `page_text`.

#### 4.5 warpui port approach
- **FFI**: pdfium binding (`get_pdfium` OnceLock + candidate paths) ports directly; keep the bundled→dev→system fallback chain. Texture render → warpui texture store.
- **Selection**: hit-test math (`screen_to_pdf_pts`, `char_hit_test`, coordinate flip) is pure and ports verbatim. Drag/keyboard via warpui's input + `interact` on the page rect.
- **Toolbar/icons**: phosphor `Text` (`CARET_LEFT/RIGHT`, `MINUS`, `PLUS`, `ARROW_SQUARE_OUT`); button-enabled gating identical. Open Externally → `dispatch_typed_action(OpenExternally{path})` or direct `Command::spawn`.
- **Theme tokens**: `theme.text`, `text_muted`, `border`, `is_dark()` for the page placeholder.
- **Port status: ~5%.** Heaviest external dependency (pdfium FFI + texture lifecycle + selection geometry); almost certainly unported. The pure-math selection/hit-test helpers are the easy 30%; the FFI binding, lazy texture cache, and eviction are the bulk.

---

### Cross-cutting notes for all four

- **None use `dispatch_typed_action`/`ctx.notify` today** — they mutate `&mut` state directly (Crane is single-process egui). In warpui, every click/keyboard handler above should be converted to a typed-action dispatch where the action needs parent context (terminal spawn, file load, hunk stage, external open) per the welcome-view's own comment that "dispatch is lifted out … because handlers need the parent Context/App."
- **All icons are phosphor** except Markdown's two stray Unicode glyphs (bullet, blockquote bar) — fix on port.
- **Hardcoded diff/markdown colors** are intentional and should be ported as constants, not remapped to theme tokens. Welcome and PDF correctly use theme tokens.
- **Heavy work is already off-thread / cached** in Diff (JobSystem) and PDF (lazy texture + eviction); preserve those contracts to avoid UI-thread stalls in warpui.

Relevant file paths: `/Users/rajpootathar/ideaProjects/crane/src/views/diff_view.rs`, `/Users/rajpootathar/ideaProjects/crane/src/views/markdown_view.rs`, `/Users/rajpootathar/ideaProjects/crane/src/views/welcome_view.rs`, `/Users/rajpootathar/ideaProjects/crane/src/views/pdf_view.rs`.


---


<!-- ===== browser ===== -->

## Browser Pane (wry WKWebView overlay)

This area implements a Chromium-free embedded web browser inside a Pane. It is split across three files: `src/browser/mod.rs` (the native WKWebView host + thread-local egui↔native bridge), `src/browser/memory.rs` (on-demand WebKit RSS poller), and `src/views/browser_view.rs` (the egui-drawn chrome: tab strip + URL toolbar + footer status bar + the rect reserved for the native webview). The native webview is a wry `build_as_child` WKWebView parented under the main NSWindow content view — it sits **above** egui's GPU surface in the OS compositor. Everything egui draws (tab strip, toolbar, footer) lives in narrow horizontal bands; the large middle rect is left to the native overlay.

### Architecture model (critical for the port)

The egui side and the native side communicate every frame through a **thread-local `Bridge`** (`browser::BRIDGE`), not direct calls. Per frame:

1. `browser_view::render` runs inside the layout walk. It does NOT touch any WKWebView. It only:
   - Draws the egui chrome.
   - Pushes intents into the thread-local bridge via free functions: `report_pane(key, rect, url)` (active tab — resize + show), `report_inactive(key, url)` (background tabs — keep alive but hidden), `report_focused_pane(key)` (this pane owns focus, route Cmd+C/V/X/A to it), and `queue_action(key, Action)` for nav button clicks.
2. After the full egui frame, `main.rs` calls `take_bridge()` then `BrowserHost::sync(window, ctx, bridge, hide_all, all_keys)`, which reconciles the live set of native WKWebViews against the bridge: builds new slots, drops gone ones, resizes/shows/hides, applies queued actions, and tells `mac_keys` which view is focused.
3. wry callbacks fire on background threads and push into shared `Arc<Mutex<…>>` maps (`loading`, `url_updates`); `main.rs` drains those each frame and applies them back to tab state, also pushing a `LOADING_SNAPSHOT` thread-local that `browser_view` reads to render spinners.

The key identity is `SlotKey = (PaneId, u32)` — pane id + per-pane tab id. Multiple tabs per pane each own a persistent WKWebView keyed by SlotKey; switching tabs hides/shows rather than rebuilds, so page state (forms, scroll, auth, audio) survives.

---

### Public types

**`browser::SlotKey = (PaneId, u32)`** — composite key: owning pane + per-pane browser-tab id. Used everywhere as the native webview store key. (`composite_id(pane_id, tab_id)` is just a tuple constructor.)

**`browser::Action`** (`#[derive(Debug, Clone)]` enum) — a queued nav intent for a webview:
- `Load(String)` — navigate to URL (empty string is a no-op).
- `Reload` — `webview.reload()`.
- `Back` — runs JS `window.history.back()`.
- `Forward` — runs JS `window.history.forward()`.
- `Close` — destroy the webview now (tab closed, pane still alive). Folded out before resize/reload in `sync`.

**`browser::Bridge`** (`#[derive(Default)]`) — per-frame egui→native handoff, all `pub`:
- `alive: Vec<(SlotKey, egui::Rect, String)>` — active tab of each visible pane; webview resized + shown, loads URL if changed.
- `inactive: Vec<(SlotKey, String)>` — other tabs; webview kept but hidden.
- `actions: Vec<(SlotKey, Action)>` — per-key nav intents queued by clicks this frame.
- `focused: Option<SlotKey>` — active-tab slot of the focused Browser pane; consumed by `sync` → `mac_keys` for clipboard routing.

**`browser::BrowserHost`** — owns all native state (lives in `App`, one instance):
- `slots: HashMap<SlotKey, Slot>` — live WKWebViews.
- `pending: HashMap<SlotKey, Vec<Action>>` — actions buffered until the slot exists / is resized.
- `loading: LoadingSet = Arc<Mutex<HashSet<SlotKey>>>` — tabs currently loading; written by wry page-load callbacks (bg thread), read on egui thread for spinners.
- `url_updates: UrlUpdates = Arc<Mutex<HashMap<SlotKey, String>>>` — latest WKWebView-reported URL per tab; drained per frame to sync the URL bar.
- `pub memory: memory::Monitor`.

`Slot` (private) — `{ webview: wry::WebView, loaded_url: String }`. `loaded_url` is the dedupe guard: `sync` only calls `load_url` when `tab.url != slot.loaded_url`, preventing SPA-route reloads.

**`browser::memory::Snapshot`** (`#[derive(Clone, Default)]`) — `{ total_bytes: u64, process_count: u32 }`. Sum of RSS of all `com.apple.WebKit.WebContent` processes (not per-tab — wry can't attribute).

**`browser::memory::Monitor`** — `{ cache: Mutex<Cached> }` where `Cached { snap: Snapshot, at: Option<Instant> }`. `snapshot()` returns the cache unless `POLL_INTERVAL` (3s) elapsed, then samples inline via `ps -axo rss=,comm=` (~5ms, no background thread). Constants: `WARN_BYTES = 1_000_000_000` (1.0 GB), `DANGER_BYTES = 2_000_000_000` (2.0 GB), `POLL_INTERVAL = 3s`. `human_bytes(u64)` → `"X.XX GB"`/`"N MB"`/`"N KB"`/`"N B"`.

**Thread-locals (in `mod.rs`):** `BRIDGE: RefCell<Bridge>`, `LOADING_SNAPSHOT: RefCell<HashSet<SlotKey>>` (per-frame snapshot of loading tabs, read for spinners), `MEMORY_SNAPSHOT: RefCell<memory::Snapshot>` (per-frame snapshot for the footer). Setters/getters: `set_loading_snapshot`, `is_loading(key) -> bool`, `set_memory_snapshot`, `memory_snapshot()`.

**Free functions for `main.rs` orchestration:** `collect_all_keys(app) -> HashSet<SlotKey>` (walks every project→workspace→tab→layout→Browser pane→its tabs; defines which webviews stay alive even on inactive workspace tabs), `apply_url_updates_to_app(app, &updates)` (writes WKWebView-reported URLs back to `btab.url` + `btab.input_buf`, skipping empty/identical).

---

### Rendered UI elements (top to bottom inside the pane)

All colors are `theme::current().<token>.to_color32()`. Font is egui Proportional unless noted. All icons are `egui_phosphor::regular`.

**1. Tab strip** — `ui.horizontal`, `item_spacing.x = 2.0`. One chip per `pane.tabs`, then a trailing `+` button.

Each **tab chip** is an `egui::Frame`:
- fill: active → `surface`; inactive → `topbar_bg`.
- stroke: 1.0px, active → `focus_border`; inactive → `inactive_border`.
- `corner_radius: 4`, `inner_margin: symmetric(8, 3)`.
- Inner `ui.horizontal` contains:
  - **Loading spinner** (only when `is_loading(key)`): the `ARROW_CLOCKWISE` glyph laid out at `FontId(11.0, Proportional)` in `accent` color, centered in an allocated `14×14` rect (`Sense::hover`). NOTE: rotation is computed (`angle = (time*3.0) % TAU`) but **not applied** — egui lacks galley rotation, so it renders as a static accent-colored glyph; the color change is the only loading signal. Port can do a real rotation since most renderers support it.
  - **Title label**: `Label::new(RichText(short_title).size(11.5).color(text)).sense(Sense::click())`. Hover → `CursorIcon::PointingHand`. Click → activate that tab (`pane.active = idx`).
  - **Close ×** (only when `pane.tabs.len() > 1`): `Button(RichText(X).size(10.0)).frame(false).min_size(14×14)`. Click → queue `Action::Close` for that tab's slot, then `pane.close_tab(idx)`.

`short_title(tab)`: if `title` non-empty and `!= url` → truncate(title, 18); else if url empty → `"New Tab"`; else strip `https://`/`http://`, take host before first `/`, truncate to 18 (ellipsis `…` appended).

**New-tab `+` button**: `Button(RichText(PLUS).size(12.0)).frame(false).min_size(22×22)`, hover text `"New tab"`. Click → `pane.new_tab()`.

**2. URL toolbar** — `ui.horizontal`, `item_spacing.x = 4.0` (only for the active tab). Three nav buttons via a local `btn` closure (`Button(RichText(glyph).size(13.0)).frame(false).min_size(24×22)` + `on_hover_text`):
- `ARROW_LEFT` "Back" → queue `Action::Back`.
- `ARROW_RIGHT` "Forward" → queue `Action::Forward`.
- `ARROW_CLOCKWISE` "Reload" → queue `Action::Reload`.

Then the **URL field**: `TextEdit::singleline(&mut tab.input_buf)`, `hint_text("https://…")`, `desired_width = (available_width - 90.0).max(80.0)`. Then a **"Go"** plain text button. Submit fires on `Go.clicked()` OR (`resp.lost_focus()` && Enter pressed). On submit: `normalize_url(input.trim())`; if non-empty set `tab.url`, `tab.title`, and the pane `title` to the URL, then queue `Action::Load(url)`. (On non-mac/linux, opens the system browser via `webbrowser::open` instead.)

Then the **open-in-system-browser** button: plain `ui.button(ARROW_SQUARE_OUT)`, hover text `"Open in system browser"`. Click (with non-empty `tab.url`) → `webbrowser::open(&tab.url)`.

`ui.add_space(2.0)`.

**3. Webview rect** — `const FOOTER_H = 22.0`. `full = available_rect_before_wrap()`. `rect` = full minus the bottom 22px (the webview area); `footer_rect` = the bottom 22px band. `ui.allocate_rect(rect, Sense::hover())`. On mac/linux: `inner = rect.shrink(1.0)`; if `native_hidden` → `report_inactive(key, url)` (hides the webview for the frame, e.g. when this pane is a drag-drop target and the blue overlay must show above it); else `report_pane(key, inner, url)` and, if `is_focus`, `report_focused_pane(key)`. The whole `rect` is then painted `surface` (placeholder beneath/around the native overlay). Finally every non-active tab is reported via `report_inactive` (done after the active tab so focus wins).

On non-mac/linux: a centered launcher card — `text_muted` label `"Embedded browser not available on this platform yet."`, then either the URL (monospace 12.5 `text`) + a `min_size 220×30` `"{ARROW_SQUARE_OUT}  Open in system browser"` button, or an italic `text_muted` `"Type a URL above and press Enter."`.

**4. Footer status bar** (mac/linux) — fills `footer_rect` with `topbar_bg`, with a 1px `divider` line across its top. Two texts at `FontId(12.5, Proportional)`, vertically centered, 10px inset:
- **Left** (`Align2::LEFT_CENTER`, `text_muted`): `"{n} tab"` / `"{n} tabs"`.
- **Right** (`Align2::RIGHT_CENTER`): memory + process count. Color/label by `Snapshot.total_bytes`:
  - `0` → `text_muted`, label `"WebKit memory: —"`.
  - `>= DANGER_BYTES` → `error` color, `"{human_bytes} (heavy — close tabs)"`.
  - `>= WARN_BYTES` → `warning` color, `human_bytes`.
  - else → `text_muted`, `human_bytes`.
  - Non-zero right label format: `"WebKit: {mem_label}  ·  {count} process{es}"` (pluralize `process`/`processes`).
- A hover hitbox over the whole footer (`ui.interact(footer_rect, Id::new(("browser_footer_mem", pane_id)), Sense::hover())`); when `total_bytes > 0`, tooltip explains WebKit usage is summed across ALL Browser panes/tabs (no per-tab attribution) and to close tabs to free memory.

---

### URL normalization (`normalize_url`, pure — port verbatim)

- empty → empty.
- already `http://` / `https://` / `about:` → unchanged.
- `is_local_host` (loopback / RFC1918: `localhost`, `0.0.0.0`, `[::1]`, `[::]`, `127.*`, `192.168.*`, `10.*`, `172.16–31.*`) → prefix `http://`.
- `host:port` with numeric port `!= 443` → prefix `http://`.
- no `.` and no `/` → `https://duckduckgo.com/?q={urlencode(raw)}` (DuckDuckGo search).
- else → prefix `https://`.

`urlencode` is a manual RFC3986-unreserved passthrough (`A-Za-z0-9-_.~`), everything else `%XX`. `is_local_host` splits host off any `:port`/`/path` before matching.

---

### Interactions summary

| Element | Event | Effect |
|---|---|---|
| Tab title | click | `pane.active = idx` |
| Tab title | hover | `PointingHand` cursor |
| Tab close × | click | queue `Action::Close`, `pane.close_tab(idx)` |
| `+` | click | `pane.new_tab()` |
| Back/Forward/Reload | click | queue respective `Action` |
| URL field | Enter (on lost focus) | submit |
| Go | click | submit (normalize → set tab.url/title/pane title → `Action::Load`) |
| Open-in-system | click | `webbrowser::open(tab.url)` |
| Footer | hover | memory explanation tooltip |
| Spinner | — | shown while `is_loading(key)`, static accent glyph |

States: tab chip active/inactive (fill+stroke swap); loading (spinner present); `pane.tabs.len() <= 1` disables the close × (omitted). No drag, no right-click menu on the egui chrome (wry's native right-click "Inspect Element" is explicitly disabled via `.with_devtools(false)`). The webview is hidden (not destroyed) when: a global overlay is up (`hide_all` in `sync`), this pane is a drop target (`native_hidden`), or the pane's workspace tab isn't active (not reported this frame).

---

### wry WebView build (`build_slot`) — native, NOT portable to most warpui backends

`WebViewBuilder::new().with_bounds(rect).with_url(url or "about:blank").with_transparent(false).with_devtools(false)` plus:
- `with_on_page_load_handler`: `Started` → insert into `loading` + push url; `Finished` → remove from `loading` + push url; always `ctx.request_repaint()`.
- `with_navigation_handler`: push url, repaint, return `true` (catches full-page loads/redirects).
- `with_initialization_script`: a JS shim (idempotent via `window.__craneNavHooked`) that monkeypatches `history.pushState`/`replaceState`, listens for `popstate`/`hashchange`, and posts `"crane-url:" + location.href` over `window.ipc.postMessage` — needed because WKWebView doesn't surface SPA route changes to the nav delegate.
- `with_ipc_handler`: strips `"crane-url:"` prefix, pushes to `url_updates`, repaints.
- `build_as_child(window)` parents under NSWindow; new slots start `set_visible(false)`.

`release_webview_memory(webview)` (called before drop / on Close): evaluates JS to `sessionStorage.clear()` / `localStorage.clear()` / `caches.delete(...)`, then `load_url("about:blank")`, then `set_visible(false)` — shrinks per-tab WebContent footprint immediately. `to_wry_rect` / `egui_placeholder_rect` convert egui rects to wry `LogicalPosition`/`LogicalSize`.

`BrowserHost::sync` reconciliation order: (1) drop slots whose key ∉ `all_keys` (after `release_webview_memory`); (2) fold `Close` actions, else mark `Load`/`Reload` as loading immediately (so the spinner appears before WKWebView's async `Started`) and buffer into `pending`; (3) if `hide_all`, hide everything + clear mac_keys focus, return; (4) hide any slot not reported this frame; (5) build+hide inactive tabs eagerly (so first switch is instant); (6) for each alive: show, always `set_bounds` (the "only-on-change" short-circuit caused stale frames in DMG builds), `load_url` if `loaded_url != url`, drain pending; (7) set `mac_keys` focused webview by extracting a raw Obj-C pointer across the objc2 0.5/0.6 version boundary (wry 0.55 ships objc2 0.6, `mac_keys` uses 0.5; retain/release are ABI-stable).

`is_idle()` (`slots.is_empty()`) lets `main.rs` short-circuit the per-frame browser pump (memory poll, loading snapshot, URL drain) when no Browser pane has materialized a webview.

---

### warpui port approach

The egui chrome ports 1:1 with the established reuse patterns:
- **Tab chips / buttons / labels**: hit-Rect-on-top + `dispatch_typed_action` + `ctx.notify`. Each chip is a rounded rect (fill/stroke per active state from theme tokens), a phosphor `Text` glyph for the spinner/close/plus, and a clickable label. Click on title → dispatch an `ActivateBrowserTab { pane_id, idx }`; close × → `CloseBrowserTab`; `+` → `NewBrowserTab`.
- **URL toolbar**: phosphor glyph buttons (`ARROW_LEFT`/`ARROW_RIGHT`/`ARROW_CLOCKWISE`/`ARROW_SQUARE_OUT`) dispatching `BrowserNav(Back/Forward/Reload)`; a text input bound to `input_buf`; Enter/Go dispatch `BrowserGo`. Reuse the existing text-input widget + the same `normalize_url` (copy the pure functions verbatim — they have no egui deps).
- **Footer**: two `Text` draws + a hover hitbox for the tooltip; color tokens `text_muted`/`warning`/`error`. The memory poller (`ps` parse, `human_bytes`, thresholds) is pure and ports verbatim.
- **Recursive Node tree**: the Browser pane is one `PaneContent::Browser(BrowserPane)` leaf; `collect_all_keys` walks the same project→workspace→tab→layout tree warpui already has.
- **The native overlay is the hard part and does NOT port via the egui-chrome patterns.** It requires a real native-webview host parented to warpui's window, plus the per-frame bridge (`report_pane`/`report_inactive`/`report_focused_pane`/`queue_action` → `take_bridge` → `sync`), the background-thread `Arc<Mutex>` url/loading maps, and macOS clipboard routing via `mac_keys`. This is platform glue, not UI, and must be reimplemented against warpui's windowing + compositor stack (z-ordering the native view above warpui's GPU surface, rect sync, show/hide on overlays/drag/inactive-tab, and the objc2 version-boundary pointer hack for Cmd+C/V/X/A).

---

### Current port-status: ~10%

Pure logic that ports verbatim (`normalize_url`, `is_local_host`, `urlencode`, `short_title`, `truncate`, `human_bytes`, the memory thresholds/poller, the `Action`/`SlotKey`/`Bridge`/`Snapshot` type shapes, `collect_all_keys`/`apply_url_updates_to_app` tree-walks) is straightforward and represents maybe 10% of the effort already-understood/reusable. The egui chrome (tab strip, URL toolbar, footer) is mechanical to re-draw with warpui's hit-Rect/dispatch/phosphor patterns but is not yet ported. The dominant, highest-risk work — the native WKWebView host (wry `build_as_child`, the per-frame bridge reconciliation in `sync`, background-thread URL/loading callbacks, SPA nav JS shim, memory-release on drop, and macOS clipboard routing across the objc2 version boundary) — is entirely unported and is OS/compositor-specific platform glue rather than reusable UI. Net: roughly 10% effectively in hand, ~90% remaining and concentrated in the native overlay.


---


<!-- ===== terminal ===== -->

## Terminal Pane

This section documents the Terminal Pane end-to-end for 1:1 reproduction in warpui. It covers the multi-tab terminal pane (`src/terminal/view.rs`), the PTY/Term backing model (`src/terminal/term.rs`), and the experimental wgpu sub-pass scaffold (`src/terminal/gpu_render.rs`). The Terminal Pane is the single most important Pane in Crane (it is the agent-CLI surface), and its rendering is a hand-rolled grid painter, NOT egui's text widgets.

### 1. Public data model

#### `Terminal` (term.rs) — one live PTY + parser + grid
The owning struct for a single shell session. Fields:
- `term: Arc<Mutex<CtTerm>>` — `crane_term::Term`. The grid, scrollback, cursor, mode bag (DECCKM / bracketed-paste / app-cursor), scroll region, selection, `dirty_epoch`, `display_offset`, `pty_replies` queue. This is the rendering source of truth.
- `parser: Arc<Mutex<CtProcessor>>` — `crane_term::Processor`, the VT byte→Handler dispatch loop with `?2026` sync buffer. Shared between the reader thread and pre-boot transcript replay so parser state is single-threaded-consistent.
- `writer: Arc<Mutex<Box<dyn Write + Send>>>` — PTY master writer. All keyboard/paste bytes go here.
- `cols: usize`, `rows: usize` — last-sized grid dims (seed 80×24, resized to viewport next frame).
- `cwd: std::path::PathBuf` — working dir the shell was spawned in; used for tab labels, path-link resolution, and spawning sibling tabs at the same cwd.
- `history: Arc<Mutex<Vec<u8>>>` — raw PTY byte log capped at `HISTORY_MAX = 256*1024`, oldest bytes drained when over cap. Retained for export; cleared on Cmd+K.
- `last_click: Option<(Instant, i32, usize)>` — (time, grid line, col) of the last click, for multi-click detection (500 ms, same cell).
- `click_count: u8` — 1=clear/place, 2=word(Semantic), 3=line(Lines).
- `master: Box<dyn MasterPty + Send>` — PTY master; resize + `as_raw_fd` for `tcgetpgrp`.
- `shell_pid: Option<u32>` — for foreground-process detection.
- `child: Option<Box<dyn portable_pty::Child + Send + Sync>>` — killed+waited on Drop (don't rely on SIGHUP).
- `pending_scroll_to_bottom: AtomicBool` — set by `write_input`, drained post-input by `flush_scroll_to_bottom` (snaps viewport to live screen on type). Deferred to avoid Context read/write-lock deadlock.
- `scroll_carry: Mutex<f32>` — fractional-row wheel-delta remainder carried across frames for sub-row-smooth scrolling.
- `alive: Arc<AtomicBool>` — false once reader thread hits EOF/err; UI polls `is_alive()` each frame and closes the Pane.

Key methods: `spawn(ctx,cols,rows,cwd)`, `spawn_with_text_history(...)`, `resize(cols,rows)` (no-op if unchanged; resizes master + `term`), `write_input(&[u8])`, `flush_pty_replies()`, `flush_scroll_to_bottom()`, `is_alive()`, `has_foreground_process()` (unix `tcgetpgrp != shell_pid`), `foreground_process_name()` (`ps -o comm= -p <pid>`), `foreground_is_cli_agent()` (allowlist: `claude codex aider opencode cursor-agent qodo goose`), `snapshot_ansi()` / `snapshot_text()` (session save), `history_snapshot()`.

`Drop`: kill+wait child, then `malloc_zone_pressure_relief` on macOS.

#### `TerminalPane` / `TerminalTab` (referenced from `state::layout`, not in these files)
- `TerminalPane { tabs: Vec<TerminalTab>, active: usize, renaming: Option<(usize, String)> }` — methods `add(Terminal)`, `close(idx)`, `active_terminal()`. `renaming` holds the in-progress rename (tab index + edit buffer), taken out for the duration of the strip render and put back.
- `TerminalTab { name: Option<String>, terminal: Terminal }` — `name=None` ⇒ label falls back to cwd basename, then `tab {n}`.

#### View-local types (view.rs)
- `struct UrlHit { col_start, col_end, url: String }` — `col_end` exclusive.
- `struct PathHit { col_start, col_end, path: PathBuf, line: Option<u32>, col: Option<u32> }` — only kept if `path.exists()`. line/col parsed from `:N[:M]` suffix, currently unused at click time.
- `enum HoveredKind<'a> { Url(&'a str), Path(&'a Path) }` — what's under the pointer.

#### `GpuRenderResources` / `GpuTerminalCallback` (gpu_render.rs)
- `GpuRenderResources { pipeline: wgpu::RenderPipeline, instance_buffer: wgpu::Buffer }` — stored in `egui_wgpu::Renderer::callback_resources` for app lifetime, built once (guarded by `INITIALIZED: AtomicBool`).
- `GpuTerminalCallback` — unit struct impl `CallbackTrait`; `paint` sets pipeline and `draw(0..6, 0..1)` (two triangles, fullscreen quad). **This is a scaffold only** — it paints a diagonal magenta→cyan gradient at 0.35 alpha, gated by `CRANE_GPU_TERM=1`. No real glyph rendering exists yet.

### 2. Rendered UI elements

#### 2a. Tab strip (always shown — mirrors Files Pane)
- Allocated `strip_height = 26.0`, full `available_width`, `Sense::hover`. New child Ui, `left_to_right(Center)`, `item_spacing.x = 2.0`, leading `add_space(4.0)`. Trailing `ui.add_space(2.0)` below strip.
- **Each tab chip** (`draw_terminal_tab`): font `FontId::new(11.5, Proportional)`; close glyph font 13.0. Width = `padding_x(8) + text_w + gap(5) + close_size(14) + padding_x(8) - 2`; height 22. `Sense::click_and_drag`. Rounded rect radius 5.
  - Background/foreground by state:
    - **Active**: bg = accent at alpha 55 (`Color32::from_rgba_unmultiplied(accent.r,g,b,55)`), fg = `theme.text`.
    - **Hover** (tab or close hovered): bg = `theme.row_hover`, fg = `theme.text`.
    - **Idle**: bg = transparent (not painted), fg = `theme.text_muted`.
  - Label text painted `LEFT_CENTER` at `min.x + padding_x`.
  - **Close button**: 14×14 rect pinned at `max.x - padding_x - close_size + 2`, vertically centered. Its own `ui.interact` with id `("term_tab_close", pane_id, idx)`, `Sense::click`. Glyph `egui_phosphor::regular::X`, `CENTER_CENTER`, color = tab fg. On hover: fill `close_rect.shrink(1.0)` radius 4 with `theme.error` (red). Cursor → PointingHand when tab or close hovered.
- **"+" button**: pinned right via `right_to_left(Center)`, leading `add_space(4.0)`, 22×22, `Sense::click`. Hover: fill radius 4 with `theme.row_hover` + PointingHand cursor. Glyph `egui_phosphor::regular::PLUS`, `CENTER_CENTER`, font 13.0, color `theme.text`.

#### 2b. Terminal grid (`render_terminal`)
- **Cell metrics**: `font_id = FontId::new(font_size, Monospace)`. `cell_h = fonts.row_height(font_id)`. `cell_w` = layout a 32-char "MMMM…" galley, divide width by 32 (matters: bare glyph advance drifts the cursor after ~25 cols). `pad_left = 2.0`.
- **Grid dims**: `cols = ((available.x - pad_left)/cell_w).floor().max(20)`, `rows = (available.y/cell_h).floor().max(5)`. `terminal.resize(cols,rows)` then `flush_pty_replies()`.
- **Painter**: `allocate_painter(cols*cell_w × rows*cell_h, click_and_drag ∪ focusable_noninteractive)`. `origin = rect.min + (pad_left,0)`. Background `rect_filled(rect, 0.0, theme.terminal_bg)`.
- **Cursor over grid**: I-beam (`CursorIcon::Text`) when hovered.
- **Cell paint** (the core loop): cells grouped by viewport line into `BTreeMap<i32, Vec<(col, CtCell, in_selection)>>`. Each row walked strictly `0..cols`, accumulating same-style runs into a `String` buf, flushed as one `LayoutJob` galley pinned to `row_x + run_start_col*col_stride` where `col_stride = cell_w.max(1.0)`. `row_y = (origin.y + line*cell_h + scroll_pixel_offset).round()`, `row_x = origin.x.round()`.
  - **Color resolution** (`color_to_egui`): `TermColor::Rgb`→direct; `Indexed(idx)`→`palette(idx)`; `Named(Foreground/Cursor)`→`theme.terminal_fg`, `Named(Background)`→`theme.terminal_bg`, named<16→palette, else fg/bg default. `palette()` is a hardcoded 16-color table (e.g. 0=`#1a1c28`, 1=`#cc5555`, 7=`#b0b4c0`, 15=`#dddee e`) + the standard 6×6×6 cube (`16..=231`) + grayscale ramp (`232..=255`).
  - **SGR flags**: `INVERSE` swaps fg/bg (with fallback so inverted default-bg text stays readable); `UNDERLINE` adds a 1px stroke of the fg color; `in_selection` overrides bg with `selection_bg()`. `WIDE_CHAR_SPACER` emits a space (don't skip — would left-shift the row).
  - **BG fill**: a `rect_filled(run_x, row_y, char_cols*col_stride × cell_h, 0.0, bg)` is painted BEFORE the glyphs (egui's `TextFormat::background` only fills behind glyph paths, losing space-cell bars).
  - **Non-ASCII glyph**: flushed in its own single-char galley pinned to `col*col_stride` so its differing advance can't shift the rest of the run.
- **Cursor block**: `rect_filled` at `(origin.x.round()+cursor_col*col_stride, round(origin.y+cursor_line*cell_h+scroll_pixel_offset))`, size `col_stride × cell_h.round()`, color = `theme.terminal_fg` at alpha 130. (Solid translucent block; no blink, no shape variants.)
- **URL/path hover underline**: 1px `line_segment` of `theme.terminal_fg` under the hovered hit, drawn after row paint, from `col_start*col_stride` to `col_end*col_stride` at row baseline.

#### 2c. Scrollbar (right edge, only when `history_size > 0 && total > rows`)
- Track width 6.0 at `clip_rect.max.x`. Thumb height = `track_h * rows/total`, min 20. `y_from_top = scrollable * (1 - display_offset/history_size)` (offset 0 = bottom).
- Thumb is an interactable rect (`Sense::drag`, id `"terminal_scrollbar"`). Rendered as a rounded bar whose width is `8` when hovered/dragged else `4`, anchored to right edge, radius `(width/2).round()`.
- Colors: dragged = `theme.accent`; hovered = white α90; idle = white α30. Cursor forced to `Default` over thumb.

### 3. Interactions

| Interaction | Effect |
|---|---|
| Click tab chip | `tp.active = idx` |
| Click close × / middle-click chip | `tp.close(idx)`; if last tab, pane returns `None` (main.rs dead-tab sweep closes pane) |
| Double-click chip (not on ×) | start rename |
| Right-click chip | context menu: **Rename Tab** (PENCIL_SIMPLE), **Close Tab** (X), **Close Other Tabs** (X_CIRCLE, only if >1 tab), **Duplicate Tab** (COPY) |
| Click "+" | spawn new Terminal at active tab's cwd (80×24, resized next frame), `tp.add` |
| Rename TextEdit | width 110, font 11.5 Proportional; auto-focus + select-all once (gated by temp bool keyed `("term_tab_rename_focused",pane_id,idx)`); Enter commits (empty→`name=None`), Esc / lost_focus cancels |
| Mouse wheel over grid | accumulate into `scroll_carry` (`wheel/cell_h`), clamp at boundaries, commit whole-row crossings via `scroll_display(lines)`, sub-row remainder rendered as `scroll_pixel_offset` |
| Drag in grid | range select; promotes to `SelectionType::Block` if click is between two box-drawing separator columns (`is_inside_vertical_separators`, ≥60% of rows have `│┃║╎╏╽╿`), else `Simple`; `pixel_to_point` maps px→`Point{Line,Column}` + `Side` |
| 1 click | clear selection / place |
| 2 clicks (≤500 ms, same cell) | `SelectionType::Semantic` (word) |
| 3 clicks | `SelectionType::Lines` |
| Shift+click | extend existing selection to click point |
| Click on hovered URL | `webbrowser::open(url)` |
| Click on hovered Path | if inside `workspace_root` (and not under `.git`) and is a file → return `Some(PathBuf)` (open in Crane editor); else `open_in_default_app` (`open`/`xdg-open`/`explorer`) |
| Drag scrollbar thumb | `scroll_display(-(dy*history/scrollable).round())` |

**Keyboard** (only when `has_focus`, input enabled, no other widget focused; routes raw bytes via `write_input`):
- `Cmd+K` → queued `clear_requested`. Bare shell: `\e[H\e[2J\e[3J` + `scroll_to_bottom` + `\x0c` (Ctrl+L to repaint prompt) + clear history log. Foreground TUI: only `\e[3J` (scrollback erase). Distinguished by `has_foreground_process()`.
- `Cmd+A` → select whole visible grid (Simple, (0,0)→(rows-1,cols-1)).
- `Cmd+C` (Event::Copy) → `selection_to_string`, trim trailing whitespace per line, `ctx.copy_text`.
- `Cmd+V` (Event::Paste) → bracketed-paste (`\e[200~`…`\e[201~`) only if `is_bracketed_paste()` (DECSET 2004), else raw. macOS image paste: NSEvent monitor writes temp PNG, path drained via `mac_keys::drain_pending_image_paths` and pasted as text.
- `Ctrl+<letter>` → control byte (`letter - 'a' + 1`).
- Other `Cmd+<key>` → swallowed (no PTY echo).
- `Alt+ArrowLeft/Right` → `\e b` / `\e f`; `Alt+Backspace` → `\e\x7f`; `Alt+<letter>` → `ESC <char>`.
- Named keys (`named_key_bytes`): Enter `\r`, Tab `\t`, Backspace `0x7f`, Esc `0x1b`, arrows CSI (`\e[A..D`) or SS3 (`\eOA..D`) under DECCKM (`is_app_cursor`), Home/End CSI/SS3, PageUp `\e[5~`, PageDown `\e[6~`, Delete `\e[3~`.
- `Event::Text` → raw bytes to PTY.
- macOS Shift+Tab: NSEvent monitor catches CSI Z (egui's focus navigator eats it in-frame); drained → `\e[Z`. `set_terminal_focused` toggled so the monitor knows to swallow.

### 4. States & animations
- **Tab**: idle / hover / active / renaming (TextEdit replaces chip). No transitions/animation — instant state swap.
- **Close button**: idle / hover (red `theme.error` fill).
- **Scrollbar thumb**: idle (4px, white α30) / hover (8px, white α90) / dragging (8px, accent). Width change is instantaneous, not animated.
- **Cursor**: static translucent block (alpha 130). No blink (no timer/animation).
- **Link hover**: underline + PointingHand cursor appears only while pointer is over the hit range.
- **Scroll**: sub-row `scroll_pixel_offset` produces visually smooth scrolling between row commits; not a tween, just per-frame fractional offset driven by `scroll_carry`.
- No disabled visual state for tabs; whole pane input is gated by `ui.is_enabled()` (modal backdrop).

### 5. warpui port approach (per element)

- **Tab strip + chips**: build with the standard warpui pattern — allocate the chip rect, paint bg/fg by state, then a hit-`Rect`-on-top for the chip body + a second hit-Rect for the close ×, dispatching `dispatch_typed_action` (e.g. `TermTabActivate{idx}`, `TermTabClose{idx}`) and `ctx.notify` for repaint. Icons are phosphor `Text` nodes (`PLUS`, `X`, `PENCIL_SIMPLE`, `X_CIRCLE`, `COPY`). Right-click menu → warpui context-menu builder keyed on the chip rect.
- **Rename TextEdit**: reuse warpui's inline single-line editor with the once-only focus+select-all flag stored in temp memory keyed by `(pane_id, idx)`; Enter/Esc/lost-focus handled identically.
- **Grid renderer**: this is the load-bearing port. Keep the run-batching algorithm verbatim — group cells per viewport line into runs of equal `(fg,bg,underline)`, paint a bg `rect` then one text galley per run pinned to `run_start_col*col_stride`, with non-ASCII glyphs flushed individually. Cursor and selection bg are plain `rect_filled`. Drive it from the same `CtTerm` snapshot (`renderable_content()` → `(point, cell)` list + cursor + `selection_range` + `display_offset` + `scrollback.len()`). The `color_to_egui` + `palette` tables port 1:1 (resolve `Named` via warpui theme tokens `terminal_fg`/`terminal_bg`).
- **Theme tokens**: `terminal_bg`, `terminal_fg`, `selection` (fallback: accent α72), `accent` (α55 active tab, α-derived), `row_hover`, `text`, `text_muted`, `error`. Map each to warpui's theme token names.
- **Scrollbar**: hit-Rect drag with the same thumb-height/`y_from_top` math; `dispatch_typed_action(ScrollDisplay{lines})`.
- **Input routing**: the keyboard→escape-sequence tables (`named_key_bytes`, `key_letter`, Ctrl/Alt/Cmd handling, bracketed-paste gating) are pure functions — port them directly. PTY writes go through the `Terminal::write_input` equivalent. The `other_widget_focused` / `is_enabled` gates and the deferred `flush_scroll_to_bottom` (avoid Context-lock deadlock) must be preserved.
- **PTY/Term backend**: port `Terminal` essentially unchanged — portable-pty + the reader thread with the dual epoch/cursor repaint gate, `take_pty_replies` drain inside the lock, `HISTORY_MAX` cap, terminfo `xterm-crane` install, Drop kill+wait. This is platform code, not UI; it should move over as-is.
- **GPU path**: `gpu_render.rs` is a scaffold (gradient only) — for warpui, do NOT port the test pattern; either skip it or build the real glyph-atlas shader from scratch. It is not on the parity-critical path.

### 6. Honest port-status estimate

**~10–15% ported.** The PTY/Term backend (`term.rs`) is essentially backend code and likely transfers near-verbatim, but the entire UI surface in `view.rs` — the run-batched grid painter, cursor/selection rendering, the 16-color palette + SGR/inverse/wide-char handling, URL/path link detection + click routing, multi-click selection, block-mode separator detection, the smooth sub-row scroll carry, the right-edge scrollbar, the full keyboard→escape-sequence input layer (including macOS NSEvent Shift+Tab / image-paste integration), and the multi-tab strip with rename/context-menu — is large, subtle, and bug-fix-encrusted (cursor stride, wide-char alignment, sync-frame replay). Each of those carries non-obvious correctness invariants that must be reproduced exactly. The `gpu_render.rs` scaffold contributes 0% of real terminal rendering. Treat the grid painter and the input layer as the two highest-risk, highest-effort port items.


---


<!-- ===== git ===== -->

## `git.rs` — Shell-Out Git API

`src/git.rs` is a **pure backend module — no UI, no egui, no rendering**. It is a stateless façade of free functions that shell out to the `git` binary via `std::process::Command` (per project rule: never `git2`/`libgit2`). It owns no state, holds no handles; every call spawns a fresh process against a `repo: &Path`. All callers (`ui_right.rs` Changes/Files panels, `ui_left.rs` workspace badges, `diff_view.rs`, `file_view.rs` gutter, commit-tree UI) consume the plain data structs defined here. This section documents the full public API surface for 1:1 reproduction.

### Module contract / conventions

- **No async.** Every function is synchronous and blocking. Callers run them on background `std::thread`s (PTY/Git workers), never the UI thread — they shell out and may stall on disk or network.
- **Failure model is silent-degrade for reads, `Result<_, String>` for mutations.** Read-style functions (`list_workspaces`, `list_local_branches`, `commit_files`, `is_submodule`, …) return empty `Vec`/`None`/`false`/`String::new()` on any error and never surface a message. Mutating functions (`stage`, `commit`, `push`, …) return `Result<(), String>` (or `Result<String, String>`) where the `Err` carries `git`'s raw stderr verbatim so the UI pill/modal shows the real auth/network/conflict error.
- **`fn run(repo, args) -> Result<(), String>`** (private, lines 1122–1133) is the shared mutation helper: spawn `git` with `args` in `current_dir(repo)`, `Ok(())` on success, else `Err(stderr)`. Reuse this exact pattern in warpui — one helper, stderr verbatim on failure.
- **Non-interactive network ops.** `push`/`pull`/`fetch` set `GIT_TERMINAL_PROMPT=0` and `stdin(Stdio::null())` so an HTTPS remote needing credentials fails fast instead of blocking the worker thread forever on a tty.
- **`-z` / NUL parsing.** `status` uses `--porcelain=v1 -z` so paths with spaces/non-ASCII are emitted raw (not C-quoted) — quoted paths broke `git add`. Rename/copy records emit two consecutive NUL records (new path, then old path).

### Public structs / enums

| Type | Kind | Fields | Holds |
|---|---|---|---|
| `WorkspaceInfo` | struct (Debug, Clone) | `path: PathBuf`, `branch: String` | One row of `git worktree list --porcelain`. `branch` defaults to `"detached"`, `"(bare)"`, or the short ref name. |
| `ChangeStatus` | enum (Clone, Copy, PartialEq, Debug) | `Added`, `Modified`, `Deleted`, `Renamed`, `Untracked` | Per-file change classification. Maps from porcelain status chars `A/M/D/R`; everything else → `Modified`; `??` → `Untracked`. |
| `FileChange` | struct (Clone, Debug) | `path: String` (for renames = NEW path), `old_path: Option<String>` (rename source, only when staged side is `Renamed`), `status: ChangeStatus` (representative — prefers staged side), `has_staged: bool`, `has_unstaged: bool`, `staged_status: Option<ChangeStatus>` (X side), `unstaged_status: Option<ChangeStatus>` (Y side) | One merged row per file. X=staged, Y=unstaged; both can be set (e.g. `MM`). UI groups/sorts by `path`. |
| `GitStatus` | struct (Clone, Default, Debug) | `branch: String`, `changes: Vec<FileChange>`, `added: usize`, `deleted: usize`, `ahead_behind: Option<AheadBehind>` | Whole Changes-pane snapshot. `branch` from `## ` line (split on `...`). `added`/`deleted` from `shortstat` + untracked line counts. `ahead_behind` is `None` for branches without `@{u}`. |
| `AheadBehind` | struct (Clone, Copy, Debug, Default) | `ahead: usize`, `behind: usize` | Commits ahead/behind upstream, for the `↑N ↓N` toolbar indicator. |
| `WorktreeDirty` | struct (Clone, Debug, Default) | `unpushed_commits: usize`, `modified_files: usize`, `has_upstream: bool` | "What would be lost if removed now" — for the Remove-Workspace confirm. When no upstream, `unpushed_commits` falls back to `main..HEAD` count. |
| `ParsedHunk` | struct | `patch: String`, `old_start/old_count/new_start/new_count: usize` | One unified-diff hunk with self-contained patch text + `@@` line ranges, for matching git hunks to the in-memory diff view by line number. |
| `FileDiff` | struct (Clone, Debug, Default) | `lines: HashMap<usize, DiffLine>` (1-based line → marker), `deletions: Vec<DeletionGap>`, `blocks: Vec<DiffBlock>` | Per-line gutter classification for the editor (`git diff HEAD -U0`). |
| `DiffLine` | struct (Clone, Debug) | `kind: DiffLineKind`, `block_idx: Option<usize>` (index into `FileDiff::blocks` for Modified; `None` for Added) | One changed working-tree line's gutter marker. |
| `DiffLineKind` | enum (Clone, Copy, Debug, PartialEq) | `Added`, `Modified` | Gutter color selector (green=added, blue=modified). |
| `DeletionGap` | struct (Clone, Debug) | `after_line: usize` (1-based; 0 = before line 1), `head_lines: Vec<String>` | Deleted region → small red gutter marker + tooltip of removed HEAD lines. |
| `DiffBlock` | struct (Clone, Debug) | `new_start: usize`, `new_count: usize`, `old_lines: Vec<String>` | One `-N +M` hunk: all N old HEAD lines + the working-tree range of the M new lines, for the modified-line tooltip. |

### Public functions (full inventory)

**Workspace / worktree:**
- `list_workspaces(repo) -> Vec<WorkspaceInfo>` — parses `git worktree list --porcelain`.
- `workspace_add(repo, path, branch, create_new) -> Result<(), String>` — `worktree add`. With `create_new`: `-b <branch> <path>` (no `--`). Without: `-- <path> <branch>`.
- `workspace_remove(repo, path) -> Result<(), String>` — `worktree remove --force <path>` (force: explicit user "Remove" decision; non-force leaves dir blocking re-add).
- `worktree_dirty(worktree) -> WorktreeDirty` — modified count via `status --porcelain --untracked-files=all` line count; upstream via `rev-parse --abbrev-ref ... @{u}`; unpushed via `rev-list --count @{u}..HEAD` (or `main..HEAD`).

**Status / counts:**
- `status(repo) -> Option<GitStatus>` — the core Changes-pane fetch. `--porcelain=v1 --branch --untracked-files=all -z`. Merges X/Y sides per file, handles `R`/`C` two-record renames, adds untracked file line counts to `added`.
- `shortstat(repo) -> Option<(usize, usize)>` (private) — `diff --shortstat HEAD`, parses `insertion`/`deletion` numbers.
- `untracked_added_lines(repo, changes) -> usize` (private) — line-counts each `Untracked` text file (skips empty/binary via NUL probe; counts final no-newline line).
- `ahead_behind(repo) -> Option<AheadBehind>` — `rev-list --left-right --count @{u}...HEAD`; `None` if 2 parts don't parse.

**Staging / hunks:**
- `stage(repo, path)` — `add -- <path>`.
- `unstage(repo, path)` — `restore --staged -- <path>`.
- `stage_hunk(repo, patch)` / `unstage_hunk(repo, patch)` — pipe patch to `apply --cached [--reverse] --unidiff-zero -` (private `apply_hunk`, stdin pipe, stderr on failure). `--unidiff-zero` required for `-U0` patches.
- `is_hunk_staged(repo, patch) -> bool` — probe via `apply --reverse --cached --check --unidiff-zero -`; success = already staged.

**Diff content / parsing:**
- `staged_content(repo, rel_path) -> Option<String>` — `show :<path>` (index version).
- `head_content(repo, path) -> String` — `show HEAD:<path>`, empty on error.
- `show_at(repo, reference, path) -> Vec<u8>` — `show <ref>:<path>`, raw bytes, empty on missing.
- `file_diff_raw(repo, rel_path) -> Option<String>` — `diff --unified=0 HEAD -- <path>` (every region its own hunk for per-hunk staging).
- `file_diff_staged(repo, rel_path) -> Option<String>` — `diff --cached -- <path>`.
- `parse_hunks(diff) -> Vec<(usize, String)>` / `parse_hunks_detailed(diff) -> Vec<ParsedHunk>` — split a unified diff into self-contained per-hunk patches (prepend `diff --git`+index/mode prefix; ensure trailing `\n`; never insert blank line before `@@`). `parse_hunk_header` (private) parses `@@ -o,c +n,m @@`.
- `parse_file_diff(repo, rel_path) -> Option<FileDiff>` — gutter data. `ls-files --error-unmatch` tracked check → `None` if untracked; `diff HEAD -U0`; classifies each hunk: pure-add → `Added` lines; `-N +M` → one `DiffBlock` + `Modified` lines; pure-delete → `DeletionGap`. `parse_range` (private) parses `"3,2"`→`(3,2)` / `"5"`→`(5,1)`.

**Branch / commit ops:**
- `current_branch(repo) -> Option<String>` — `branch --show-current`; detached → `(detached <short>)`.
- `list_local_branches(repo)` / `list_remote_branches(repo)` — `for-each-ref --format=%(refname:short) refs/heads/` or `refs/remotes/` (drops `*/HEAD`).
- `checkout_branch(repo, branch)` — `switch <branch>` (modern, dash-safe).
- `commit(repo, message)` — `commit -m`.
- `push(repo) -> Result<String,String>` / `pull` (`pull --ff-only`) / `fetch` (`fetch --prune`) — non-interactive; return a human summary line (ref-update / "Everything up-to-date" / "No new refs" / "Fetched N refs") for the status pill.
- `commit_files(repo, sha) -> Vec<(char, PathBuf)>` — `show --name-status --format=`.
- `checkout_commit(repo, sha)` — `checkout <sha>` (detached). `branch_from(repo, name, sha)` — `branch <name> <sha>`. `cherry_pick(repo, sha)`. `revert(repo, sha)` — `revert --no-edit`.

**Repo discovery:**
- `find_git_root(start) -> Option<PathBuf>` — canonicalize + walk ancestors for `.git` (innermost wins; via `crate::util::find_ancestor`).
- `discover_repos(start, max_depth) -> Vec<PathBuf>` — DFS for `.git` dirs, skipping `node_modules/target/dist/build/.next/vendor/.venv/venv/.cache/.turbo/.cargo` and dotdirs (except `.git`); sorted+deduped.
- `is_submodule(repo, path) -> bool` — `submodule status --recursive`, abs-path match (incl. canonicalized).
- `is_path_ignored(repo, path) -> bool` — `check-ignore -q -- <rel>`, exit 0 = ignored.

### Tests
Two unit tests pin `parse_hunks`: (1) no blank line before `@@` (the "garbage at line 5" `git apply` bug), header starts `diff --git`, ends with `\n`; (2) trailing newline appended when the source diff lacks one.

### Interactions / UI elements / states / animations
**None — this file has zero.** No rendered element, no icon, no theme token, no click/hover/drag/keyboard handler, no hover/selected/active/expanded/disabled state, no animation. It is a data/process layer consumed entirely by other modules. All UI for these (Changes tree icons, stage/unstage buttons, `↑N ↓N` indicators, diff gutter colors, Remove-Workspace modal) lives in `ui_right.rs`, `ui_left.rs`, `views/diff_view.rs`, `views/file_view.rs` and must be documented in *those* areas, where the egui_phosphor glyphs and theme tokens actually resolve.

### warpui port approach
This is a **straight logic port — no UI reuse patterns apply** (no hit-Rect, no `dispatch_typed_action`, no `ctx.notify`, no phosphor `Text`, no Node tree). Concretely:
1. **Recreate the module verbatim** as a stateless free-function module in warpui's backend. Spawn the `git` binary via the platform's process API; keep the exact arg vectors, the `-z`/NUL porcelain parsing, `--unidiff-zero` for hunk apply, `GIT_TERMINAL_PROMPT=0`+null-stdin for network ops, and the single `run()` mutation helper (stderr verbatim on `Err`).
2. **Port the structs 1:1** as plain data types (the table above is the full schema). They cross the worker→UI boundary, so keep them `Clone`/`Debug`.
3. **Keep the failure contract:** reads silent-degrade (empty/`None`/`false`), mutations return `Result<_, error-string>` carrying raw stderr — warpui's pill/modal renders that string.
4. **Threading:** call every function off the UI thread (warpui's git worker), and wake the renderer on completion (warpui equivalent of `Context::request_repaint()`), exactly as Crane does.
5. **Port both diff parsers and the gutter classifier exactly** (`parse_hunks*`, `parse_file_diff`, `parse_hunk_header`, `parse_range`) — these are pure string algorithms with subtle invariants (prefix already ends in `\n`, no blank line before `@@`, trailing `\n` required, 1-based line indexing, `-N +M`→single block). Port the two unit tests alongside to lock those invariants.

### Port status
**0% (not started in warpui), but this is the lowest-risk, mechanically-portable area in the codebase.** It is pure Rust `std` + `git` shell-out with no egui, no platform UI, and no Crane-specific framework dependency except `crate::util::find_ancestor` (a trivial ancestor-walk helper that must be ported alongside `find_git_root`). Estimated effort is a direct copy + the small `find_ancestor` dependency + the two tests; there are no rendering or interaction decisions to reproduce here.


---


<!-- ===== git-log ===== -->

## Git Log Pane (3-column dock + commit graph)

A bottom-docked region with a header strip and a three-column body: **Refs** (left, collapsible) | **Log** (middle, graph + commit rows) | **Details** (right, collapsible). Backed by an async worker that shells `git log --all` + `git for-each-ref` + `git worktree list`, an in-memory lane-layout graph engine, a filesystem watcher on `.git`, and a render-time filter layer. Source: `src/git_log/{view/mod.rs,view/log.rs,view/refs.rs,view/details.rs,graph.rs,state.rs,data.rs,refs.rs,refresh.rs}`.

### 1. Public types

#### `data.rs`
- `pub type Sha = String;`
- `pub struct CommitRecord { sha: Sha, parents: Vec<Sha>, author: String, date: String /* ISO 8601 */, subject: String, refs_decoration: String }` — one git log line. `date` kept as raw ISO string (parsed lazily to avoid chrono in hot path). `refs_decoration` is the raw `%D` output, e.g. ` (HEAD -> main, origin/main, tag: v1.0)`.
- `pub fn parse_log_output(stdout: &str) -> Vec<CommitRecord>` — splits on `\n` (RECORD_SEP), fields on `\x1f` (FIELD_SEP, the ASCII unit separator). Format string: `%H<US>%P<US>%an<US>%aI<US>%s<US>%D`. Lines with fewer than 6 fields are skipped (malformed-safe). Parents split on space; empty parents → empty Vec (root commit).
- `pub fn load_commits(repo: &Path, max_count: usize) -> Vec<CommitRecord>` — runs `git log --all --date-order --pretty=format:… --max-count=N`. Returns empty Vec on any error (logs a warning). Called with `max_count = 10_000`.

#### `refs.rs`
- `pub struct RefEntry { name: String /* fully-qualified refs/heads/… */, sha: String, upstream: Option<String> }`
- `pub struct WorktreeEntry { path: PathBuf, branch: String }`
- `pub struct RefSet { local: Vec<RefEntry>, remote: Vec<RefEntry>, tags: Vec<RefEntry>, worktrees: Vec<WorktreeEntry>, head: Option<String> /* HEAD sha */ }` — derives Default, PartialEq, Clone.
- `parse_for_each_ref(stdout)` — buckets refs by `refs/heads/`, `refs/remotes/`, `refs/tags/` prefix; fields `%(refname)<US>%(objectname)<US>%(upstream)`.
- `parse_worktree_porcelain(stdout)` — parses `git worktree list --porcelain`: tracks `worktree <path>`, `branch refs/heads/<b>` (strips prefix), `bare` → `(bare)`, `detached` → `detached`.
- `load_refs(repo)` — runs `for-each-ref` over the three ref namespaces, then `rev-parse HEAD` (sets `head`), then `worktree list --porcelain` (sets `worktrees`).

#### `graph.rs`
- `pub struct LaneRow { sha: Sha, own_lane: u8, parent_lanes: Vec<u8>, terminating_lanes: Vec<u8>, passthrough_lanes: Vec<(u8 /*lane*/, u8 /*color slot*/)>, color: u8, visible_lanes_after: u8 }` — per-row graph geometry.
  - `own_lane`: column index of this commit's dot.
  - `parent_lanes`: lanes the parents occupy on the next row; `[0]` is normally `own_lane` (linear continuation), empty for root commits.
  - `terminating_lanes`: lanes alive before this row but not after (closing branches) → painter draws a cap circle.
  - `passthrough_lanes`: lanes alive before AND after, not this commit's own lane → painter draws a full-height vertical segment in that lane's color slot.
  - `color`: 0–7 palette index for this commit's dot.
  - `visible_lanes_after`: lane count still active after this row (drives `max_lane`/graph width).
- `pub struct LaneFrame { rows: Vec<LaneRow>, max_lane: u8 }` — derives Clone, Default. The whole frame is cheap to clone (no commit payloads).
- `pub struct ColorSeeder { epochs: Vec<u32> }` — per-lane allocation counter. `allocate(lane)` increments the lane's epoch and returns `((lane*7919) ^ (epoch*31337)) % 8`. `current(lane)` returns the color without incrementing. Gives "stable color per branch occupancy" — a lane re-used by a new branch changes color.
- `pub fn layout(commits: &[CommitRecord]) -> LaneFrame` — single-pass newest→oldest lane allocator. For each commit: find the lane waiting for its sha (or allocate leftmost-free); first parent claims the same lane unless that parent is already tracked elsewhere (then terminate + merge); subsequent parents fork into fresh lanes or merge into existing; compact trailing empty lanes; compute terminating/passthrough sets. 16 unit tests pin straight-line, fork/merge, octopus (3-parent), root termination, passthrough, dangling-lane elimination, and color-seeder stability.

#### `state.rs`
- `pub struct FilterState { text: String, branch: Option<String>, user: Option<String> }` — render-time filter, never touches git query. Derives Default, Clone.
- `pub enum GitLogOp { Checkout(Sha), BranchFrom(Sha), WorktreeFrom(Sha), CherryPick(Sha), Revert(Sha), CopyHash(Sha) }` — context-menu op, bubbled to main.
- `pub struct GraphFrame { commits: Vec<CommitRecord>, refs: RefSet, lanes: LaneFrame, generation: u64 }` — one consistent worker snapshot.
- `pub struct GitLogState { … }` — the persistent pane state. Key fields:
  - `height: f32` (def 320), `maximized: bool`.
  - `col_refs_width: f32` (def 220), `col_details_width: f32` (def 360), `col_log_meta_width: f32` (def 220, clamped 120–360 at render).
  - `col_refs_collapsed: bool`, `col_details_collapsed: bool`.
  - `selected_commit: Option<Sha>`, `selected_file: Option<PathBuf>`.
  - `frame: Option<GraphFrame>`, `generation: u64`, `worker_job: Option<JobHandle<GraphFrame>>`, `reload_pending: bool`.
  - `filter: FilterState`, `filter_lane_cache: Option<(u64 sig, u64 gen, LaneFrame)>`.
  - `watcher: Option<refresh::Watcher>`, `watched_repo: Option<PathBuf>`, `last_poll: Instant`.
  - `fetch_in_flight: Arc<AtomicBool>`.
  - `pending_op: Option<GitLogOp>`, `pending_branch_prompt: Option<(Sha, String)>`.
  - `last_visible_count: usize`, `pending_scroll_to_selected: bool`, `pending_focus_filter: bool`, `has_focus: bool`.
- Methods: `maybe_reload`, `reload`, `poll_worker`, `fetch_all`, `is_fetching`, `is_loading`, `job_scope`.

#### `view/mod.rs`
- `pub struct ViewEffect { close: bool, open_diff: Option<(String sha, PathBuf)>, op: Option<GitLogOp>, branch_from: Option<(String sha, String name)> }` — effects bubbled to main's render path (caller has `&mut App`).
- `pub fn render(ui, region: Rect, state: &mut GitLogState, repo: &Path) -> ViewEffect`.

### 2. Worker / data flow (refresh.rs + state.rs)

- **`maybe_reload(repo, ctx)`** runs once per frame: re-creates the `Watcher` if the repo path changed; triggers reload when (a) no frame and no in-flight job, (b) the FS watcher fired (debounced 250 ms), or (c) 30 s poll fallback fires *only* when the watcher has been quiet > 30 s.
- **`reload`** submits a `JobSystem` job (`JobKey` scoped by hashed repo path, `Priority::Foreground`, `Pool::Io`) that runs `load_commits(repo, 10_000)` + `load_refs(repo)` + `graph::layout(&commits)` → `GraphFrame { generation: gen+1 }`. If a job is already in flight, sets `reload_pending`.
- **`poll_worker`** drains the job; on `Done` swaps in the new frame, bumps `generation`, and if `reload_pending` kicks a follow-up reload (never drop a watched change). On `Cancelled` clears the handle.
- **`Watcher`** wraps `notify::RecommendedWatcher` on `.git/HEAD`, `.git/refs` (recursive), `.git/packed-refs`. `poll(min_gap)` coalesces bursts and debounces; `last_event_elapsed()` gates the poll fallback.
- **`fetch_all_async`** sets `fetch_in_flight=true`, submits a `Global`-scoped `Background`/`Io` job running `git fetch --all --prune --tags`, clears the flag on exit. Fire-and-forget — the watcher picks up the resulting ref writes.

### 3. Rendered UI

**Background:** whole `region` filled `theme.bg`. Focus tracker: an invisible `Sense::click` interact over `region` (id `git_log_focus_tracker`) sets `has_focus=true` on click, clears it on `clicked_elsewhere`.

#### 3a. Header strip (`HEADER_H = 28.0`), one `horizontal` row
Left→right:
1. 8 px pad. **Refs toggle button** — glyph `ARROW_LINE_RIGHT` when collapsed (hover "Show refs panel") else `ARROW_LINE_LEFT` (hover "Hide refs panel"); toggles `col_refs_collapsed`. Pinned left so direction is unambiguous.
2. 8 px pad. **"Git Log"** label, `.strong()`, `theme.text`.
3. 8 px pad. **Loading state**: `ui.spinner()` + "loading…" small/`muted`. Else if frame loaded: commit count label — `"N commits"` or `"K of N commits"` when a filter is active, small/`muted`.
4. Right-anchored group (`Layout::right_to_left`): 8 px pad, **X button** (`icons::X`, hover "Close (Cmd+9)") → `request_close`; 4 px; **Details toggle** (`ARROW_LINE_LEFT` collapsed / `ARROW_LINE_RIGHT` expanded, hover Show/Hide details panel) → toggles `col_details_collapsed`; 4 px; **Refresh** (`ARROW_COUNTER_CLOCKWISE`, hover "Refresh") → cancels in-flight job, clears handle, `reload`; 4 px; **Fetch** — `ui.spinner()` while `is_fetching()`, else button `DOWNLOAD_SIMPLE` (hover "Fetch all (git fetch --all --prune --tags)") → `fetch_all`.

#### 3b. Body layout (`view/mod.rs`)
Body = region below header. Outline: `rect_stroke` 1px `theme.border` inset. Constants: `SPLIT_W=8`, `MIN_COL_W=140`, `MIN_LOG_W=240`.
- Column widths: refs = 0 if collapsed else `col_refs_width`; details = 0 if collapsed else `col_details_width`; log = remaining.
- Rects in order: `refs_rect | split1_rect (8px) | log_rect | split2_rect (8px) | details_rect`.
- **Splitter 1** (`split1_rect`): filled `theme.divider`; when refs not collapsed, an `interact(Sense::drag())` (id `git_log_split1_drag`); on hover/drag sets `CursorIcon::ResizeHorizontal`; drag adjusts `col_refs_width += delta.x` clamped `[MIN_COL_W, body.width - details_w - MIN_LOG_W - 16]`.
- **Splitter 2** (`split2_rect`): same, id `git_log_split2_drag`, `col_details_width -= delta.x` (inverse because it grows leftward), same clamp band.
- Each visible column rendered into a child `Ui` with `max_rect` + matching `set_clip_rect`.

#### 3c. Refs column (`view/refs.rs`)
Vertical `ScrollArea` (`id_salt "git_log_refs"`, `auto_shrink [false,false]`). If `refs` is None → 6px + "loading…" small/`muted`. Else four sections via `ref_section`/`wt_section`:
- Section header: uppercase title (`"LOCAL"`, `"REMOTE"`, `"TAGS"`, `"WORKTREES"`), color `HEADER_COLOR = rgb(140,146,162)`, size 10.5, `.strong()`, preceded by 6px space. Empty sections render nothing.
- **Branch/tag/remote row** (`ref_section`): display = name with namespace prefix stripped. Prefix glyph `ASTERISK` if this ref's sha == HEAD sha, else `GIT_BRANCH`, then two spaces + display. Size 12.5. `.strong()` when it's HEAD. Color `accent()` when it's the active branch filter. Rendered as `Label::new(text).sense(Sense::click())`; hover → `PointingHand`. Click: if already the active filter → clear `filter.branch`; else set `filter.branch = Some(display)` AND write the ref's tip sha to `selected_sha_out`.
- **Worktree row** (`wt_section`): `FOLDER` glyph + branch + ` · ` + folder name (file_name of path), size 12.5, clickable label (no action) with `on_hover_text` = full path.
- **Clear-filter button**: when `filter.branch.is_some()`, 8px + `small_button("{X}  Clear filter")` → clears `filter.branch`.
- Selecting a branch in mod.rs sets `selected_commit`, clears `selected_file`, and sets `pending_scroll_to_selected = true`.

#### 3d. Log column (`view/log.rs`) — the centerpiece
**Constants:** `ROW_H=22`, `COL_W=14` (lane column width), `DOT_R=4`, `GRAPH_PAD_LEFT=8`.

**Empty/loading states:** no frame → 8px + "loading…" (if loading) / "no commits to display" small/`muted`. Empty commits → "No commits yet" `muted`.

**Filter bar** (`bar_h=24`, corner radius 4), 4px top space, `horizontal`, item_spacing.x=6:
- 8px pad. **Search field** (width 240): manually painted `rect_filled(theme.surface_alt)` + `rect_stroke(1px, theme.divider)`. `MAGNIFYING_GLASS` glyph at left+8 (font 12, `theme.text_muted`). A borderless `TextEdit::singleline(&filter.text)` overlaid in an inset child rect (left+26 to right-22), transparent bg, hint "subject / hash / author", id `git_log_filter_text`. Requests focus once when `pending_focus_filter` (then clears it). When text non-empty, a **clear (×)** hit-rect (right-20..right-4): hover paints `surface_hi` rounded-3 bg + `PointingHand`; `icons::X` (font 11, `text_muted`); click clears text.
- **Branch facet** — `compact_combo` (id `git_log_branch_filter`), label = filter.branch or "branch", active when set. Menu: "all branches" (clears), separator, each local branch as `selectable_value`.
- **User facet** — `compact_combo` (id `git_log_user_filter`), label = filter.user or "user". Menu: "all users", separator, sorted/deduped authors from commits.
- Right-anchored status label (`right_to_left`, 8px pad): `"N commits"` or `"K of N"` (size 11, `text_muted`).
- `compact_combo`: a styled `egui::ComboBox::from_id_salt` — visuals overridden so inactive bg = `surface_alt`, hovered bg = `surface_hi`, corner radius 4; bg_stroke 1px = `accent` when active else `divider`; selected_text size 12, color `text` when active else `text_muted`.

**Filtering (after the bar):**
- text needle: case-insensitive substring over `"{subject} {sha} {author}"`.
- branch filter: BFS the in-memory parent graph from the ref's tip sha → set of *reachable* shas (not just the decorated tip); ~µs for 10k commits.
- user filter: exact author match.
- When any filter active, lanes are recomputed from *only the visible commits* via `graph::layout`, cached in `filter_lane_cache` keyed by `(hash(text,branch,user), frame.generation)`; cache cleared when no filter.
- `lanes_ref` = filtered lanes or `frame.lanes`; `graph_width = GRAPH_PAD_LEFT + (max_lane+1)*COL_W`; `meta_w = col_log_meta_width.clamp(120,360)`.

**Keyboard nav:** gated on `has_focus && no egui widget focused && !visible.is_empty()`. ArrowDown/`J` → next visible row; ArrowUp/`K` → prev (saturating); no selection → row 0. Updates `selected_commit`, clears `selected_file`.

**Auto-scroll:** if `pending_scroll_to_selected`, find selected sha's visible index and set `vertical_scroll_offset = idx*ROW_H` (centers target).

**Commit rows** — `ScrollArea::vertical(id_salt "git_log_commits").auto_shrink([false,false]).show_rows(ui, ROW_H, total, …)`. Per row (`vi` indexes filtered slice, `i` = canonical commit index):
- `allocate_response(available_width × ROW_H, Sense::click())`.
- **Row bg:** selected → `surface_hi`; hovered → `surface_alt`; else transparent (rect_filled, radius 0).
- **Graph** via `paint_lane` (see below) using `lanes_ref.rows[vi]` and next row.
- **Ref pills** (`parse_ref_pills`): painted left of subject starting at `rect.left + graph_width + 4`, top+4, height `ROW_H-8`, radius 4. Width estimated `chars*6.2 + 10`. Font proportional 10.5, label centered. Categories/colors:
  - `HEAD -> x` → label "HEAD → x", bg `rgb(102,187,106)` green, fg black.
  - `HEAD` (detached) → green/black.
  - `tag: x` → label x, bg `rgb(255,202,40)` yellow, fg black.
  - local branch (matched against `RefSet.local` short names) → bg `rgb(171,71,188)` purple, fg white.
  - remote-tracking (matched against `RefSet.remote`) → bg `rgb(66,165,245)` blue, fg white.
  - unknown → bg `rgb(110,118,132)` grey, fg white.
- **Subject** painted at `text_x` LEFT_TOP, font 12.5, `theme.text`.
- **Meta** = `"{author}  {date_before_T}"` at `rect.right - meta_w`, LEFT_TOP, font 11.5, `muted()` — only drawn if `meta_x > text_x + 80` (collision guard).
- **Click** → `clicked_sha` → sets `selected_commit`, clears `selected_file`.
- **Right-click context menu** (`row_resp.context_menu`): buttons (glyph + 2 spaces + label) → each sets `picked_op` and closes:
  - `ARROW_RIGHT` "Checkout this commit" → `Checkout`
  - `GIT_BRANCH` "Create branch from here…" → `BranchFrom`
  - `FOLDER_PLUS` "Create worktree from here…" → `WorktreeFrom`
  - `GIT_DIFF` "Cherry-pick onto current" → `CherryPick`
  - `ARROW_COUNTER_CLOCKWISE` "Revert" → `Revert`
  - separator
  - `COPY` "Copy hash" → `CopyHash`
- After the loop: `picked_op` → `state.pending_op`.

**`paint_lane(ui, rect, lane_row, next_lane_row)`** — palette `PALETTE[8]`: green `66bb6a`, blue `42a5f5`, orange `ff9800`, purple `ab47bc`, pink `ec407a`, teal `26a69a`, red `ef5350`, yellow `ffca28`. `color = PALETTE[lane_row.color % 8]`. `dot_x = rect.left + 8 + own_lane*14 + 7`, `dot_y = rect.center.y`.
- **Passthrough lanes:** vertical `line_segment` (stroke 1.5, lane color) from `top-1` to `bottom+1` (clamped to `bottom` on the last row to avoid spill into empty space). The ±1px overlap defeats AA seam between rows.
- **Parent connections:** for each `parent_lane`, next-row color resolved (own_lane match → next.color; passthrough match → its color; else this color). Same lane → straight `line_segment`; off-axis → `QuadraticBezierShape` with control point at `(p_x, dot_y + ROW_H/2)`. Stroke 1.5.
- **Terminating caps:** `circle_stroke` radius `DOT_R-1` at `(t_x, rect.top+2)`, 1px `muted()`.
- **Commit dot:** `circle_filled` radius `DOT_R` at `(dot_x, dot_y)`, drawn *last* (on top of incoming lines).

#### 3e. Details column (`view/details.rs`)
No frame → empty. No `selected_commit` → 8px + "Select a commit" `muted`. Commit not found → empty. Else vertical `ScrollArea` (id `git_log_details`):
- 6px. **Subject** `.strong().size(13)`.
- 2px. **`"{author}  ·  {date}"`** small/`muted`.
- 2px. **Short sha** (first 12 chars) small/`muted`/`.monospace()`, clickable label, hover `PointingHand`, click → `ctx.copy_text(full sha)`.
- 8px, `ui.separator()`, 4px.
- **Changed files** via `crate::git::commit_files(repo, sha)`. Empty → "(no files)" small/`muted`. Each row (`horizontal`): a 1-char status label `.monospace().strong()`, color by status — A→green `66bb6a`, M→yellow `ffca28`, D→red `ef5350`, R→blue `42a5f5`, other→`muted`. Then the path as clickable label, size 12, color `rgb(220,225,232)` when selected else `rgb(180,188,200)`. Hover → `PointingHand`. Click → sets `selected_file`, sets `cb.open_diff = (sha, path)` (caller opens a Diff Pane in the active Layout).

#### 3f. Inline branch-from-commit prompt (`view/mod.rs`)
When `pending_branch_prompt.is_some()`, a floating modal centered in `region` (320×90): drop shadow `rect_filled(expand(3), radius 6, black_alpha(120))`; body `rect_filled(radius 6, theme.surface)`; border `rect_stroke(1px, theme.border_strong)`. Content (inset 12×10): label `"Create branch from {sha[..7]}"` `.strong()`; 6px; `TextEdit::singleline` hint "new branch name" width 296, requests focus; 6px; horizontal **Create** / **Cancel** buttons. Enter (on lost_focus) confirms; Escape cancels. Confirm with non-empty trimmed name → `effect.branch_from = (sha, name)` and clears prompt; cancel clears prompt; otherwise persists buffer.

### 4. Theme tokens referenced
`bg`, `surface`, `surface_alt`, `surface_hi`, `border`, `border_strong`, `divider`, `text`, `text_muted`, `accent` (via `theme::current()`), plus helpers `muted()` and `accent()` from `crate::ui::util`. Hard-coded RGB values used for graph palette, ref pills, file-status colors, the refs section header (`140,146,162`), and details file path colors — these are *not* theme tokens and must be ported verbatim.

### 5. warpui port approach
- **State:** port `GitLogState`/`GraphFrame`/`FilterState`/`GitLogOp` 1:1. The async worker → warpui's existing job/task system (Io pool, foreground). Keep generation-based frame swap and `reload_pending` follow-up logic. Filter + lane cache logic is pure and ports directly.
- **Graph engine (`graph.rs`):** pure Rust, **zero UI** — copy verbatim including the 16 tests. `LaneFrame`/`LaneRow`/`ColorSeeder` are the load-bearing reproduction artifact; the painter just reads them.
- **3-column dock:** compute rects manually (as in mod.rs) rather than nested egui panels — warpui can reuse the same `SPLIT_W/MIN_COL_W/MIN_LOG_W` math. Splitters = `interact(Sense::drag)` hit-rects painting `divider` fill + `ResizeHorizontal` cursor.
- **Header buttons & context menu:** standard clickable buttons; icons = `egui_phosphor::regular` `Text` glyphs (names listed above). Right-click menu → set `pending_op`, dispatch the typed `GitLogOp` up via `ViewEffect` (warpui: `dispatch_typed_action` + `ctx.notify` for reload after the op mutates refs).
- **Commit rows:** use `show_rows` virtualization (10k commits) with fixed `ROW_H`. Each row = one allocated `Sense::click` Rect; all visuals (bg, graph, pills, subject, meta) are `Painter` draws over that rect — the hit-rect-on-top + manual-paint pattern, not widgets. Ref pills are manually painted rounded rects with estimated widths.
- **Refs/Details columns:** recursive flat sections of clickable `Label::new(...).sense(click)` rows; selection writes through to shared state + `pending_scroll_to_selected`.
- **Watcher:** `notify` on `.git/HEAD`+`refs/`+`packed-refs` with 250ms debounce + 30s poll fallback — ports directly.
- **Keyboard nav / Cmd+F focus:** gate on `has_focus` + no-widget-focused, exactly as written, to avoid stealing keys from terminals/filter.

### 6. Honest port status
**~0% ported.** This is a from-scratch egui/Painter-based git-graph pane with no warpui equivalent yet. The clean wins: the graph layout engine (`graph.rs`, fully unit-tested, UI-free) and the data/refs parsers (`data.rs`, `refs.rs`) are pure and copy across at ~100% confidence. The painter (`log.rs` lane drawing, ref pills, AA-seam ±1px tricks, bezier curves) and the 3-column rect math are concrete and fully specified above but unbuilt. The watcher (`refresh.rs`) depends on `notify` and warpui's job system. Realistic estimate: parsers+graph engine portable immediately (~40% of the code volume, high confidence); UI layer (~60%) needs full reimplementation against warpui's paint/interaction primitives.


---


<!-- ===== lsp ===== -->

## LSP Integration

This section documents Crane's Language Server Protocol (LSP) subsystem (`src/lsp/`), an entirely **headless / non-rendering** area: it manages background server processes, parses JSON-RPC, and exposes diagnostics + goto-definition + (deferred) hover to the editor. It has **no UI elements of its own** — its output is consumed by the Files Pane editor (diagnostic underlines, gutter chips) and a future Settings → LSP panel. The spec below covers public types, the data flow, the lifecycle state machine, the (minimal) UI surfaces it feeds, and the warpui port approach.

### Module map

- `src/lsp/mod.rs` — `LspManager` (the public façade), `LanguageConfig`/`LanguageConfigs` (persisted per-language toggles), `which_on_path` helper, install-prompt state.
- `src/lsp/server.rs` — `ServerKey` enum, `LspServer` (one OS process + reader threads + shared response state), `Status`, `Diagnostic`, `Location`, path→key routing, JSON-RPC message handling.
- `src/lsp/protocol.rs` — (referenced, not in scope of this read) `read`/`send` framed JSON-RPC, `path_to_uri`.
- `src/lsp/downloader.rs` — `Downloader`, `DownloadState`, opt-in binary auto-install (rust-analyzer via GitHub gz; TS/Pyright/CSS/HTML/ESLint via `npm install`).

---

### Public types

#### `enum ServerKey` (`server.rs`) — `Clone, Copy, PartialEq, Eq, Hash, Debug`
The canonical language-server identity. Variants:
- `RustAnalyzer`, `TypeScript`, `Gopls`, `Pyright`, `CssLs`, `HtmlLs`, `Eslint` (secondary analyzer for TS/JS, only attached when an eslint config is found in an ancestor dir).

Key methods:
- `command() -> (&'static str, &'static [&'static str])` — the binary name + args (`rust-analyzer []`, `typescript-language-server ["--stdio"]`, `gopls []`, `pyright-langserver ["--stdio"]`, `vscode-css-language-server ["--stdio"]`, `vscode-html-language-server ["--stdio"]`, `vscode-eslint-language-server ["--stdio"]`).
- `install_hint() -> &'static str` — human shell command shown in the install prompt (e.g. `"rustup component add rust-analyzer"`, `"npm i -g typescript typescript-language-server"`, `"go install golang.org/x/tools/gopls@latest"`).
- `language_id(ext) -> &'static str` — maps file extension to the LSP `languageId` (`ts/mts/cts`→`typescript`, `tsx`→`typescriptreact`, `jsx`→`javascriptreact`, else `javascript`; `rs`→`rust`, etc.).

Free functions: `keys_for_path(&Path) -> Vec<ServerKey>` (ext→servers, adds `Eslint` when `has_eslint_config` hits a cached ancestor scan — 5s cache, 11 config filenames); `key_for_path` (first key only, `dead_code`).

#### `enum Status` (`server.rs`) — `Clone, Copy, Debug, PartialEq, Eq`
Lifecycle state of one server: `Spawned` → `Initializing` (has pending opens, not yet initialized) → `Ready` (initialize response received) → `Dead` (spawn failed, write failed, or reader-thread EOF). Derived in `LspServer::status()` from `Shared { dead, initialized, pending_opens }`.

#### `struct Diagnostic` (`server.rs`) — `Clone, Debug`
- `line: u32`, `col_start: u32`, `col_end: u32` (0-indexed; `col_end = u32::MAX` when the range spans multiple lines), `severity: u8` (1 error, 2 warning, 3 info, 4 hint), `message: String` (`dead_code`, reserved for future hover/inline tooltip), `source: Option<String>` (linter name e.g. `"tsserver"`/`"eslint"`, `dead_code`, reserved for source-tagged UI).

#### `struct Location` (`server.rs`) — `Clone, Debug`
Normalized goto-definition target: `path: PathBuf`, `line: u32`, `character: u32` (0-indexed). Parsed from LSP `Location`, `Location[]`, or `LocationLink[]` (`targetSelectionRange` ⟶ `targetRange` fallback). `uri_to_path` strips `file://` and percent-decodes.

#### `struct LanguageConfig` (`mod.rs`) — `Clone, Debug, Serialize, Deserialize`
Persisted per-language toggles: `enabled: bool` (spawn at all), `check_on_save: bool` (send `didSave` → rust-analyzer runs `cargo check`), `format_on_save: bool` (run formatter on save — Phase 2, not wired). `Default` = all-on except `format_on_save`. `defaults_for(key)` gives Rust `check_on_save: true`, all others `false`.

#### `struct LanguageConfigs` (`mod.rs`) — `Clone, Debug, Default, Serialize, Deserialize`
`configs: HashMap<String, LanguageConfig>` keyed by the **Debug string** of `ServerKey` (`"RustAnalyzer"`, …) so it survives enum reordering. `get_or_default(key)` / `set(key, cfg)`. This is the part that persists in the session/`crane.yaml`.

#### `enum DownloadState` (`downloader.rs`) — `Clone, Debug`
`NotStarted`, `Downloading { progress_bytes: u64 }`, `Ready(PathBuf)`, `Failed(String)`.

#### `struct Downloader` (`downloader.rs`) — `Default`
`states: Arc<Mutex<HashMap<ServerKey, DownloadState>>>`. Methods: `resolved(key)` (fast-path trusts `Ready`, else stats `expected_path` and promotes), `state(key)`, `is_supported(key)` (RustAnalyzer always; npm-family iff `has_npm()`; Gopls `false`), `runtime_missing_hint(key)` (returns "Requires Node.js (npm) …" / "Requires Go …"), `start_download(key, ctx)`, `expected_path(key)` (under `~/.crane/lsp/<subdir>/…`). Free helpers: `has_npm()`, `has_node()` (`dead_code`), `human_bytes(n)`.

#### `struct LspManager` (`mod.rs`) — `Default`
The public façade. Fields:
- `servers: HashMap<ServerKey, Arc<LspServer>>` — live processes.
- `files: RwLock<HashMap<PathBuf, Vec<ServerKey>>>` — which servers each open file is attached to (a file can fan out to type-checker + linter).
- `downloader: Downloader` (pub).
- `declined: HashSet<ServerKey>` (pub) — servers the user declined to install this session (no re-nag until restart).
- `prompt_install: Option<ServerKey>` (pub) — the server currently being offered for install (drives the modal).
- `pending_files: RwLock<HashMap<ServerKey, Vec<(PathBuf, String)>>>` — files queued while a server is still downloading/spawning.

`LspServer` (private internals) holds `key`, `stdin: Arc<Mutex<Option<ChildStdin>>>`, `shared: Arc<(Mutex<Shared>, Condvar)>`, `next_id: AtomicI64`, `doc_versions: Mutex<HashMap<String,i32>>`, `_child`, `_ctx`. `Shared` holds `initialized`, `init_request_id`, `dead`, `last_stderr` (bounded to 4 lines), `pending_opens`, `diagnostics: HashMap<uri, Vec<Diagnostic>>`, `hover_results`, `definition_results`, `pending_kinds: HashMap<id, RequestKind>`.

---

### Behavior / data flow (no rendering)

**Editor → manager (the public API that the Files Pane calls):**
- `did_open(ctx, path, text, configs)` — routes via `keys_for_path`, filters by `enabled`, records `files[path]=keys`, calls `open_on_server` per key. `open_on_server`: evicts a `Dead` server, reuses a live one, else resolves a binary (PATH first via `which_on_path`, then downloaded copy), spawns + `did_open`, or queues into `pending_files` and (if supported, not declined, no prompt up, not already downloading) sets `prompt_install`.
- `did_change(path, text)` — version-bumps the doc, **drops stale diagnostics for the uri**, sends `textDocument/didChange` (full text).
- `did_save(path, text, configs)` — only fires `didSave` when `enabled && check_on_save`.
- `diagnostics(path) -> Vec<Diagnostic>` — merges diagnostics from every attached server. **This is the hot read the editor calls each repaint.**
- `goto_dispatch(path,line,char) -> Vec<(ServerKey,i64)>` (non-blocking fire) + `take_goto_result(key,id) -> Option<Option<Location>>` (poll). Replaced an older blocking call that froze the render thread up to 1.5s.
- `hover(...)` — `dead_code`, UI not wired; blocking, 800ms budget.
- `is_tracked(path)`, `statuses() -> Vec<(ServerKey, Status)>`, `last_stderr(key) -> Vec<String>` (for Settings).

**Per-frame ticks (called once per repaint from the app loop):**
- `shutdown_disabled(configs)` — drops `Arc<LspServer>` for any now-disabled language (triggers `Drop` → `graceful_shutdown`: `shutdown` request, 200ms, `exit`, 50ms, hard-kill fallback), strips the key from `files`/`pending_files`. NOTE (per memory `project_lsp_idle_shutdown`): only fires on config toggle, never on idle.
- `tick(ctx)` — early-exits when nothing changed; re-queues files for dead servers that now have a downloaded binary; drains `pending_files` via `try_spawn_pending`; raises `prompt_install` for a dead-from-PATH server that's supported, not declined, not downloading, and absent from PATH.
- `accept_install(ctx)` / `decline_install()` — consume `prompt_install`; accept kicks `downloader.start_download`, decline records in `declined`.

**Server internals (one process):** stdout reader thread parses framed JSON-RPC (`protocol::read`), dispatches in `handle_message`: `publishDiagnostics` → `parse_diagnostic` into `diagnostics[uri]`; id-bearing responses matched against `init_request_id` (sets `initialized`) and `pending_kinds` (Definition→`extract_location`, Hover→`extract_hover`); untracked ids dropped. stderr reader thread keeps the last 4 lines. Every message calls `ctx.request_repaint()`. `send_initialize` posts capabilities + rust-analyzer `checkOnSave`/`check` `initializationOptions`, then a worker thread waits up to 8s on the condvar for `initialized` and flushes `pending_opens` as `didOpen`.

---

### UI surfaces this area feeds (the only "rendered" parts)

There are **no widgets defined in these three files.** The rendered consumers are:

1. **Install-prompt modal** — driven by `LspManager.prompt_install: Option<ServerKey>`. When `Some`, the app shows a blocking modal (rendered in `main.rs`/settings, not here). Expected content per Crane modal conventions: title naming the server, body showing `install_hint()` (and `runtime_missing_hint()` if the runtime is missing), and two buttons — Install (→ `accept_install`) and Not now (→ `decline_install`). Phosphor glyph for the install action would be `DOWNLOAD_SIMPLE`; dismiss `X`. Colors/tokens per Crane modal style (accent button = accent token, dismiss = subtle border). **None of this is in the LSP files; it's the contract the modal must satisfy.**
2. **Settings → About/LSP status list** — driven by `statuses()` and `last_stderr(key)`. Per server: a status chip (`Ready`/`Initializing`/`Spawned`/`Dead`) and, when `Dead`, the captured stderr lines. Suggested chip colors: Ready = success/green token, Initializing/Spawned = warning/amber, Dead = error/red; glyphs `INFO`/`WARNING`/`X_CIRCLE`. Download progress would render `human_bytes(progress_bytes)` and a `DownloadState`-driven bar.
3. **Editor diagnostic rendering** — the Files Pane consumes `diagnostics(path)` to draw underlines/gutter markers (severity→color: 1 red/error, 2 amber/warning, 3/4 info/hint blue/gray). This is in `views/file_view.rs`, not here.

All three are **outside these files** — this area only produces the state they bind to.

---

### Interactions

There are no direct pointer/keyboard interactions in this module. Indirect ones:
- **Open a file** in the Files Pane → `did_open` (may raise install prompt).
- **Type** → `did_change` (clears+refreshes diagnostics).
- **Save (Cmd+S)** → `did_save` (rust-analyzer `cargo check`).
- **Goto-definition** (editor gesture, e.g. Cmd+click / F12 — wired in the editor view) → `goto_dispatch` then poll `take_goto_result`, navigating to `Location`.
- **Install prompt Install/Not-now buttons** → `accept_install` / `decline_install`.
- **Toggle a language in Settings** → mutate `LanguageConfigs`, next-frame `shutdown_disabled` tears the server down.

States: per-server `Status` (Spawned/Initializing/Ready/Dead), per-key `DownloadState` (NotStarted/Downloading{bytes}/Ready/Failed). No hover/selected/expanded/animation state in this layer (animations like a download spinner belong to the consuming UI).

---

### warpui port approach

warpui has no JSON-RPC LSP client today, so this is a near-greenfield port with a direct structural analog:

- **`LspManager` → a warpui background service / store.** Keep the same façade methods (`did_open`/`did_change`/`did_save`/`diagnostics`/`goto_dispatch`+`take_goto_result`). Process spawn + framed JSON-RPC + reader threads port 1:1 (std `Command`/threads, no async runtime — matches Crane's "no async" rule and warpui equivalents). Replace `egui::Context::request_repaint()` with warpui's `ctx.notify` / redraw-request equivalent so the UI re-polls `diagnostics()`.
- **`ServerKey`/`Status`/`Diagnostic`/`Location`/`DownloadState` enums/structs port verbatim** — pure data, no egui. `LanguageConfig(s)` stays `Serialize/Deserialize` for the config store; keep the Debug-string keying.
- **Install-prompt modal**: render as a warpui modal bound to `prompt_install`. Buttons = clickable hit-Rect-on-top + `dispatch_typed_action(AcceptInstall/DeclineInstall)`; icons via phosphor `Text` (`DOWNLOAD_SIMPLE`, `X`); colors from theme tokens (accent button, subtle dismiss). On action, mutate manager state and `ctx.notify`.
- **Settings LSP list**: iterate `statuses()`, render a row per server: phosphor glyph + label + status chip + (if Dead) stderr lines; for downloads bind to `DownloadState` and render `human_bytes` + a progress bar. Recursive-tree machinery is not needed (flat list).
- **Diagnostics in the editor**: feed `diagnostics(path)` into warpui's text-buffer renderer; map `severity` 1–4 → error/warning/info/hint theme tokens; `col_end == u32::MAX` ⟶ underline to EOL.
- **Downloader**: reuse `ureq` + `flate2` for rust-analyzer and shell-out `npm install` for the npm family; same `~/.crane/lsp/` layout (rename base dir if warpui uses a different home), same macOS quarantine `xattr -d` strip.

---

### Honest port-status: ~0%

None of this LSP subsystem exists in warpui today — no JSON-RPC client, no server lifecycle, no downloader, no diagnostics plumbing, no install modal, no Settings status list. The Crane implementation is mature and self-contained (pure data types + std threads + `ureq`/`flate2`/`npm`), so the port is mechanical but unstarted. Realistic completion estimate for this area in warpui: **0% implemented**, with the core types (`ServerKey`/`Diagnostic`/`Location`/`DownloadState`/`LanguageConfig`) being the cheapest first slice (verbatim copy) and the process/threading/reader layer being the bulk of the work.


---


<!-- ===== modals ===== -->

## Modals + Toasts

This area covers eight overlay surfaces in `src/modals/`: five **modals** (Find in Files, Settings, Settings→Language Servers sub-panel, Tab Switcher, New Workspace, LSP Install prompt) and three **toasts** (LSP download progress, Update toast, PTY Notification toast). All are immediate-mode egui overlays driven from `App` state every frame. There are no persistent widget objects; each `render(ctx, app)` reads `App` fields, paints, and writes results back into `App`.

Two egui overlay primitives are used and the port must distinguish them:
- **`egui::Modal`** (Settings, New Workspace) — paints a dimmed click-absorbing backdrop, handles Esc + background-click dismiss, returns `ModalResponse { should_close() }`.
- **`egui::Window`** with `.title_bar(false).anchor(CENTER_CENTER)` + `Frame::popup` (Find in Files, Tab Switcher, LSP Install) — centered floating panel, **no** backdrop dimming, no auto-dismiss (dismiss handled by app-level shortcut handler consuming Esc/⌘W).
- **`egui::Area`** with `.order(Order::Tooltip)` + `.fixed_pos(...)` (all three toasts + LSP download toast) — corner-anchored, non-modal, click-through except on explicit interact rects.

---

### Theme tokens used (canonical color names)

Across all eight files the theme tokens referenced are: `bg`, `surface`, `surface_alt`, `surface_hi`, `accent`, `text`, `text_hover`, `text_muted`, `border`, `selection`, `row_hover`, `row_active`, `success`, `warning`, `error`, `syntax_theme` (string). All are accessed via `theme::current()` (a global) returning a `Theme`, and converted with `.to_color32()`. Note `theme::current()` is called repeatedly inline (no caching) in most files; `find_in_files.rs` and the toasts bind a local `th`/`theme` once.

`linear_multiply(f)` is used to derive translucent variants: `accent.linear_multiply(0.25)` (scope active bg), `accent.linear_multiply(0.5)` (match-highlight background). `Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)` is the "selected sidebar/theme-row" fill (accent at alpha 55/255).

---

### 1. Find in Files (`find_in_files.rs`) — ⌘⇧F

**Public structs/enums:**
- `SearchScope` enum (`Clone,Copy,PartialEq,Eq`): `AllProjects`, `ActiveProject`, `ActiveWorkspace`. `.label()` → `"All Projects"|"Project"|"Workspace"`. `.tooltip()` → longer descriptions.
- `SearchMatch`: `path: PathBuf`, `display_path: String` (`"<root_name>/<rel>"`), `line: u32`, `byte_start/byte_end: usize` (byte offsets of the match span **within the matched line**), `line_text: String`.
- `SearchResults`: `matches: Vec<SearchMatch>`, `files_seen: HashSet<PathBuf>`, `truncated: bool`, `running: bool`, `error: Option<String>`, `token: u64`. Shared as `Arc<Mutex<SearchResults>>` between UI and worker thread.
- `FindInFilesState` (held in `App::find_in_files: Option<…>`): `query`, `case_sensitive`, `whole_word`, `regex` (all defaults false), `file_mask: String`, `scope` (default `AllProjects`), `results: Arc<Mutex<…>>`, `selected: usize`, `last_query_at: Option<Instant>`, `pending_kick: bool`, `search_token: u64`, `focus_input: bool` (default true), `cancel_flag: Option<Arc<AtomicBool>>`, `preview_cache: Option<(PathBuf, Vec<String>)>`.
- Internal: `SearchRoots`, `CollectSink<'a>` (impls `grep_searcher::Sink`), `SearchOutcome` enum.

**Constants:** `DEBOUNCE = 150ms`, `MAX_RESULTS = 1000`, `PREVIEW_CONTEXT = 10` (lines each side), `MODAL_W = 880`, `MODAL_H = 620`, `QUERY_RIGHT_PAD = 230`, row height `22`, preview row height `18`.

**Layout (top→bottom, `egui::Window` titlebar-off, fixed 880×620, anchor CENTER_CENTER offset `[0,-40]`, `Frame::popup` inner margin 14):**
1. **Header row** (`render_header`): bold `"Find in Files"` size 14; then muted size-11 `"{n} matches in {m} files"` (or `"{n}+ matches in {m}+ files"` when truncated, shown only when query non-empty); right-aligned frameless close button — phosphor `X` size 13, min 22×22, tooltip `"Close (Esc)"`, sets `clicked_x`.
2. `add_space(8)`. **Query row** (`render_query_row`): phosphor `MAGNIFYING_GLASS` (muted) + `TextEdit::singleline` (id `find_in_files_query`, hint `"Find…"`, width `MODAL_W-28-230` min 200, height 26). `focus_input` gates a one-shot `request_focus()`. Then three `toggle_pill`s: `"Aa"` (case sensitive), `"W"` (whole word), `".*"` (regex). On query `changed()` or any pill toggle: set `last_query_at=now`, `pending_kick=true`, `selected=0`.
3. `add_space(4)`. **Mask row** (`render_mask_row`): muted size-11 label `"File mask"` + `TextEdit::singleline` (id `find_in_files_mask`, hint `"*.rs, *.toml"`, width 220, height 22) + size-10 muted `"(comma-separated globs)"`. On change → kick.
4. `add_space(6)`. **Scope row** (`render_scope_row`): three buttons (`All Projects`/`Project`/`Workspace`), height 24, corner_radius 4. Active: fill `accent×0.25`, text `text_hover`, stroke `accent`. Inactive: fill transparent, text `text_muted`, stroke `border`. Pointing-hand cursor on hover; tooltip from `.tooltip()`. Click (when not already active) switches scope + kicks.
5. `add_space(6)`, `separator()`.
6. **Results list** (`render_results_list`) allocated in a region of height `available*0.42` min 140. Empty state: centered muted size-12 message — `"Type to search across files"` / `"Searching…"` / `"Search failed"` / `"No matches"`. Non-empty: `ScrollArea::vertical` (id_salt `find_in_files_results`, auto_shrink false) using `show_rows` with row_h 22. Auto-scroll: compares `selected` against a temp-memory `find_in_files_prev_sel`; when changed, sets `vertical_scroll_offset((selected*22 - 80).max(0))`. Each row (`allocate_exact_size`, `Sense::click`): selected → `rect_filled(3.0, row_active)`; else hovered → `rect_filled(3.0, row_hover)`. Left: the matched line rendered as a `LayoutJob` (monospace 12, color `text` or `text_hover` if selected) with the match span [byte_start,byte_end) painted with `background = accent×0.5`, `color = WHITE`; wrapped to `text_w` (1 row, no break_anywhere). Right: `"{display_path}  :{line}"` proportional 11 muted, right-pinned. `byte_floor()` clamps offsets to char boundaries. Hover → pointing hand. Click → `selected = i`, clear preview cache. Double-click → `open_request`.
7. `separator()`, then **status line** (`status_line`): muted size-11 — `"Error: {e}"` / blank / `"{n} matches"` / `"{n}+ matches"` / appends `" — searching…"` while running.
8. `add_space(2)`. **Preview** (`render_preview`) in region height `available-list_h-28` min 120: header = full path size-11 `text_hover`; then `ScrollArea::vertical` (id_salt `find_in_files_preview`) showing lines `[target-10 .. target+10+1]`. Each line is `allocate_exact_size(w,18)`. Hit line: `rect_filled(0.0, row_active)` + the match span highlighted (`accent×0.5` / WHITE). Gutter: right-aligned `"{:>5}"` line number, monospace 11 muted, at x+4; line text monospace 12 `text` at x+56, wrapped to `w-56` single row. Preview content is cached in `preview_cache` keyed by path (read from disk via `std::fs::read_to_string`).

**Search engine / threading:** Debounce loop in `render`: if `pending_kick && last_query_at.elapsed() >= 150ms`, call `spawn_search`; while `pending_kick`, `request_repaint_after(DEBOUNCE)`. `spawn_search` cancels the prior worker (`cancel_flag.store(true)`), bumps `search_token`, resets `results`, spawns a `std::thread` running `run_search`. Worker builds a `grep_regex::RegexMatcherBuilder` (case_insensitive/case_smart = `!case_sensitive`, `word(whole_word)`; literal queries escaped via `escape_regex`), walks each root with `ignore::WalkBuilder` (hidden true, all git-ignore layers on, parents true, no follow-links), applies comma-split glob overrides, and feeds files to `grep_searcher::Searcher` with `CollectSink`. Sink pushes every submatch span per matched line (via `matcher.find_iter`), checks `token` mismatch / `cancel` flag / `MAX_RESULTS` to bail, and `request_repaint()` at ≤20Hz (50ms throttle). On finish sets `running=false`.

**Keyboard nav** (handled in `render` after the window, consuming keys with `Modifiers::NONE`): ArrowUp/Down move `selected` ±1, PageUp/Down ±10 (saturating), each clears `preview_cache`; Enter → `open_request` from selected match. (Esc/⌘W close is handled by the global `shortcuts.rs`, not here.)

**Open action:** `open_request: Option<(PathBuf, line0, byte_start)>`. Reads file, opens into active layout via `app.open_file_into_active_layout` (read-only flag = not inside active workspace), finds the matching `Files` pane tab, sets `files.active` and `ft.pending_cursor = Some((line, 0))`, then `close(app)`. `close()` also sets the cancel flag.

**warpui port approach:** Reuse the recursive overlay + hit-rect pattern. Modal frame = a centered fixed-size panel painted on top with a popup background; the toggle pills, scope buttons, and result rows are all **hit-Rect-on-top + dispatch_typed_action** (painted rect + an interaction sense rect, click dispatches a typed action: `SetScope`, `TogglePill(kind)`, `SelectMatch(i)`, `OpenMatch(i)`). Icons = phosphor `Text` glyphs (`MAGNIFYING_GLASS`, `X`). The match-span highlighting is a 3-segment `LayoutJob` — port verbatim, including `byte_floor` char-boundary clamping. The debounced background search → a worker task writing into an `Arc<Mutex<SearchResults>>` with a monotonic `search_token`; `ctx.notify` / `request_repaint` to wake the UI. Auto-scroll-to-selected via remembered prev-selection in temp memory. Reuse `ignore`+`grep` stack as-is (no port needed).

**Port status: ~10%.** Engine + state model are well-specified but the dual-galley row rendering, debounce/token worker plumbing, preview cache, and keyboard nav are all unbuilt in warpui.

---

### 2. Settings modal (`settings.rs`)

**Public:** `SettingsEffect` enum (`None`, `ReloadFonts`) — returned to caller so it can rebuild egui fonts. Gated on `app.show_settings: bool`. Section enum is `state::SettingsSection` (variants `Appearance`, `Editor`, `Terminal`, `LanguageServers`, `Shortcuts`, `About`; provides `ALL`, `.icon()` phosphor glyph, `.label()`). Constants: `SIDEBAR_W=200`, `WIN_W=960`, `WIN_H=640`.

**Layout:** `egui::Modal` (id `settings_modal`), 960×640. Header: `ui.heading("Settings")` + right-aligned frameless phosphor `X` close button (size 13, min 22×22, tooltip `"Close"`). `separator()`. Then `horizontal_top`: **sidebar** (200 wide) | `separator` | **content** (`vertical`, fixed height, `ScrollArea::vertical` id_salt `("settings_section", section as u32)`). Dismiss when `modal_resp.should_close()` (Esc / backdrop click) or close button. Theme change applies immediately via `theme::set` + `apply_style(ctx)`.

**Sidebar** (`render_sidebar`): each section is `allocate_exact_size(194×32, click)`. Selected → fill `accent@alpha55`, text `text`. Hovered & not selected → fill `row_hover`. Painted with `painter().text`: icon glyph (`FontId 15 Proportional`) at x+12, label (`FontId 13`) at x+36, both at `center().y`, `Align2::LEFT_CENTER`. Pointing-hand on hover. Click sets `app.settings_section`. 2px gap between rows, corner_radius 6 on fill.

**Appearance section** (`render_appearance`):
- `section_title("Appearance")` (size-16 bold + separator).
- `setting_row("Editor / Terminal font size", …)`: a `Slider` (9.0..=28.0, step 1, `trailing_fill`) with widget bg colors overridden (`surface_alt`/`surface_hi`/`accent`) + a `small_button("Reset")` → 14.0. Below: italic muted size-10.5 tip `"Cmd + / Cmd − / Cmd 0 also resize…"`.
- `setting_row("Monospace font", …)`: muted label of current font file-name (or `"Default"`) + `small_button("Choose…")` opening `rfd::FileDialog` filtered to ttf/otf (sets `custom_mono_font`, `reload_fonts=true`) + conditional `small_button("Reset")`.
- `"Theme"` bold size-12.5, then a `ScrollArea` (id_salt `settings_themes`, max_height `available-62`) of theme rows. Each row `allocate_exact_size(w×44, click)`, corner_radius 6: active fill `accent@55`, else `surface`; hovered-not-active fill `row_hover`. Paints 5 color **swatches** (12×12, 2px radius, 4px gap) from `[bg, surface, accent, text, selection]`, then theme `name` (`FontId 13`, color `text`) at `x = 12 + 16*5 + 8`. Active row paints right-aligned phosphor `CHECK` (size 14, accent). Click-not-active → `theme_change = Some(name)`.
- Muted size-11 line `"Custom themes (.toml) live in {themes_dir}"` + `small_button("Open themes folder")` (creates dir, `super::open_in_file_manager`).
- `setting_row("Syntax highlighting", …)`: `ComboBox` (id_salt `syntax_theme_override`, width 220) — `selected_text` is the override or `"Auto ({syntax_theme})"`; options: `selectable_label` `"Auto (pair with UI theme)"` (→ None), separator, then each `available_syntax_themes()` (→ Some(name)).

**Editor section** (`render_editor`): three `ui.checkbox`es — `editor_word_wrap` `"Word wrap"`, `editor_trim_on_save` `"Trim trailing whitespace on save"`, `single_click_open` `"Single-click to open files (preview tab)"` — then a `placeholder` italic muted note.

**Terminal section** (`render_terminal`): title + `placeholder` note only (no controls).

**Shortcuts section** (`render_shortcuts`): `ScrollArea` id_salt `settings_shortcuts` + `egui::Grid` `shortcuts_grid` (2 cols, spacing [18,6]) — 20 rows of `(monospace bold key, plain desc)`. Full table is enumerated in the source (⌘O … Ctrl+C/Ctrl+D).

**About section** (`render_about`): bold size-22 `"Crane"`, muted `"Version {CARGO_PKG_VERSION}"`, two descriptive lines, then a row of three `ui.button`s: `"GitHub"` / `"Releases"` (open URLs via `webbrowser`) / `"Check for updates"` (resets `update_check` fields + `spawn_check`). If an update is available, accent bold `"Update available: v{version}"`.

**Shared helpers:** `setting_row` = a bordered `Frame` (fill `surface`, stroke `border`, corner 6, margin sym(12,10)) with a label on the left (size-12.5 bold `text`) and right-aligned content. `section_title` = size-16 bold + separator. `placeholder` = italic muted size-12.

**warpui port approach:** The whole modal = backdrop-dimming Modal + a 200px sidebar of `dispatch_typed_action(SelectSection)` hit-rects (icon+label painted, not egui widgets) + a scrollable content pane that match-dispatches on the section enum. Theme rows and sidebar rows are pure painted rects with click sense — reuse the hit-Rect pattern; swatches are `rect_filled` loops. Sliders/checkboxes/combos map to warpui equivalents writing back into typed `App` fields, with a `ReloadFonts`/`ThemeChange` effect returned to the caller. Reuse phosphor `X`/`CHECK` and `section.icon()` glyphs.

**Port status: ~15%.** Structure is simple and most controls are standard, but sidebar/theme-row custom painting, the combo, the font picker (rfd), and the effect-return wiring are unported.

---

### 3. Settings → Language Servers (`settings_lsp.rs`)

Rendered inline inside the Settings content pane (not its own modal). Top: `section_title("Language Servers")` + explanatory muted size-11.5 paragraph. `ScrollArea` id_salt `lsp_list`. First a truncated `"PATH: {first 90 chars}…"` (monospace 10.5 muted). Then one **row per `ServerKey`** in fixed order: `RustAnalyzer, TypeScript, Eslint, Gopls, Pyright, CssLs, HtmlLs`, 8px gaps.

**Per-row** (`render_lsp_row`, bordered `Frame` fill `surface`/stroke `border`/corner 6/margin sym(12,10)):
- Header: bold size-13 `"{key:?}"` (Debug name) + right-aligned **status chip** (`status_chip`): label+color from server `Status` (`ready`/success, `initializing`/warning, `starting`/warning, `dead`/error) else download state (`downloading`/warning, `installed (not started)`/text_muted when on PATH or downloaded, `not installed`/text_muted).
- `"$ {cmd}"` monospace 10.5 muted (the binary name from `key.command()`).
- PATH line: `"PATH → {path}"` monospace 10.5 **success** if found via `which_on_path`, else `"PATH → not found"` muted.
- If `DownloadState::Ready(p)`: `"Crane → {path}"` monospace 10.5 success.
- **Dead-server diagnostic panel** (only when `Status::Dead` and stderr non-empty): joined stderr lines in monospace 10.5 **error**. Special-case rustup shim missing (RustAnalyzer + stderr contains `"Unknown binary 'rust-analyzer'"`): italic muted explanation + an `"Install via rustup"` strong button that fire-and-forget spawns `rustup component add rust-analyzer` (stdio nulled).
- **Per-language toggles** (read `app.language_configs.get_or_default(key)`, write back via `.set` on any change): `checkbox` `"Enable language server"`; an `add_enabled(cfg.enabled, …)` check-on-save checkbox whose label varies by key (`"Run cargo check on save…"` / `"Run go vet / build on save"` / `"Notify server on save"`); a `format_on_save` checkbox whose label varies (`"Format on save (rustfmt)"` / `(prettier)` / `(ruff)` / `(gofmt)` / `"(ESLint fixes via Prettier)"`).
- **Download/install row**: if `Downloader::is_supported(key)`, match `dl_state`: `Downloading` → `"⬇ downloading… {human_bytes}"` italic warning; `Ready` → `"✓ downloaded by Crane"` success + `small_button("Re-download")`; `Failed(e)` → `"✗ {e}"` error + `small_button("Retry")`; `NotStarted` → strong button `"⬇ Download & use Crane's copy"` (removes from `declined`, starts download). Else if `runtime_missing_hint` → italic warning hint. Else → `"install yourself: {install_hint}"` monospace 10.5 **accent**.

> Note: this file uses raw Unicode glyphs `⬇ ✓ ✗` in strings (violates the project's phosphor-only rule) — the warpui port should replace them with `DOWNLOAD_SIMPLE`, `CHECK`, `X`.

**warpui port approach:** Recursive list over the fixed `ServerKey` array → one bordered card per key. Status chip = a colored text token computed from `(Status, DownloadState, found_on_path)`. Checkboxes write typed `LanguageConfig` back through `app.language_configs.set`. Buttons dispatch typed actions (`StartDownload(key)`, `RustupAddComponent`, etc.). Reuse `which_on_path` PATH scan as-is.

**Port status: ~5%.** Depends on the LSP subsystem (`app.lsp`, `Downloader`, `language_configs`) which must exist first; UI is straightforward once those are ported.

---

### 4. Tab Switcher (`tab_switcher.rs`) — ⌘` / ⌘~

**Public:** state lives in `App::tab_switcher: Option<TabSwitcherState>` (`{ entries: Vec<(ProjectId,WorkspaceId,TabId)>, highlight: usize, cmd_was_held: bool }`). Two public fns:
- `advance_or_open(app, backward) -> bool`: collects MRU-ordered live entries (`collect_entries`); if <2, no-op returns false. First tap opens with `highlight = 1` (the *previous* tab, alt-tab style). Subsequent taps wrap `highlight` ±1. Returns true to tell the shortcut handler to suppress other handling this frame.
- `render(ctx, app) -> bool`: paints the overlay and commits on Cmd release / click / Esc. Returns true on commit/cancel.

**Layout:** `egui::Window` titlebar-off, anchor CENTER_CENTER `[0,0]`, `Frame::popup` margin sym(14,10), `min_width 460`. Header muted size-11 `"Switch tab"`. `ScrollArea::vertical` (max_height 340, auto_shrink `[false,true]`): one row per label, `allocate_exact_size(w×22, click)`. Highlighted row → `rect_filled(4.0, row_active)`. Label painted with `painter().text` at x+8, `FontId 12 Proportional`, color `text`, `Align2::LEFT_CENTER`. Footer muted size-10.5 `"Cmd+\` next · Cmd+~ previous · release Cmd to commit · Esc cancel"`. Labels = `"{project} / {workspace_label} / {tab_name}"` (`label_for`).

**Interaction / commit logic:** `request_repaint_after(30ms)` keeps the loop ticking to observe Cmd release. Cmd-held detection: on macOS uses `crate::mac_keys::is_cmd_held()` (NSEvent-sourced, more reliable than egui's modifier state on idle frames); elsewhere `i.modifiers.mac_cmd || command`. Esc is consumed (`consume_key`) so it doesn't leak into the terminal. Close priority: **Esc > click > Cmd-release**. `commit(app, idx)` takes the state, re-validates the target tab still exists, then sets `app.active`, `last_workspace`, project `last_active_workspace`, and workspace `active_tab`. `collect_entries` = MRU (`app.tab_mru`) filtered to live, then all remaining tabs appended.

**warpui port approach:** This is a transient cycle-overlay, not a search modal — keep that semantic. Port `advance_or_open` as the ⌘`/⌘~ keybinding entry. Rows = painted hit-rects (`SelectAndCommit(i)` on click). The Cmd-release commit is the critical mechanic: warpui needs an OS-level Cmd-held probe (equivalent to `mac_keys::is_cmd_held`) plus a per-frame repaint tick; commit when `was_held && !cmd_held`. Esc-consume must beat the terminal handler.

**Port status: ~10%.** MRU model and commit/validate logic are clear; the OS Cmd-held probe and repaint-tick loop are the load-bearing port risk.

---

### 5. New Workspace / New Worktree (`new_workspace.rs`)

Gated on `App::new_workspace_modal: Option<…>` (the modal struct lives in `state`, fields used here: `project_id`, `branch: String`, `branch_locked: bool`, `create_new_branch: bool`, `mode: state::LocationMode` {`Global`,`ProjectLocal`,`Custom`}, `custom_path: String`, `error: Option<String>`, methods `resolved_parent(path,name)`). Width 480.

**Layout** (`egui::Modal` id `new_workspace_modal`, min_width 460): `heading("New Worktree")`. **Branch**: bold label + `TextEdit::singleline` (hint `"feature/my-branch"`, width 440, `add_enabled(!branch_locked)`). If `branch_locked` → muted size-11 rgb(130,136,150) note `"Checking out existing branch into a new worktree"`; else `checkbox(create_new_branch, "Create new branch")`. **Location**: bold label + a horizontal row of three `selectable_value`s (`Global` / `Project-local` / `Custom`) each with a hover tooltip describing its path template. If `Custom`: a `TextEdit` (hint `"/path/to/parent"`, width 440-88) + `"Browse…"` button (sets `browse`). **Preview**: truncated `Label` `"→ {resolved_parent}/{branch or <branch>}"` size-10.5 rgb(130,136,150). If `error`: `colored_label(rgb(220,100,100), err)`. **Actions**: `button("Create")` strong → `create`, `button("Cancel")` → `cancel`.

**Resolution:** dismiss on `should_close()` or cancel. `browse` opens `rfd::FileDialog::pick_folder` (start dir = custom_path or home/cwd), writes `custom_path` + forces `mode=Custom`. `create` → `app.create_workspace_from_modal(ctx)`.

> Note: this file uses hardcoded `Color32::from_rgb(...)` instead of theme tokens — the port should map rgb(130,136,150)→`text_muted`, rgb(220,100,100)→`error`.

**warpui port approach:** Standard backdrop modal with typed-action buttons (`CreateWorkspace`, `CancelModal`, `BrowseFolder`). Text fields write into the typed modal state. `selectable_value` trio → a small segmented control writing `LocationMode`. The live preview string recomputes from `resolved_parent` each frame. rfd folder picker stays native.

**Port status: ~15%.** Smallest modal; mostly standard widgets, blocked only on the `create_workspace_from_modal` git-worktree backend.

---

### 6. LSP Install prompt (`lsp_install.rs`)

Gated on `app.lsp.prompt_install: Option<ServerKey>` and globally suppressed by `app.lsp_install_prompts_disabled`. `egui::Window` titlebar-default `"Install language server?"`, anchor CENTER_CENTER, fixed 440×240, non-collapsible/non-resizable. Body: size-12.5 `text` `"Crane can't find {bin} on your PATH — it's needed for {lang} diagnostics…"`; muted size-12 `install_blurb` (from `describe(key)` — per-key text, e.g. rust-analyzer "~15 MB from rust-lang/rust-analyzer into ~/.crane/lsp/"). Four buttons in a row: strong `"Download & use"` (→ `accept_install`), `"Not now"` (→ session decline), `"Never ask again"` (decline + disable this language + save Settings), `"Never ask for any language"` (decline + set `lsp_install_prompts_disabled` + save). Footer italic muted size-10.5 explainer.

`render_download_toast(ctx, app)` (lives in this file but is a **toast**): iterates all 7 `ServerKey`s; for each `Downloading{progress_bytes}` paints an `egui::Area` (`Order::Tooltip`, id `("lsp_dl_toast", key)`) at `(screen.max.x - 280, screen.min.y + 60)` — a 248-wide `Frame` (surface/border/corner 8/margin 10) with strong `"Downloading {key:?}…"` + muted `human_bytes(progress)`. `request_repaint_after(150ms)`.

**warpui port approach:** Modal with four typed-action buttons. The download toast = a top-right `Area` per active download (reuse the toast pattern below), `ctx.notify`-driven repaint while bytes climb. `describe(key)` is a static lookup table — port verbatim.

**Port status: ~5%.** Blocked on the LSP downloader subsystem.

---

### 7. Update toast (`update_toast.rs`)

Gated on `app.update_check.should_show()` (and a separate "up to date" branch when `manual_check && !manual_result_seen`). Bottom-right `egui::Area` (`Order::Tooltip`, id `update_toast`) at `(screen.max.x - toast_w - 20, screen.max.y - 140)`, `toast_w = min(440, screen.width-40)`. `Frame` fill `surface`, stroke `border`, corner 10, margin 14, inner width `toast_w-28`.

**Header row:** phosphor `ARROW_CIRCLE_UP` size 18 **accent** + a vertical block: strong size-13 `"Crane v{version} is available"`, muted size-11.5 `"You're on v{CARGO_PKG_VERSION}. Grab the new build?"`. Then `add_space(10)` and a state machine on `app.updater.state()` (`UpdateState`):
- `Downloading{bytes}` → italic `"{DOWNLOAD_SIMPLE}  Downloading… {human_bytes}"`, repaint 150ms.
- `Installing` → italic `"Installing…"`, repaint 300ms.
- `Ready` → row: `button("Later")` (→ `dismiss_session`) on left; right-aligned strong `"{ARROW_COUNTER_CLOCKWISE}  Restart now"` (→ `apply_and_exit`). Layout deliberately separates the destructive action.
- `Failed(err)` → error label `"Install failed: {err}"` + `button("Open in browser")` (→ webbrowser open `url`).
- `Idle` → if `unsupported_reason` (snap/flatpak/apt): show reason + `"Got it"` (dismiss_session) + `"Remind in 7 days"` (remind_later). Else: if `supports_in_app` strong `"{DOWNLOAD_SIMPLE}  Install update"` (→ `updater.start(asset_urls, ctx)`) else strong `"{DOWNLOAD_SIMPLE}  Download"` (→ webbrowser + `dismiss_forever`); plus `"Not now"` (dismiss_session) and `"Remind in 7 days"` (remind_later).

`release_urls_for(version)` builds platform-specific GitHub asset URLs (macOS arch-dmg then universal-dmg; Linux x86_64 tarball). `human_bytes` formats B/KB/MB.

**"Up to date" toast** (`render_up_to_date`): `egui::Area` id `update_toast_uptodate` at `(max.x - w - 20, max.y - 90)`, `w = min(320, …)`. `CHECK_CIRCLE` size 16 **success** + strong `"You're up to date (v{CARGO_PKG_VERSION})"` + right-aligned frameless `X` close (size 11) → sets `manual_result_seen=true`, `manual_check=false`. `request_repaint_after(6s)` auto-dismiss.

**warpui port approach:** Corner-anchored `Area` (no backdrop). The `UpdateState` match → a typed render-state; each button dispatches a typed action (`DismissSession`, `RemindLater`, `StartUpdate`, `ApplyAndExit`, `OpenBrowser`). Progress toasts need a `ctx.notify`/repaint tick. Reuse phosphor `ARROW_CIRCLE_UP`/`DOWNLOAD_SIMPLE`/`ARROW_COUNTER_CLOCKWISE`/`CHECK_CIRCLE`/`X`. `release_urls_for` + `human_bytes` port verbatim.

**Port status: ~5%.** Blocked on the `update`/`updater` subsystem; UI itself is mechanical.

---

### 8. PTY Notification toast (`notification_toast.rs`)

Surfaces OSC 9 / OSC 777 desktop notifications from terminal programs (Claude Code Stop/Notification hooks, build scripts). Drains `App::pending_notifications: VecDeque<PaneNotification>` into `App::active_notification: Option<PaneNotification>` (one at a time, only when none active). `PaneNotification` fields used: `body: String`, `urgent: bool`, `created_at: Instant`, plus source ids resolved via `app.notification_source_names(&n) -> (proj, ws, tab)`. TTL: `NORMAL_TTL = 5s`, `URGENT_TTL = 12s`.

**Logic:** On rotate-in, if `!window_focused` fire an OS banner via `fire_os_notification` (`notify_rust::Notification`, title `"Crane — {proj} / {ws}"` + `" (urgent)"`, before latching so a burst still yields one banner per event). Expire when `created_at.elapsed() >= ttl`. `request_repaint_after(min(remaining, 500ms))` to drive the TTL clock on idle frames.

**Layout:** Bottom-right `egui::Area` (`Order::Tooltip`, id `crane_pty_notification_toast`) at `(max.x - 420 - 20, max.y - 84 - 28)`, `toast_w = min(420, …)`, `toast_h = 84`. `Frame` fill `surface`, corner 10, margin 12; **stroke** = `error` if urgent else `border`. Inner width `toast_w-24`. Header glyph = phosphor `WARNING` (urgent) or `INFO`, size 18, color = `error` (urgent) / `accent`. Vertical block: a breadcrumb row (item_spacing.x=4) — `proj_name` size-13 strong in header_color, then `"{CARET_RIGHT}  {ws}  {tab}"` size-11 muted; below, the body (`truncate_body` to 180 chars, appends `…`) size-12 `text`. Right-aligned frameless `X` close (size 13 muted, min 22×22, pointing-hand on hover) → clears `active_notification`.

**Click-through:** the full inner `min_rect` (minus close column) is re-`interact`ed (`area_id.with("body_click")`, `Sense::click`); hover → pointing hand; click → `app.focus_notification_source(&notif)` + clear active.

**warpui port approach:** Queue-drain pattern (`VecDeque` → single active slot) + TTL clock with `request_repaint_after`. Toast = corner `Area`; body click = one big hit-rect dispatching `FocusNotificationSource(id)`, close button = small hit-rect dispatching `DismissNotification`. Urgent vs normal = stroke/icon/color swap (`WARNING`/`INFO`, `error`/`accent`/`border`). OS banner via the platform notification API equivalent of `notify_rust`, fired only when window unfocused. Reuse phosphor `WARNING`/`INFO`/`CARET_RIGHT`/`X`.

**Port status: ~10%.** The OSC 9/777 ingestion → `pending_notifications` exists conceptually in the terminal layer; the queue-drain + TTL + breadcrumb toast UI + OS-banner fallback are unported.

---

### Cross-cutting port notes
- **Two dismiss models:** `egui::Modal` (Settings, New Workspace) gives free Esc + backdrop-click dismiss via `should_close()`; `egui::Window`-popups (Find/Tab-switcher/LSP-install) rely on the app-level shortcut handler consuming Esc/⌘W. warpui must replicate both: a dimming-backdrop modal primitive AND an undimmed centered-popup primitive whose Esc is wired through the global key handler.
- **All clickable custom rows** (Find results, settings sidebar/theme rows, tab-switcher rows) follow the identical pattern: `allocate_exact_size(rect, Sense::click)` → `painter().rect_filled(corner, fill)` for selected/hover state → `painter().text/galley` for content → pointing-hand on hover → click dispatches. Port once as a reusable `hit_row` helper.
- **Toasts** all share: `egui::Area` + `Order::Tooltip` + `fixed_pos` from `ctx.content_rect()` corners + surface/border `Frame` with corner 8–10 + a `request_repaint_after` tick to drive timers/progress on idle frames. Port once as a `corner_toast` helper.
- **Theme drift to fix during port:** `new_workspace.rs` hardcodes rgb colors (→ `text_muted`/`error`), `settings_lsp.rs` uses raw `⬇✓✗` glyphs (→ phosphor `DOWNLOAD_SIMPLE`/`CHECK`/`X`).
- **Overall area port status: ~10%.** State models and behaviors are fully specified here, but essentially all custom-painted rows, the dual overlay primitives, the toast/timer plumbing, and the LSP/update/git backends they depend on remain to be built in warpui.


---


<!-- ===== shell-infra ===== -->

## App Shell + Infrastructure + Theme

This section documents the top-level application shell (`main.rs`), startup/font/PATH infrastructure (`startup.rs`), the global keyboard shortcut dispatcher (`shortcuts.rs`), the macOS NSEvent monitor (`mac_keys.rs`), the native macOS menu (`platform_menu.rs`), the threaded job system (`jobs/system.rs`), the filesystem watcher (`file_watcher.rs`), the directory listing cache (`dir_cache.rs`), the Prettier-discovery / formatter shell-out (`format/mod.rs`), the staged auto-updater (`update/apply.rs` + `update/check.rs`), and the complete theme system (`theme.rs`). This is the chrome and plumbing that wraps every other Pane/Panel area — it owns the per-frame render order, the window-region geometry math, the focus modal pipeline, and the color tokens every other area reads from.

---

### 1. Theme system (`theme.rs`) — colors, tokens, palettes

This is the single most load-bearing file for visual reproduction. **Every color in the entire app resolves through a `Theme`.** warpui must replicate this token set verbatim.

#### 1.1 `struct Rgb`
```rust
#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }
```
- `Rgb::new(r,g,b)` — const constructor.
- `Rgb::to_color32()` → `egui::Color32::from_rgb(r,g,b)` (the egui path).
- `Rgb::to_warp()` → `warpui::color::ColorU::new(r,g,b,255)` — **already written for the migration.** This is the bridge the warpui port consumes. The dual-path (`to_color32` + `to_warp`) is intentional; both coexist until egui is removed. Port consumers call `.to_warp()`.

#### 1.2 `struct Theme` — the token palette (26 named tokens)
```rust
pub struct Theme {
    pub name: String,
    // Surfaces / chrome
    pub bg, sidebar_bg, topbar_bg: Rgb,
    pub surface, surface_alt, surface_hi: Rgb,   // 3-step elevation ramp
    // Lines
    pub border, border_strong, divider: Rgb,
    // Text ramp
    pub text, text_hover, text_muted, text_header: Rgb,
    // Semantic
    pub accent: Rgb,
    pub row_hover, row_active: Rgb,
    pub focus_border, inactive_border: Rgb,
    pub error, success, warning: Rgb,
    // Terminal
    pub terminal_bg, terminal_fg: Rgb,
    #[serde(default)] pub selection: Rgb,        // text/cell selection bg
    #[serde(default)] pub syntax_theme: String,  // syntect theme display name
}
```

**Token semantics (how each is used app-wide — the contract warpui must honor):**
| Token | Used for |
|---|---|
| `bg` | window/canvas fill (`ui.painter().rect_filled(full, 0.0, bg)`), `panel_fill`, `extreme_bg_color` |
| `sidebar_bg` | Left Panel + Right Panel fill |
| `topbar_bg` | Main Panel top bar |
| `surface` / `surface_alt` / `surface_hi` | widget inactive / hovered / active fills respectively; `surface` also = `code_bg_color`, `window_fill` |
| `border` / `border_strong` | widget inactive vs hovered/active strokes; 1px |
| `divider` | 1px vertical line between panels; git-log splitter uses hardcoded `Color32::from_rgb(36,40,52)` (NOTE: this is a bug — it should read `divider`; warpui should use `divider` token) |
| `text` / `text_hover` / `text_muted` / `text_header` | primary / hover / dimmed / section-header text |
| `accent` | focus, selection stroke, primary action; selection bg uses accent @ alpha 70 |
| `row_hover` / `row_active` | list row hover/selected bg; `row_hover` = `faint_bg_color` |
| `focus_border` (used as 2px accent on active Pane) / `inactive_border` (subtle border on other Panes) | |
| `error` / `success` / `warning` | diagnostics, git status, toasts |
| `terminal_bg` / `terminal_fg` | terminal pane grid |
| `selection` | terminal cell selection (falls back to accent @ ~28% if missing in old TOMLs) |

#### 1.3 Built-in themes — `Theme::builtins()` returns 21 themes in this exact order:
`dark` (`crane-dark`, default), `light` (`crane-light`), `tokyo-night`, `dracula`, `catppuccin-mocha`, `gruvbox-dark`, `nord`, `rose-pine`, `catppuccin-latte`, `coffee`, `warm-neon`, `one-dark`, `solarized-dark`, `solarized-light`, `monokai`, `darcula`, `github-dark`, `vscode-dark`, `vscode-light`, `high-contrast-dark`, `high-contrast-light`.

Each is a hardcoded constructor (`Theme::dark()` … `Theme::high_contrast_light()`). **All 26 field values for all 21 themes are in the file and must be copied byte-for-byte** — they are the ground truth. Default dark example (the canonical palette): `bg=(14,16,24)`, `sidebar_bg=(18,20,28)`, `topbar_bg=(20,22,32)`, `surface=(40,45,60)`, `surface_alt=(56,62,82)`, `surface_hi=(72,80,104)`, `border=(60,66,86)`, `border_strong=(96,106,132)`, `divider=(36,40,52)`, `text=(212,216,228)`, `text_hover=(234,238,248)`, `text_muted=(150,156,172)`, `text_header=(140,146,162)`, `accent=(90,135,220)`, `row_hover=(30,34,46)`, `row_active=(48,56,80)`, `focus_border=(100,140,220)`, `inactive_border=(36,40,52)`, `error=(220,110,110)`, `success=(120,210,140)`, `warning=(220,180,110)`, `terminal_bg=(14,16,24)`, `terminal_fg=(176,180,192)`, `selection=(50,78,128)`, `syntax_theme="OneHalfDark"`.

#### 1.4 Derived colors (helper methods on `Theme`)
- `is_dark()` — perceived luminance of `bg` (`0.299r + 0.587g + 0.114b < 128`). Drives light/dark adaptation everywhere.
- `diff_added()` / `diff_modified()` / `diff_deleted()` — return `Color32` with a dark/light branch each (e.g. added = `(80,180,100)` dark / `(30,130,55)` light). These are NOT theme fields — they're computed. warpui must port these as methods returning `ColorU`.

#### 1.5 Global state + persistence
- `static CURRENT: RwLock<Option<Theme>>` — parking_lot RwLock. `init()`/`set()` write; `current()` clones (cheap, falls back to `Theme::dark()` if uninitialized).
- `themes_dir()` = `~/.crane/themes`.
- `load_all()` — scans `*.toml` in themes_dir, parses each via `toml::from_str`, then appends any builtin NOT shadowed by a same-named on-disk file; sorted by name.
- `find_by_name(name)` — `load_all().find(name == ...)`.
- `ensure_builtin_tomls_on_disk()` — on first launch, writes every builtin to `~/.crane/themes/<name>.toml` with a 3-line `# Crane theme: <name>` header comment. Never overwrites existing files. Called once in `CraneApp::new`.

**warpui port for theme:** Port `Rgb`/`Theme` structs unchanged (they're pure data + serde). The 21 constructors copy verbatim. Replace all `current()` reads in render code with the same global RwLock pattern. Consumers call `.to_warp()` instead of `.to_color32()`. `diff_*()` helpers return `ColorU`. The TOML load/save and builtin-on-disk dump are pure file I/O — port unchanged. **Port status: ~80%** — `to_warp` bridge exists, structs are serde-ready and warp-agnostic; remaining work is wiring `current()` reads into warpui scene render and confirming `ColorU` alpha handling for the accent-@70 selection fill.

---

### 2. App shell entry + main render loop (`main.rs`)

#### 2.1 `fn main()`
- `env_logger::init()`.
- `startup::fix_path_for_gui_launch()` (rehydrate PATH).
- Linux only: `gtk::init()` before any webview is built (non-fatal on failure).
- Builds `egui::ViewportBuilder`: **inner size `[1480.0, 920.0]`, min inner `[800.0, 500.0]`, title `"Crane"`**, icon from `crane.png`. `persist_window: true` (eframe restores window geometry across launches via its RON storage). warpui must replicate the same default + min window dimensions and window-geometry persistence.
- `eframe::run_native("Crane", …)`. In the creation callback: `platform_menu::install()` then (macOS) `mac_keys::install_cmd_v_monitor()`, then `CraneApp::new(cc)`.

#### 2.2 `struct CraneApp` (the eframe::App)
```rust
struct CraneApp {
    app: App,                                  // all real state
    last_saved_snapshot: String,               // session.json dedup
    last_saved_settings_snapshot: String,      // settings.json dedup
    last_save_at: Instant,
    pending_close: Option<PaneId>,             // terminal-running confirm modal target
    browser_host: BrowserHost,                 // embedded webviews
}
```

#### 2.3 `CraneApp::new(cc)` — boot sequence (order matters):
1. `request_repaint_after(1500ms)` — heartbeat so timers/async results surface even when idle.
2. `startup::migrate_config_dir()` — one-shot `~/.config/crane` → `~/.crane` rename.
3. Load session: `state::session::load()` → `.restore(ctx)` or `App::new()`.
4. `Settings::load().apply(&mut app)` — user prefs override stale session keys.
5. `theme::ensure_builtin_tomls_on_disk()`.
6. Resolve initial theme by `app.selected_theme` name, fall back to dark → `theme::init()`.
7. `startup::load_fonts(ctx, app.custom_mono_font)`.
8. `ctx.set_zoom_factor(app.ui_scale)`.
9. `startup::apply_style(ctx)`.
10. `app.update_check.spawn_check(ctx)`.

#### 2.4 `eframe::App::ui()` — the per-frame pipeline (≈ 780 lines; this is the master render order warpui must reproduce frame-for-frame)

The frame runs this fixed sequence:
1. (macOS) `mac_keys::set_terminal_focused(false)` — reset NSEvent Tab-swallow gate; terminal view re-sets `true` later if focused.
2. `platform_menu::drain_events()` → match menu IDs → set `show_settings` / `show_help` / kick update check / open file/folder dialog / `pending_quit_modal`.
3. `app.ensure_initial(ctx)`, `app.sync_tab_mru()`, `app.poll_loose_git_init(ctx)` (throttled), `app.poll_dead_worktrees(ctx)` (3 s throttle), `terminal::gpu_render::ensure_initialized(frame)`.
4. **Close-request guard:** if `viewport().close_requested() && !confirmed_quit` → `CancelClose` + `pending_quit_modal = true`. Then render confirm-quit modal if pending.
5. **Tab switcher** (`handle_tab_switcher_keys`): drains the macOS Cmd+\` cycle delta (`mac_keys::drain_pending_tab_cycle()`, signed i32), advances/opens the overlay. If consumed, **skip** the generic shortcut handler this frame.
6. Else `shortcuts::handle(ctx, app, &mut pending_close)`.
7. **Dead-tab reaping:** for each Terminal Pane, `tabs.retain(is_alive)`; empty pane → `close_focused()`; clamp `active`.
8. `app.refresh_active_git_status(...)`, `app.lsp.shutdown_disabled(...)`.
9. **Browser host pump** (gated on `!is_idle()`): drain URL updates, set loading + memory snapshots, request 100 ms repaint while loading. (Linux: also drain GTK event loop.)

10. **Region geometry math** (warpui must replicate exactly):
   - `full = ui.available_rect_before_wrap()`.
   - `bg/sidebar_bg/divider = t.*.to_color32()`; paint `full` with `bg`.
   - `left_w = show_left ? left_panel_w : 0`, `right_w = show_right ? right_panel_w : 0`.
   - **Status bar strip:** `status_bar_rect` = bottom `ui::status::HEIGHT` px; `content_bottom = status_bar_rect.min.y`.
   - `left_rect` = `full.min` → `(min.x+left_w, content_bottom)`.
   - `right_rect` = `(max.x-right_w, min.y)` → `(max.x, content_bottom)`.
   - `center_rect` = `(min.x+left_w, min.y)` → `(max.x-right_w, content_bottom)`.

11. **Left Panel** (if `show_left`): fill `sidebar_bg`, 1px `divider` line on right edge, child UI clipped to `left_rect`, `ui::projects::render`. **Resize handle:** 6px-wide strip straddling the right edge (`±3px`), `Sense::click_and_drag()`, `ResizeHorizontal` cursor on hover/drag, drag sets `left_panel_w = (pos.x - full.min.x).clamp(180.0, full.width()*0.45)`.

12. **Right Panel** (if `show_right`): symmetric — fill, 1px divider on left edge, `ui::explorer::render`, handle on left edge, drag sets `right_panel_w = (full.max.x - pos.x).clamp(200.0, full.width()*0.5)`.

13. **Center / Main Panel:** child clipped to `center_rect`; `ui::top::render` (top bar). `canvas_rect = center_rect` minus `ui::top::TOTAL_H` from the top. `full_inset = canvas_rect.shrink(6.0)`.

14. **Bottom-docked Git Log carve-out** (constants: `GIT_LOG_SPLITTER_H=4.0`, `GIT_LOG_MIN_H=120.0`): when active Tab's `git_log_visible`, height = `git_log_state.height` (default 320) clamped to `[120, full_inset.height()*0.7]`. Carves `inset` (top), `git_log_splitter_rect` (4px), `git_log_body_rect` (bottom) out of `full_inset`. The Git Log Pane sits **outside** the Layout tree — non-draggable, non-dockable.

15. **Diagnostics snapshot** into `diag_map` (path → `Vec<Diagnostic>`) to avoid borrow conflicts. Closures built for render: `diag_fn`, `notify_saved` (pushes to a `RefCell<Vec<(path,text)>>` save_queue), `format_before_save` (consults `language_configs`, calls `format::format_text`), `goto_request` (pushes to `RefCell` goto_queue). Plus `syntax_override`, `workspace_root`, `EditorPrefs { word_wrap, trim_on_save }`.

16. **Modal-open gate:** `modal_open = show_settings || show_help || new_workspace_modal || find_in_files || !missing_project_modals.is_empty() || pending_close.is_some()`. If open → `center_ui.disable()` (clicks/keys don't leak to panes).

17. **Render the active Layout** via `ui::pane_view::render_layout(...)` returning a `PaneAction`, matched into mutations: `Focus`, `Close` (terminal-running → `pending_close`), `ResizeSplit{path,ratio}`, `SwapPanes{a,b}`, `DockPane{src,target,edge}`, `ToggleMaximize`, `ReplaceWithTerminal` (spawns PTY at layout cwd), `ReplaceWithBrowser`, `ShowFilesPanel`, `OpenFile`, `OpenFileExternal`. If no layout → `render_empty_state`.

18. **Git Log Pane render** (after Layout so splitter is on top): splitter drag adjusts `state.height` (`ResizeVertical` cursor); body via `git_log::view::render` returns a `ViewEffect` (`close`, `open_diff`, `branch_from`, `op`). Ops: Checkout/CherryPick/Revert/CopyHash/BranchFrom/WorktreeFrom — each shells to git or parks UI state; most drop `state.frame = None` to force reload.

19. **Modal cascade** (order = z-order, last on top): `render_missing_project_modal`, `render_new_workspace_modal`, `render_help_modal`, `find_in_files::render`, `render_settings_modal` (returns `SettingsEffect::ReloadFonts` → re-`load_fonts`), `render_confirm_close`, `render_confirm_remove_worktree`, `render_confirm_close_tab`, `render_confirm_delete_file`, `tab_switcher::render`, `render_lsp_install_prompt`, `render_lsp_download_toast`, `update_check.drain()` + `render_update_toast`.

20. **Notification drain:** `window_focused = ctx.input(focused)`, `drain_terminal_notifications()` (OSC 9/777; skip native banner if window focused), `poll_cli_agent_sessions()` (1 Hz), `render_notification_toast`.

21. **Deferred queues:** drain `save_queue` → `lsp.did_save` + `refresh_diff_panes_for_path`; `refresh_diff_panes_after_hunk_stage`; drain `goto_queue` → `lsp.goto_dispatch` → push `PendingGoto{server,request_id,dispatched_at}`; drain ready gotos (5 s watchdog) → `goto_location`.

22. **Status bar** render in `status_bar_rect`; `branch_picker::render`; `sync_lsp_changes(ctx)`.

23. **Browser host sync:** compute `overlay_visible` (any modal OR a Tooltip/Foreground egui layer that actually painted this frame — checked via `ctx.memory(areas.top_layer_id + visible_last_frame)`), then `browser_host.sync(frame, ctx, bridge, overlay_visible, all_keys)`.

24. `maybe_save()`.

#### 2.5 `render_confirm_close(ctx)` — terminal-running confirm modal
`egui::Window "Terminal is still running"`, non-collapsible/non-resizable, centered (`Align2::CENTER_CENTER`), `min_width 340`. Body label: "A process is running in this terminal. Closing it will kill the process." Row of two buttons: **"Cancel"** and **"Close terminal"**. Escape = cancel. Confirm → set `ws.focus = id` + `close_focused()`.

#### 2.6 `maybe_save()` — debounced atomic session persistence
- Returns early if `< SAVE_DEBOUNCE` since last save.
- Serialize `Session::from_app` to pretty JSON on the render thread (fast), diff against `last_saved_snapshot`; if unchanged, just bump timestamp.
- Hand bytes to a `std::thread::spawn`: ensure parent dir, copy old `session.json` → `.json.bak`, **atomic write** (`.json.tmp` → write → `sync_all` → `replace_file` rename → fsync parent dir).
- Settings saved separately to `~/.crane/settings.json` (own dedup snapshot, own thread).

#### 2.7 `goto_location` / `on_exit`
- `goto_location` — opens file into active Layout's Files pane (or reuses an open File Tab), sets `pending_cursor = (line,char)`.
- `on_exit` — `jobs.shutdown()` (joins workers; the global `OnceLock<Arc<JobSystem>>` would otherwise leak and never Drop) + drop `file_watcher`.

**warpui port for main.rs:** This becomes the warpui presenter/scene root. The region math (left/right/center/status/git-log rects) ports 1:1 — same clamps, same 6px insets, same handle widths. **Clickable elements** (panel resize handles, splitters) port as **hit-Rect-on-top + dispatch_typed_action**: register the handle rect, on drag emit a typed `ResizePanel{which, new_w}` / `ResizeGitLog{dy}` action. The **`PaneAction` enum** maps directly to warpui typed actions dispatched up from the Layout scene. Modals become warpui overlay layers rendered in the same z-order. `ctx.notify` replaces the per-frame `request_repaint` calls. **Port status: ~35%** — geometry/region math is straightforward and largely mechanical, but the full modal cascade, browser-host overlay gating, and the deferred save/goto/diag queues are deeply egui-coupled (borrow-juggling via `RefCell`, `ctx.memory` layer probing) and need rearchitecting around warpui's scene/action model.

---

### 3. Startup infra (`startup.rs`)

- **`fix_path_for_gui_launch()`** (unix only): heuristic GUI-launch detection (PATH lacks `/usr/local/bin`/`/opt/homebrew/bin`/`.cargo/bin` but HOME set) → runs `$SHELL -l -c "echo __CRANE_PATH__:$PATH"` to import login-shell PATH. Always sprinkles version-manager dirs (`.cargo/bin`, `.local/bin`, `~/bin`, `go/bin`, `.volta/bin`, `.fnm/...`, `.asdf/shims`, `.bun/bin`, `n/bin`, homebrew, plus nvm node versions sorted by mtime). De-dups preserving order; `set_var("PATH", ...)` before any threads spawn.
- **`migrate_config_dir()`** — `~/.config/crane` → `~/.crane` one-shot rename.
- **`load_app_icon()`** — `include_bytes!("../crane.png")` → RGBA8 → `egui::IconData`.
- **`load_fonts(ctx, custom_mono)`** — builds `FontDefinitions`: adds `egui_phosphor` Regular variant (**this is the icon font the entire UI depends on — warpui must load the same Phosphor glyph set**), inserts `jetbrains_mono` (bundled `assets/JetBrainsMono-Regular.ttf`, primary Monospace at index 0) + `cascadia_mono` (`assets/CascadiaMono-Regular.ttf`, Monospace fallback index 1, covers Braille/block/box-drawing JBM lacks). Cascadia also appended to Proportional. Optional user `custom_mono` font inserted at Monospace index 0. Then `add_system_fallback_fonts` (per-OS CJK/Arabic/Hebrew/Devanagari system font paths appended to both families). `ctx.set_fonts`.
- **`apply_style(ctx)`** — translates the active `Theme` into `egui::Style`: light/dark `Visuals` base by `bg` brightness; widget states map `inactive→surface`, `hovered→surface_alt`, `active→surface_hi` with `border`/`border_strong` strokes and `text`/`text_hover` fg; `CornerRadius::same(6)` on widgets, `10` on windows, `8` on menus; `selection.bg_fill = accent @ alpha 70`, `selection.stroke = accent`; `panel_fill/extreme_bg=bg`, `code_bg=surface`, `faint_bg=row_hover`, `override_text_color=text`. Spacing: `button_padding=(10,5)`, `item_spacing=(8,5)`, `menu_margin=6`. Debug paint flags zeroed in debug builds.

**warpui port:** Font loading → warpui's font registry (load Phosphor + JetBrains Mono + Cascadia + system fallbacks identically). `apply_style` → a warpui theme-to-style adapter producing the same corner radii (6/8/10), paddings, and the accent-@70 selection. PATH/icon/migrate are pure infra (port unchanged). **Port status: ~40%** — `to_warp` exists, but the egui-`Style` translation has no warpui analogue yet and the icon-font wiring needs warpui's glyph atlas.

---

### 4. Global shortcuts (`shortcuts.rs`)

`handle(ctx, app, pending_close)` — runs once/frame before panels. Canonical bindings (all `Cmd`-prefixed; consumed where noted to avoid leaking to terminal):
- **Cmd+Shift+F** → Find in Files (consumed; bypasses modal-guard, re-triggerable).
- **Modal-open guard:** when any modal open, Cmd+W / Esc are *consumed* and dismiss the topmost modal (settings/help/new-workspace/remove-worktree/close-tab/delete-file/quit/missing-project/find/pending-close). Everything else falls through (Cmd+S still works inside modals).
- **Cmd+T** split terminal (Horizontal), **Cmd+Shift+T** new Tab, **Cmd+D** split H, **Cmd+Shift+D** split V, **Cmd+W** close pane (Files pane w/ >1 tab → close active File Tab instead; terminal-running → `pending_close`), **Cmd+Shift+W** close Tab, **Cmd+]** next pane, **Cmd+[** prev pane, **Cmd+= / Cmd++** zoom in (font_size +1, max 40), **Cmd+-** zoom out (min 8), **Cmd+0** reset (14.0), **Cmd+B** toggle Left, **Cmd+/** toggle Right (both gated on `!any_focus`), **Cmd+Option+T** new Browser tab (focused Browser pane only).
- **Cmd+Backspace / Delete** (no focus) → confirm-delete selected file. **Cmd+Z** (no focus) → undo last Files-Pane move/trash. **Cmd+O** open file (native dialog), **Cmd+Shift+O** open folder as project. **Cmd+9** toggle Git Log pane. **Cmd+F** (when Git Log focused) → focus its filter TextEdit; else falls to Files-pane find.
- `terminal_is_running(app, id)` helper gates close-confirm.

**warpui port:** A `keymap.rs`-style binding table (warpui already has `keymap.rs` / `keymap_tests.rs`) emitting typed actions; the focus-guard (`ctx.memory().focused().is_some()`) maps to warpui's focus tracking. **Port status: ~25%** — warpui has a keymap framework but none of these specific Crane bindings/guards are wired.

---

### 5. macOS NSEvent monitor (`mac_keys.rs`) — platform infra, NOT visual

A local `NSEvent` monitor (`KeyDown | FlagsChanged`) intercepting chords winit/egui can't see:
- **Cmd+V image paste** — reads NSPasteboard (PNG, else TIFF→PNG via `NSBitmapImageRep`), writes `~/.crane/paste-images/<uuid>.png`, queues path (`drain_pending_image_paths`).
- **Cmd+C/V/X/A in Browser pane** — forwards AppKit selectors (`copy:`/`paste:`/`cut:`/`selectAll:`) to the stored `FOCUSED_WEBVIEW` (AtomicPtr, +1 retained), swallows.
- **Shift+Tab / Tab** — swallowed → queued as CSI Z / `\t` only when `TERMINAL_FOCUSED` (gated atomic, reset each frame in `ui()`).
- **Cmd+\` / Cmd+~** — macOS steals these natively; intercepted, queued as signed `PENDING_TAB_CYCLE` (forward +1 / backward -1), drained by tab switcher.
- `CMD_HELD` atomic tracked off `flagsChanged` for reliable tab-switcher commit-on-release.
- `install_cmd_v_monitor()` idempotent; monitor leaked for process lifetime.

**warpui port:** This is platform glue, not UI. If warpui keeps the winit/AppKit host, port the monitor 1:1 (atomics + queues are host-agnostic). **Port status: ~0%** but **low priority** — it's invisible infra; only matters once warpui owns the event loop.

---

### 6. Native macOS menu (`platform_menu.rs`)

`muda`-based NSApp menu, installed once (leaked). Submenus + items:
- **Crane:** About Crane (with version), "Check for Updates…" (`ID_CHECK_UPDATES`), —, "Settings…" `Cmd+,` (`ID_SETTINGS`), —, Services, —, Hide/HideOthers/ShowAll, —, "Quit Crane" `Cmd+Q` (`ID_QUIT`, custom so the in-app confirm modal can intercept).
- **File:** "Open File…" `Cmd+O` (`ID_OPEN_FILE`), "Open Folder as Project…" `Cmd+Shift+O` (`ID_OPEN_FOLDER`). **No Edit submenu** (deliberate — would break egui copy/paste; see file comment).
- **Window:** Minimize/Maximize/—/Fullscreen.
- **Help:** "Keyboard Shortcuts" (`ID_SHORTCUTS`).
- `drain_events()` → `Vec<String>` of fired IDs (matched in `ui()`). Non-macOS: all stubs returning empty.

**warpui port:** Pure host glue — port verbatim if AppKit host retained; IDs feed the same `App` flag toggles. **Port status: ~0%**, low priority.

---

### 7. Job system (`jobs/system.rs`) — threaded compute infra

`JobSystem`: 2 thread pools — **Cpu** (`available_parallelism().clamp(1,4)`, for highlight/diff/parse) + **Io** (fixed 2, for git shell-out / file reads). Public types: `Scope` (Global/Project/Workspace/Tab/Pane(u64)), `Priority` (Idle/Background/Visible/Foreground, ordered), `Pool` (Cpu/Io), `JobKey{scope, kind:&'static str}`, `CancelToken` (Arc<AtomicBool>, cooperative), `JobOutput<T>` (Done/Cancelled), `JobHandle<T>` (`try_recv()` non-blocking, `is_disconnected()`, `cancel_token()`). `submit()` dedups by key (new supersedes old via cancel), priority-ordered `BinaryHeap` per pool (ties FIFO by seq). `cancel`/`cancel_scope`, `shutdown` (flips all tokens, joins). Workers `catch_unwind` so one panic doesn't kill the pool. `Drop` mirrors shutdown.

**warpui port:** **Entirely backend** — host/UI-agnostic. Port unchanged; warpui scene just polls `JobHandle::try_recv()` and `ctx.notify`s on completion (replacing the `repaint` callback). **Port status: ~0% needed / 100% reusable** — copy as-is, swap the repaint closure for `ctx.notify`.

---

### 8. File watcher (`file_watcher.rs`) — backend infra

`notify::RecommendedWatcher` + one debouncer thread (50 ms quiet window). `ChangeEvent{project, paths, created, modified, removed, arrived_at}`. `watch_project`/`unwatch_project` (canonicalizes paths for macOS `/private/var` realpath routing), `take_receiver`. `is_filtered` drops `.git/objects|logs|index.lock|HEAD.lock`, `.DS_Store`, `Thumbs.db`, `*.swp/.swx/~/~$*/.tmp`. `route_to_project` = longest-prefix match. Drop tears down watcher + joins debouncer. **2 threads total regardless of project count.**

**warpui port:** Pure backend, port unchanged; App drains the `Receiver` each frame and `ctx.notify`s. **Port status: ~0% needed / 100% reusable.**

---

### 9. Dir cache (`dir_cache.rs`) — backend infra

`DirCache` (global `OnceLock<Arc<DirCache>>`): caches sorted/filtered `read_dir` listings keyed by `(path, mtime)` (POSIX dir-mtime bumps on add/remove → self-invalidating). `DirEntryCached{name, path, is_dir}`. Sort = **dirs-first then alpha** (`(!is_dir, name)`). Bounded at `MAX_ENTRIES=512` (arbitrary eviction, not strict LRU). `invalidate(path)` / `clear()`. Returns `Arc<Vec<…>>` (cheap clone on hit). No JobSystem (read_dir is sub-frame).

**warpui port:** Backend, port unchanged; the Files Pane (Left/Right tree) reads from it. **Port status: ~0% needed / 100% reusable.** The dirs-first-then-alpha sort is the contract the tree render must keep.

---

### 10. Format discovery + formatter shell-out (`format/mod.rs`)

`FormatStyle{tab_width, use_tabs, source}` (default tab_width=2, spaces). `discover(file)` walks ancestors for `.prettierrc[.json/.yaml/.yml]` or `package.json#prettier` (JSON parsed; YAML recognized→defaults). `indent_unit()` = tab or N spaces. `format_text(key, path, content)` shells out per LSP key: TypeScript/CssLs/HtmlLs → `prettier --stdin-filepath`, RustAnalyzer → `rustfmt --emit stdout`, Pyright → `ruff format -`, Gopls → `gofmt`, Eslint → None (skipped). Pipes via stdin/stdout, returns None on missing binary/non-zero. Editor helpers: `char_idx_to_byte`, `auto_indent_context` (copies prev-line indent, +1 level after `{([` or `=>`).

**warpui port:** Backend logic consumed by the Files Pane editor (format-on-save, Tab indent). Port unchanged. **Port status: ~0% needed / 100% reusable.**

---

### 11. Auto-update (`update/apply.rs` + `update/check.rs`) — toast UI + backend

**`check.rs`:** `UpdateCheck{available, prompts:HashMap<version,PromptState>, dismissed_this_session, manual_check, manual_result_seen, rx}`. `PromptState` = Dismissed | RemindAt(u64). `spawn_check` → background `fetch_latest` (GitHub `releases/latest`, skips draft/prerelease, semver `is_newer`). `should_show`, `dismiss_session`/`dismiss_forever`/`remind_later` (`REMIND_AFTER_SECS = 7 days`). `AvailableUpdate{version, url}`.

**`apply.rs`:** `Updater{state:Arc<Mutex<UpdateState>>}`. `UpdateState` = Idle | Downloading{bytes} | Installing | Ready{staged_bundle} | Failed(String). macOS: download universal DMG → `hdiutil attach` → copy `Crane.app` to staging → detach → `codesign --verify --deep --strict` → strip quarantine; "Restart now" writes `/tmp/crane-swap-<pid>.sh` (waits PID, swaps bundle, `open`). Linux: tar.gz → extract → find binary → swap script (`setsid`/`nohup`); `LinuxInstallKind` (Snap/Flatpak/SystemPackage/SelfManaged) gates whether in-app update is allowed (`unsupported_reason()` returns package-manager guidance). `start(urls, ctx)` tries each URL until 200; `apply_and_exit()` spawns swap + `exit(0)`.

**Rendered UI (the update toast — `render_update_toast` in modals, driven by these states):** The toast shows version, a download/restart CTA, and remind/dismiss buttons; `UpdateState` drives label (`Downloading {bytes}` → progress, `Installing`, `Ready` → "Restart now", `Failed` → reason text). Colors: `accent` for CTA, `error` for Failed.

**warpui port:** Both files are backend/state (port unchanged — `Arc<Mutex<UpdateState>>` + background threads, `ctx.notify` instead of `request_repaint`). The **toast UI** itself lives in `modals/` (a separate area) and is a clickable overlay → port as hit-Rect overlay + dispatch_typed_action (Restart/Remind/Dismiss → typed actions). **Port status: backend ~0% needed / 100% reusable; toast UI ~10%** (overlay not yet built in warpui).

---

### Overall area port status

| Sub-area | Reusable as-is | Port status |
|---|---|---|
| `theme.rs` (structs, 21 palettes, `to_warp`) | mostly | **~80%** |
| `main.rs` shell/region-math/render-pipeline | math reusable; render egui-coupled | **~35%** |
| `startup.rs` (PATH/icon/migrate) + fonts/style | infra reusable; style adapter missing | **~40%** |
| `shortcuts.rs` | needs warpui keymap wiring | **~25%** |
| `mac_keys.rs` / `platform_menu.rs` | host glue, 1:1 if AppKit host kept | **~0%** (low priority) |
| `jobs/system.rs` | fully host-agnostic | **100% reusable** |
| `file_watcher.rs` | fully host-agnostic | **100% reusable** |
| `dir_cache.rs` | fully host-agnostic | **100% reusable** |
| `format/mod.rs` | fully host-agnostic | **100% reusable** |
| `update/*` backend | host-agnostic (swap repaint→notify) | **100% reusable**; toast UI **~10%** |

**Honest weighted area total: ~45%.** The backend infra (jobs, watcher, dir-cache, format, update-engine, theme data) is ready or trivially portable; the genuinely unported work is the **egui-specific render pipeline in `main.rs`** (modal cascade z-order, browser-overlay gating via `ctx.memory` layer probing, the `RefCell` deferred save/goto/diag queues), the **`apply_style` → warpui style adapter**, and **wiring `shortcuts.rs`/`mac_keys` into warpui's keymap + event host.**

Relevant files (all absolute):
`/Users/rajpootathar/ideaProjects/crane/src/main.rs`, `/Users/rajpootathar/ideaProjects/crane/src/theme.rs`, `/Users/rajpootathar/ideaProjects/crane/src/startup.rs`, `/Users/rajpootathar/ideaProjects/crane/src/shortcuts.rs`, `/Users/rajpootathar/ideaProjects/crane/src/mac_keys.rs`, `/Users/rajpootathar/ideaProjects/crane/src/platform_menu.rs`, `/Users/rajpootathar/ideaProjects/crane/src/jobs/system.rs`, `/Users/rajpootathar/ideaProjects/crane/src/file_watcher.rs`, `/Users/rajpootathar/ideaProjects/crane/src/dir_cache.rs`, `/Users/rajpootathar/ideaProjects/crane/src/format/mod.rs`, `/Users/rajpootathar/ideaProjects/crane/src/update/apply.rs`, `/Users/rajpootathar/ideaProjects/crane/src/update/check.rs`.


---

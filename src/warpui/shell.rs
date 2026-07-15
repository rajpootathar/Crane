//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::warpui::file_pane::FileView;
use crate::warpui::file_tree;
use crate::warpui::icons;
use crate::warpui::layout::{Dir, Node, PaneId};
use crate::warpui::rect_probe::{pane_under, DockEdge, PaneRect, RectProbe};
use crate::warpui::split::SplitBox;
use warpui::color::ColorU;
use warpui::elements::{
    Align, Border, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, Dismiss, DispatchEventResult, Draggable,
    DraggableState, DropShadow, Empty, EventHandler, Expanded, Fill, Flex, Hoverable,
    MouseStateHandle,
    ParentElement, Radius, Rect, ScrollbarWidth, Stack, Text,
};
use warpui::platform::Cursor;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::vec2f;
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::warpui::theme;
use crate::warpui::view::TerminalView;
use crate::warpui::welcome_view::{WarpWelcomeView, WelcomeAction, WelcomeCallback};

/// A full-screen blocking modal. Rendered as the topmost child of the root
/// stack: a dim backdrop that absorbs clicks + a centered content card. Escape
/// or a Cancel/close button dismisses it (see `modal_overlay`). Port of old egui
/// `src/modals/*` (confirm_quit, confirm_close_tab, help, settings).
enum Modal {
    /// "A process is still running. Quit anyway?" — raised when the app is asked
    /// to quit while a terminal has a live foreground process.
    ConfirmQuit,
    /// Cmd+W over a terminal pane with a live foreground process — confirm before
    /// tearing the pane (and its PTY) down.
    ConfirmClosePane(PaneId),
    /// Settings: Appearance (theme picker + zoom) + About (version).
    Settings,
    /// Keyboard shortcuts cheat-sheet.
    Help,
    /// Cmd+Shift+F project-wide text search. Query + result state lives in
    /// `CraneShellView::find_in_files`; this variant only marks the modal open
    /// so the backdrop/scaffold/key-gating reuse the shared framework.
    FindInFiles,
    /// Cmd+` tab switcher. Entry list + highlight lives in
    /// `CraneShellView::tab_switcher`; this variant only marks it open.
    TabSwitcher,
    /// "Switch Branch" — a searchable local+remote branch list. Query + list
    /// live in `CraneShellView::switch_branch`; this variant marks it open so the
    /// modal scaffold + key-gating reuse the shared framework.
    SwitchBranch,
    /// "New Workspace" — create a git worktree for a branch. State lives in
    /// `CraneShellView::new_workspace`.
    NewWorkspace,
    /// "Remove Worktree" confirm — raised from the worktree-row menu before
    /// `git worktree remove`. Carries the `(project, worktree)` indices; the
    /// human label + path + dirty/unpushed WARNING for the card come from
    /// `remove_wt_info`, computed once when the modal opens (never per frame).
    ConfirmRemoveWorktree { pi: usize, wi: usize },
    /// "Close Tab" confirm — raised before tearing down a tab's layout + PTYs
    /// when it holds a running terminal or an editor with unsaved edits.
    /// Carries the `(project, worktree, tab)` key.
    ConfirmCloseTab { key: (usize, usize, usize) },
    /// "Close File Tab" confirm — raised when a file chip's × targets an editor
    /// buffer with unsaved edits (the top-level Tab confirm doesn't cover the
    /// per-file close path). Carries the File Tab index.
    ConfirmCloseFileTab { index: usize },
}

/// Visual style of a modal button (`modal_button`).
#[derive(Clone, Copy)]
enum ModalBtn {
    /// Filled error red — a destructive confirm (Quit, Close).
    Danger,
    /// Plain surface pill — Cancel / secondary.
    Plain,
    /// Filled accent — the primary / affirmative action (Create, Confirm).
    Primary,
}

/// Precomputed detail for the `ConfirmRemoveWorktree` modal. Filled once when
/// the modal opens (a couple of quick git shell-outs) so the card render stays
/// pure — no per-frame `git` calls.
struct RemoveWtInfo {
    /// Human label for the worktree (its display name / branch).
    label: String,
    /// Filesystem path of the worktree checkout.
    path: String,
    /// True when the worktree has uncommitted changes (incl. untracked).
    dirty: bool,
    /// Commits ahead of upstream that would be discarded if the branch is only
    /// checked out here — `> 0` warns "unpushed commits".
    ahead: usize,
}

/// State for an open project right-click context menu.
struct ProjectContextMenu {
    project_idx: usize,
    /// Window-relative position of the right-click that opened the menu.
    x: f32,
    y: f32,
}

/// A right-click context menu over a row in the Right Panel — either a Changes
/// row or a Files row. Anchored at the click position; rendered as a
/// width-constrained popover with the `Dismiss` overlay pattern.
enum RowMenu {
    /// Changes-tab file row: Stage / Unstage / Open as File / Copy Path / Open Diff.
    /// `staged` = index side set, `has_unstaged` = worktree side set; an `MM`
    /// file is both, so the menu offers Stage AND Unstage.
    Change { path: String, staged: bool, has_unstaged: bool, x: f32, y: f32 },
    /// Files-tab row: Open / Reveal / Copy Path / New File / New Folder / Delete.
    File { path: PathBuf, is_dir: bool, x: f32, y: f32 },
}

/// Visual tier of a Left-Panel row. `Selected` = the single active leaf (the
/// selected tab, or its deepest visible ancestor when the leaf is collapsed
/// out of view); `Ancestor` = the project/workspace chain that contains the
/// selection (context, not selection); `Plain` = everything else.
#[derive(Clone, Copy, PartialEq)]
enum RowTier {
    Plain,
    Ancestor,
    Selected,
}

/// Inline "new file / new folder" editor pending in the Files tree. Text is
/// entered via the same keystroke route as the commit box (`SendKeys` →
/// `edit_new_entry`). Ported from old egui `PendingNewEntry`.
struct PendingNewEntry {
    parent: PathBuf,
    is_folder: bool,
    name: String,
    error: Option<String>,
}

/// A single node of the directory-grouped Changes tree (port of old egui
/// `DirNode` in explorer.rs).
#[derive(Default)]
struct ChangeDir {
    dirs: BTreeMap<String, ChangeDir>,
    files: Vec<ChangeFile>,
}

struct ChangeFile {
    name: String,
    path: String,
    /// True when the index column (X) is set — the file has content staged.
    staged: bool,
    /// True when the worktree column (Y) is set — the file has content NOT
    /// yet staged. Independent of `staged`: an `MM` file is BOTH `staged`
    /// (index modified) AND `has_unstaged` (worktree modified). Drives the
    /// row's Stage-vs-Unstage action and the context menu's dual entries.
    has_unstaged: bool,
    status: char,
}

impl ChangeDir {
    /// Collect every descendant file's path (folder-level stage/unstage).
    fn collect_paths(&self, out: &mut Vec<String>) {
        for f in &self.files {
            out.push(f.path.clone());
        }
        for child in self.dirs.values() {
            child.collect_paths(out);
        }
    }

    /// `(all_staged, any_staged)` across the subtree. "Fully staged" means
    /// every file has its worktree side clean (no `has_unstaged`) — so a
    /// folder holding an `MM` file reads as NOT fully staged, and its bulk
    /// marker offers Stage (add the remaining worktree changes) rather than
    /// Unstage.
    fn staged_state(&self) -> (bool, bool) {
        let mut total = 0usize;
        let mut fully_staged = 0usize;
        let mut any = false;
        fn walk(n: &ChangeDir, total: &mut usize, fully_staged: &mut usize, any: &mut bool) {
            for c in n.dirs.values() {
                walk(c, total, fully_staged, any);
            }
            for f in &n.files {
                *total += 1;
                if f.staged {
                    *any = true;
                    if !f.has_unstaged {
                        *fully_staged += 1;
                    }
                }
            }
        }
        walk(self, &mut total, &mut fully_staged, &mut any);
        (total > 0 && fully_staged == total, any)
    }
}

/// How long a live OSC-0/2 terminal title must hold steady before it is adopted
/// as a Tab row's label. Agent CLIs (Claude Code &c.) rewrite the title several
/// times a second with live spinner / token-count text; without this window the
/// tree row name churns constantly. The FIRST title on a fresh terminal adopts
/// immediately (see `TitleDebounce::observe`) so a new shell's initial title
/// isn't held back — churn protection only kicks in once a title is displayed.
const TITLE_STABLE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// Per-tab debounce state for the live terminal title. Keyed by tab key in
/// `CraneShellView::title_debounce`. Pure state machine — see `observe`, which
/// is unit-tested (`title_debounce_*`).
#[derive(Default, Clone)]
struct TitleDebounce {
    /// The title currently shown in the row. `None` until the first is adopted.
    displayed: Option<String>,
    /// The most recent live title seen (the candidate awaiting promotion).
    candidate: Option<String>,
    /// When `candidate` was first observed; drives the stability window.
    candidate_since: Option<std::time::Instant>,
}

impl TitleDebounce {
    /// Feed the current live title; returns the stabilized title to display.
    ///
    /// - Already displaying `live` → no-op.
    /// - Nothing displayed yet (fresh terminal still on its "Terminal N"
    ///   default) → adopt immediately, no wait.
    /// - Otherwise track how long `live` has held steady; promote it only once
    ///   it has been unchanged for `window`. A changing `live` keeps resetting
    ///   the candidate clock, so churning titles never get promoted.
    fn observe(&mut self, live: &str, now: std::time::Instant, window: std::time::Duration) -> String {
        // `shown` is what the row currently displays; if nothing has ever been
        // adopted (fresh shell still on its default label), adopt immediately.
        let Some(shown) = self.displayed.clone() else {
            self.displayed = Some(live.to_string());
            self.candidate = Some(live.to_string());
            self.candidate_since = Some(now);
            return live.to_string();
        };
        if shown == live {
            return shown;
        }
        if self.candidate.as_deref() != Some(live) {
            // New candidate — (re)start its stability clock.
            self.candidate = Some(live.to_string());
            self.candidate_since = Some(now);
        } else if let Some(since) = self.candidate_since {
            if now.duration_since(since) >= window {
                // Held steady for the full window — promote.
                self.displayed = Some(live.to_string());
                return live.to_string();
            }
        }
        // Not (yet) promoted — keep showing the committed title.
        shown
    }
}

pub struct CraneShellView {
    ui_font: FamilyId,
    icon_font: FamilyId,
    /// Monospace face (Menlo) for the Git Log lane graph + commit rows.
    mono_font: FamilyId,
    /// All panes by id. Persistent (terminals keep their PTY + scrollback).
    panes: HashMap<PaneId, PaneContent>,
    /// Per-tab split tree (the Layout). Each leaf references a pane id.
    layouts: HashMap<(usize, usize, usize), Node>,
    /// The focused pane — target for split / close / scroll.
    focused: Option<PaneId>,
    /// When set, only this pane renders (expand-to-full / maximize).
    maximized: Option<PaneId>,
    /// The dedicated File pane (files open as TABS inside it), if open.
    files_pane: Option<PaneId>,
    /// Open file paths in the File pane (shell-side mirror, drives the header
    /// tab strip + persistence).
    file_pane_paths: Vec<PathBuf>,
    /// Active file tab index in the File pane.
    file_pane_active: usize,
    /// Live warp editor per open file path — kept alive across tab switches so
    /// each tab preserves its own cursor / scroll / unsaved edits. The Editor
    /// pane shows the one for `file_pane_paths[file_pane_active]`.
    editor_views: HashMap<PathBuf, ViewHandle<crate::warpui::editor_view::WarpEditorView>>,
    /// Live markdown views per open `.md` path — kept alive across tab switches
    /// so each rendered doc preserves its own scroll offset (peer of
    /// `editor_views`; a Markdown pane shows the one for the active file tab).
    markdown_views: HashMap<PathBuf, ViewHandle<crate::warpui::markdown_view::WarpMarkdownView>>,
    /// Cached terminal snapshots (cwd + ANSI scrollback) for persistence.
    /// Refreshed on every action but time-debounced so per-keystroke cost stays
    /// low while still capturing recent command output.
    term_cache: RefCell<HashMap<PaneId, crate::warpui::persist::STerminal>>,
    /// Last time `term_cache` was refreshed (debounce clock).
    last_term_snapshot: std::cell::Cell<Option<std::time::Instant>>,
    /// Panes cleared via Cmd+K since the last snapshot — allows their persisted
    /// history to shrink (otherwise we never shrink, to keep a restored session
    /// sticky across save/restore generations instead of degrading).
    term_cleared: RefCell<HashSet<PaneId>>,
    /// Persistent drag state per pane (survives re-renders; Arc-shared).
    drag_states: HashMap<PaneId, DraggableState>,
    /// Last painted window rect per pane (recorded by RectProbe) — used to
    /// compute the dock zone under the cursor during a drag.
    pane_rects: Rc<RefCell<HashMap<PaneId, PaneRect>>>,
    /// Live drop preview during a drag: (target pane, dock edge).
    drop_preview: Rc<RefCell<Option<(PaneId, DockEdge)>>>,
    /// Monotonic pane id source.
    next_pane_id: PaneId,
    /// Which tab's layout is shown in the center.
    active_tab: Option<(usize, usize, usize)>,
    projects: Vec<crate::warpui::projects::ProjectNode>,
    /// Active worktree dir — drives the Files/Changes panel root.
    active_cwd: Option<PathBuf>,
    /// Cached current branch of the active worktree (status bar).
    branch: String,
    /// Git Log bottom dock (sits below the panes, outside the pane tree,
    /// height-resizable). Old Crane renders the git log as a dock, not a pane.
    show_git_log: bool,
    git_log_ratio: Rc<Cell<f32>>,
    git_log_drag: Rc<Cell<bool>>,
    /// Loaded lane-graph snapshot (commits + refs + lanes). `None` until the
    /// first off-thread [`git_log::load`] lands. Shared behind `Rc` so the
    /// custom list element can borrow it without a data clone.
    git_log_frame: Option<Rc<crate::warpui::git_log::GraphFrame>>,
    /// Repo the cached frame was loaded against — detects Workspace switches.
    git_log_repo: Option<PathBuf>,
    /// True while an off-thread graph load is in flight (loading placeholder).
    git_log_loading: bool,
    /// Generation guard so a stale spawn result (from an old repo / reload)
    /// can't clobber a newer frame.
    git_log_gen: u64,
    /// Debounce clock for ref-change auto-reloads.
    git_log_last_reload: std::time::Instant,
    /// Last time a filesystem ref event triggered a synchronous
    /// `poll_worktrees` from `drain_fs_events`. Rate-limits that path (>= 1s)
    /// so an event storm can never saturate the UI thread with per-project
    /// `git worktree list` shell-outs on the 250ms fast tick.
    fs_ref_poll_last: std::time::Instant,
    /// Debounce clock for background (non-active-repo) badge rescans —
    /// see `drain_fs_events`'s non-active block.
    bg_badge_last_scan: std::time::Instant,
    /// Non-active repo roots touched since the last background badge scan but
    /// not yet flushed because the debounce window hadn't elapsed. Carried
    /// across ticks so a root touched right before the window closes isn't
    /// dropped forever if no further edits land on it — the next tick (even
    /// with zero new fs events) flushes it once the debounce clears.
    pending_bg_roots: std::collections::HashSet<PathBuf>,
    /// Selected commit SHA (highlighted; drives the detail panel).
    git_log_selected: Option<String>,
    /// Fractional row scroll offset for the commit list (owned here so it
    /// survives the element's per-frame rebuild).
    git_log_scroll: Rc<Cell<f32>>,
    /// Hovered commit-list row index (shared with the element).
    git_log_hover: Rc<Cell<Option<usize>>>,
    /// Ref-scoped log filter (a branch/tag picked in the refs column);
    /// `None` = `--all` (the full graph).
    git_log_ref_filter: Option<String>,
    /// Case-insensitive text filter over subject / hash / author; "" = off.
    git_log_filter: String,
    /// True while the git-log filter field owns typing (`SendKeys` routes here).
    git_log_filter_active: bool,
    /// Filtered-frame cache keyed by (needle, generation) — `filtered_frame`
    /// re-runs lane layout over up to 10k commits, which must not happen per
    /// paint. RefCell: filled lazily from `render` (which is `&self`).
    git_log_filtered:
        std::cell::RefCell<Option<(String, u64, Rc<crate::warpui::git_log::GraphFrame>)>>,
    /// Open commit context menu: (sha, window x, window y).
    git_log_menu: Option<(String, f32, f32)>,
    /// Inline "create branch from commit" prompt: (sha, name buffer).
    git_log_branch_prompt: Option<(String, String)>,
    /// True while `git fetch --all --prune --tags` runs off-thread.
    git_log_fetching: bool,
    /// Last release re-check (in-session updates are re-polled every 6h).
    last_update_check: std::time::Instant,
    /// Per-version update-prompt decisions (old check.rs semantics): Skip =
    /// never show that version again; Remind = resurface after 7 days.
    /// Persisted via `WarpuiState::update_prompts`.
    update_prompts: HashMap<String, UpdatePrompt>,
    /// Version whose banner was closed (×/Later) this session — transient.
    update_dismissed_session: Option<String>,
    /// True from the moment the user explicitly triggers "Check for
    /// Updates…" (menu or Settings) until the result is acknowledged. Old
    /// Crane's `manual_check`/`manual_result_seen`: a routine background
    /// check that finds nothing stays silent, but a check the user asked
    /// for always gets a visible answer — including "you're up to date",
    /// shown in the same persistent banner rather than a separate toast.
    manual_update_check: bool,
    /// Set when a typing-rate action skipped the per-action `save_state`;
    /// the 1.5s poll tick flushes it so nothing is ever lost for long.
    state_dirty: Cell<bool>,
    /// Scroll state for the refs column.
    git_log_refs_scroll: ClippedScrollStateHandle,
    /// Active Settings section (sidebar selection).
    settings_section: SettingsSection,
    /// Editor soft word-wrap default for newly opened files (persisted).
    word_wrap_default: bool,
    /// Strip trailing whitespace on save (persisted; applied to every editor).
    trim_on_save: bool,
    /// Syntect theme override (mirrors `crate::syntax::theme_override`).
    syntax_override: Option<String>,
    /// Scroll state for the Settings section body.
    settings_scroll: ClippedScrollStateHandle,
    /// In-flight sidebar row drag (None = no drag).
    tree_drag: Option<TreeDrag>,
    /// Window-space cursor during a sidebar drag (drives the drop-line).
    tree_drag_pos: Rc<Cell<Option<warpui::geometry::vector::Vector2F>>>,
    /// Sidebar row rects + scopes, repopulated at paint (visual order).
    tree_zones: crate::warpui::rect_probe::ZoneList<TreeScope>,
    /// Previous frame's zones — render-time readers (the drop-line overlay)
    /// use this snapshot: the live list is cleared at render start and only
    /// refills at paint, AFTER the overlay is built.
    tree_zones_last: crate::warpui::rect_probe::ZoneList<TreeScope>,
    /// Per-row `DraggableState`s, keyed by a stable row string.
    tree_drag_states: std::cell::RefCell<HashMap<String, DraggableState>>,
    /// In-flight Files-tree row drag: the source path (None = no drag).
    fs_drag: Option<PathBuf>,
    /// Window-space cursor during a Files-tree drag (drop-target highlight).
    fs_drag_pos: Rc<Cell<Option<warpui::geometry::vector::Vector2F>>>,
    /// Directory-row rects (+ the tree root as a whole), repopulated at paint.
    fs_zones: crate::warpui::rect_probe::ZoneList<PathBuf>,
    /// Previous frame's dir-row zones (render-time drop-hover highlight).
    fs_zones_last: crate::warpui::rect_probe::ZoneList<PathBuf>,
    /// Per-file-row `DraggableState`s.
    fs_drag_states: std::cell::RefCell<HashMap<String, DraggableState>>,
    /// Undo stack for Files-tree ops (Cmd+Z when no editor owns focus).
    file_ops: Vec<FileOp>,
    /// Loaded `git show` detail for the selected commit.
    git_log_detail: Option<crate::warpui::git_log::CommitDetail>,
    /// True while the selected commit's `git show` is computing.
    git_log_detail_loading: bool,
    /// Row scroll offset for the detail/diff panel.
    git_log_detail_scroll: usize,
    /// Selected index in the commit detail's changed-files list.
    git_log_detail_file: usize,
    /// Commit message buffer + whether the commit box has keyboard focus
    /// (keys route to it instead of the terminal).
    commit_msg: String,
    commit_focused: bool,
    /// Draggable left-panel boundary (fraction of the window width).
    left_ratio: Rc<Cell<f32>>,
    left_drag: Rc<Cell<bool>>,
    /// Draggable right-panel boundary (center | right within the remaining area).
    right_ratio: Rc<Cell<f32>>,
    right_drag: Rc<Cell<bool>>,
    /// Selected (project_idx, worktree_idx, tab_idx) — drives breadcrumb +
    /// row highlight. `tab_idx == usize::MAX` means the worktree row itself.
    /// Plain view state: mutated in `handle_action` so warpui re-renders.
    selected: (usize, usize, usize),
    show_left: bool,
    show_right: bool,
    /// Right panel: true = Files tab, false = Changes tab.
    files_tab: bool,
    expanded_dirs: HashSet<PathBuf>,
    selected_file: Option<PathBuf>,
    /// Cached right-panel contents — rebuilt in `refresh_panel` on action, NOT
    /// in render() (which runs every repaint). Avoids shelling out `git` /
    /// walking the FS every frame.
    file_rows: Vec<file_tree::FileRow>,
    changes: Vec<crate::warpui::git::Change>,
    /// Left tree expand state.
    expanded_projects: HashSet<usize>,
    expanded_worktrees: HashSet<(usize, usize)>,
    /// Per-workspace (project, worktree) tab list — the Tab strip. Tabs carry a
    /// STABLE id (not a position) so closing one doesn't reindex the others.
    worktree_tabs: HashMap<(usize, usize), Vec<TabMeta>>,
    /// Monotonic tab id source.
    next_tab_id: usize,
    /// Projects added by the user via "Add Project" (not sourced from session.json).
    added_projects: Vec<crate::warpui::persist::AddedProject>,
    /// Paths of session.json projects the user explicitly removed.
    removed_project_paths: Vec<String>,
    /// Per-project tint overrides keyed by project path.
    project_tints: HashMap<String, [u8; 3]>,
    /// Pool of persistent per-row hover states for context-menu / modal rows,
    /// keyed by a stable string. Menu rows are rebuilt every render, but a
    /// `Hoverable`'s hover must survive the mouse-in → repaint gap, so the
    /// `MouseStateHandle` (an `Arc<Mutex<..>>`) has to persist on the view.
    /// `RefCell` because `render` is `&self` and lazily get-or-inserts handles.
    menu_hover: RefCell<HashMap<String, MouseStateHandle>>,
    /// Per-tab title debounce, keyed by tab id (`TabMeta::id`). Smooths the
    /// live OSC-0/2 terminal title so agent-CLI spinner churn doesn't rename
    /// the Tab row several times a second (see `TitleDebounce`). Keyed by the
    /// globally-unique tab id — NOT the positional `(pi, wi, tid)` tuple —
    /// so it survives project drag-reorder / removal index remaps without
    /// joining `rekey_after_reorder`. `RefCell` because `render` is `&self`
    /// and updates the state on each paint. Entries are dropped when the
    /// owning tab is torn down.
    title_debounce: RefCell<HashMap<usize, TitleDebounce>>,
    /// Active project context menu, or None when no menu is open.
    context_menu: Option<ProjectContextMenu>,
    /// Hover tooltip currently on-screen: (label text, window-space x, y) —
    /// set by `CraneShellAction::ShowTooltip` / cleared by `HideTooltip`,
    /// dispatched from a button's `Hoverable::on_hover`. Rendered by
    /// `tooltip_overlay` in `render()`, positioned via the same `Popover`
    /// on-screen clamp used by the context-menu popovers.
    hover_tip: Option<(String, f32, f32)>,
    /// Collapsed folder groups, keyed by `ProjectNode::group_path`. Absent →
    /// expanded (members visible). Toggled via `CraneShellAction::ToggleGroup`.
    collapsed_groups: HashSet<String>,
    /// Collapsed directory groups in the Changes tree, keyed by their relative
    /// path (e.g. "src/warpui"). Port of old egui `collapsed_change_dirs`.
    collapsed_change_dirs: HashSet<String>,
    /// Commit / stage error surfaced under the commit box (legacy `git_error`).
    commit_error: Option<String>,
    /// Cached `(ahead, behind)` of the active repo's upstream, or None when no
    /// upstream is configured. Recomputed in `refresh_panel`.
    ahead_behind: Option<(usize, usize)>,
    /// Per-relative-path git status char for the Files tree colouring, plus the
    /// set of directory rel-paths that contain a changed descendant. Rebuilt in
    /// `refresh_panel` from `changes`.
    file_status: HashMap<String, char>,
    dirty_dirs: HashSet<String>,
    /// Shared async git-op status (Push / Pull / Fetch / Commit). Written by the
    /// background thread in `git::spawn_git_op`, polled each render for the pill.
    git_op: Arc<Mutex<crate::warpui::git::OpStatus>>,
    /// Repaint waker handed to background git-op threads.
    git_wake: Arc<dyn Fn() + Send + Sync>,
    /// Keeps the git-op repaint stream alive for the view's lifetime.
    _git_repaint: warpui::r#async::SpawnedLocalStream,
    /// Repaint waker for the shell view itself. Cloned into each terminal's PTY
    /// wake (so OSC title changes refresh the tab label), the editor pane's
    /// drag handler (so the Ln/Col status row tracks mouse selection), and the
    /// startup update check (so About surfaces "Update available" promptly).
    ui_wake: Arc<dyn Fn() + Send + Sync>,
    /// Keeps the shell repaint stream alive for the view's lifetime.
    _ui_repaint: warpui::r#async::SpawnedLocalStream,
    /// Active Right-Panel row context menu (Changes or Files row), or None.
    row_menu: Option<RowMenu>,
    /// Inline pending new-file/new-folder editor in the Files tree.
    pending_new_entry: Option<PendingNewEntry>,
    /// Path pending a delete confirmation (confirm overlay).
    pending_delete: Option<PathBuf>,
    /// When set, the branch picker overlay is open at this (x, y).
    branch_picker: Option<(f32, f32)>,
    /// Cached local + remote branch names for the picker (refreshed on open).
    branch_list: Vec<String>,
    /// Scroll state for the Right Panel change/file list (so the commit box
    /// stays reachable when there are many rows). Persists across re-renders.
    right_scroll: ClippedScrollStateHandle,
    /// Scroll state for the branch-picker overlay list.
    branch_scroll: ClippedScrollStateHandle,
    /// Active worktree/branch row context menu (pi, wi, x, y), or None.
    worktree_menu: Option<(usize, usize, f32, f32)>,
    /// Active Tab row context menu ((pi, wi, tid), x, y), or None.
    tab_menu: Option<((usize, usize, usize), f32, f32)>,
    /// Active folder-group header context menu ((group_path, x, y)), or None.
    /// Opened by right-clicking a container-folder header; offers the tint
    /// palette + "Remove folder group" (removes every member project atomically).
    folder_menu: Option<(String, f32, f32)>,
    /// Whether the top-bar ＋ New Pane dropdown is open. Anchored under the
    /// button at render time (fixed offset from the right edge — the top bar
    /// doesn't expose its own button rect), dismissed like any context menu.
    new_pane_menu_open: bool,
    /// Per folder-group tint overrides keyed by the container folder's own path
    /// (`ProjectNode::group_path`). Painted on the FOLDER header icon + label.
    group_tints: HashMap<String, [u8; 3]>,
    /// In-flight inline rename (worktree or tab), or None. While `Some`, typed
    /// keys route to its buffer (top priority in `SendKeys`).
    renaming: Option<RenameState>,
    /// Per-worktree display-name overrides, keyed by the worktree checkout PATH
    /// (stable across reloads; indices shift). Wins over the branch name.
    worktree_names: HashMap<String, String>,
    /// Per-worktree tint overrides keyed by the worktree checkout PATH; applied
    /// to the GIT_BRANCH icon + name in the nav row.
    worktree_tints: HashMap<String, [u8; 3]>,
    /// Per-tab tint overrides keyed by (worktree_path, tab_id); applied to the
    /// Tab row icon + label.
    tab_tints: HashMap<(String, usize), [u8; 3]>,
    /// Last worktree-row click ((pi, wi), instant) — drives double-click → rename.
    last_wt_click: Option<((usize, usize), std::time::Instant)>,
    /// Last tab-row click ((pi, wi, tid), instant) — drives double-click → rename.
    last_tab_click: Option<((usize, usize, usize), std::time::Instant)>,
    /// The active full-screen modal (quit confirm / close-pane confirm / Settings
    /// / Help), or None. Rendered last (topmost) in the root stack. Transient —
    /// never persisted.
    modal: Option<Modal>,
    /// Detail for the open `Modal::ConfirmRemoveWorktree` card (label / path /
    /// dirty / unpushed warning), computed once when that modal opens.
    remove_wt_info: Option<RemoveWtInfo>,
    /// Set once the user confirms Quit in the ConfirmQuit modal so the re-issued
    /// terminate passes straight through the `on_should_terminate_app` guard.
    confirmed_quit: bool,
    /// Scroll state for the Help / Settings modal body (tall content).
    modal_scroll: ClippedScrollStateHandle,
    /// Cmd+Shift+F Find-in-Files modal state, or None when closed. `self.modal`
    /// mirrors this as `Modal::FindInFiles` while it is open.
    find_in_files: Option<FindInFilesState>,
    /// Scroll state for the Find-in-Files result list.
    find_scroll: ClippedScrollStateHandle,
    /// Cmd+` tab-switcher overlay state, or None when closed. `self.modal`
    /// mirrors this as `Modal::TabSwitcher` while it is open.
    tab_switcher: Option<TabSwitcherState>,
    /// Scroll state for the tab-switcher list.
    switcher_scroll: ClippedScrollStateHandle,
    /// "Switch Branch" modal state, or None when closed. Mirrored by
    /// `Modal::SwitchBranch`.
    switch_branch: Option<SwitchBranchState>,
    /// Scroll state for the Switch-Branch list.
    switch_branch_scroll: ClippedScrollStateHandle,
    /// "New Workspace" modal state, or None when closed. Mirrored by
    /// `Modal::NewWorkspace`.
    new_workspace: Option<NewWorkspaceState>,
    /// Per-project cache of the last `git worktree list` signature, used by the
    /// background worktree-poll tick to skip re-computing when nothing changed.
    worktree_poll_sig: HashMap<String, String>,
    /// Per-scope generation counter for keyed async git scans (the reusable
    /// dedup / cancel-on-supersede primitive backing `spawn_git_scan`). A scope
    /// is an arbitrary string key — `"tree"` for a whole-tree sidebar backfill,
    /// `"add:<path>"` for a freshly added project, `"panel:<repo>"` for the
    /// active-repo Changes/status refresh. Each spawn bumps its scope's counter
    /// and captures the new value; when the background result lands, the
    /// callback drops it if a newer generation for the same scope has since
    /// superseded it. This gives OG's job-dedup / cancel semantics without a
    /// condvar / priority thread pool.
    git_scan_gen: HashMap<String, u64>,
    /// Keeps the worktree-detection poll stream alive for the view's lifetime.
    _worktree_tick: warpui::r#async::SpawnedLocalStream,

    // ── LSP wiring ───────────────────────────────────────────────────────────
    /// The language-server client. Diagnostics + goto-definition for the active
    /// editor. No-ops gracefully when no matching server is installed.
    lsp: crate::lsp::LspManager,
    /// Wake handle handed to `LspManager` so its background threads can nudge
    /// the UI to repaint when async results land. Shares the shell's `ui_wake`
    /// closure, which feeds the `_ui_repaint` stream.
    lsp_wake: crate::lsp::Wake,
    /// Per-language behavior toggles. Default set (matches the egui app's
    /// startup `LanguageConfigs::default()`); not yet surfaced in warpui
    /// Settings, so it never diverges from the per-server defaults.
    lsp_configs: crate::lsp::LanguageConfigs,
    /// Last `buffer_version` sent to the server per open editor path — drives
    /// `did_change` change detection (send only on an actual content edit).
    lsp_versions: HashMap<PathBuf, u64>,
    /// Last diagnostics fingerprint pushed to each editor. Avoids re-pushing
    /// (and re-rendering) identical diagnostics every poll tick.
    lsp_diag_sig: HashMap<PathBuf, Vec<(u32, u32, u32, u8)>>,
    /// In-flight goto-definition requests, polled each tick until they resolve
    /// (or a 5s watchdog prunes them). Port of the egui app's `pending_gotos`.
    pending_gotos: Vec<PendingGoto>,
    /// 300ms poll timer: ticks `LspManager`, syncs the active editor's
    /// `did_change` + diagnostics, and drains goto results. Kept alive for the
    /// view's lifetime.
    _lsp_tick: warpui::r#async::SpawnedLocalStream,
    /// Editor Language Server opt-in. OFF by default: the agent CLI is the
    /// code-intelligence layer, so no rust-analyzer (or any server) is spawned
    /// unless the user turns this on in Settings. Every LSP side effect
    /// (did_open on file open, the poll tick body, goto-definition) is gated on
    /// this flag. Persisted via `WarpuiState::lsp_enabled`.
    lsp_enabled: bool,
    /// Editor format-on-save opt-in. ON by default (old-egui parity). When on,
    /// Cmd+S routes through `EditorView::save_on_cmd_s`, which formats the buffer
    /// off-thread before the write; a formatter error keeps the original bytes.
    /// Toggled from Settings > Editor. Persisted via `WarpuiState::format_on_save`.
    format_on_save: bool,

    // ── Agent-native wiring ───────────────────────────────────────────────────
    /// Filesystem watcher: external / agent edits under any watched Project or
    /// Workspace (worktree) root trigger a Changes/Files/diff refresh for the
    /// ACTIVE repo. Kept alive for the view's lifetime (Drop tears down the OS
    /// watch + joins the debounce thread).
    file_watcher: crate::warpui::file_watcher::FileWatcher,
    /// Coalesced change events drained on the fast tick. Each `ChangeEvent.root`
    /// is a canonicalized watched root (matched against `canonicalize(active_cwd)`
    /// to decide whether the ACTIVE repo needs a refresh).
    fs_events: std::sync::mpsc::Receiver<crate::warpui::file_watcher::ChangeEvent>,
    /// Original path strings currently registered with `file_watcher`, so
    /// `sync_watches` only canonicalizes / (un)registers on an actual change
    /// instead of every tick.
    watched: HashSet<String>,
    /// Bounded FIFO of live notification toasts (OSC 9 / OSC 777). Swept by the
    /// fast tick; rendered as a bottom-right overlay.
    toasts: VecDeque<Toast>,
    /// Monotonic toast id source.
    next_toast_id: u64,
    /// Keeps the fast (250ms) tick alive: sweeps expired toasts and drains
    /// `fs_events` so external edits refresh the active repo without input.
    _fast_tick: warpui::r#async::SpawnedLocalStream,
    /// 33ms browser reconcile tick (`browser_tick`). Near-idle when no Browser
    /// Pane exists — a single `matches!` scan of `self.panes` per tick.
    _browser_tick: warpui::r#async::SpawnedLocalStream,
    /// 33ms animation tick — repaints while a toast or attention pulse is on
    /// screen so the glow/dot breathe at full frame rate instead of the fast
    /// tick's 4 FPS. No-op boolean check when idle.
    _anim_tick: warpui::r#async::SpawnedLocalStream,
    /// Native WKWebView slots for Browser Panes, reconciled by `browser_tick`.
    browser_host: crate::warpui::browser::BrowserHost,
}

/// An in-flight goto-definition request token (server + JSON-RPC id) plus the
/// time it was dispatched, so a watchdog can prune requests that never resolve.
struct PendingGoto {
    server: crate::lsp::ServerKey,
    request_id: i64,
    dispatched_at: std::time::Instant,
}

/// One in-app desktop-notification toast surfaced from an OSC 9 / OSC 777
/// escape a program in a Crane terminal emitted (agent CLI `Stop`/`Notification`
/// hooks, build scripts, …). Bounded FIFO in `CraneShellView::toasts`; rendered
/// as a bottom-right stack overlay that auto-dismisses after `TOAST_TTL`. Port
/// of old egui `PaneNotification` + `src/modals/notification_toast.rs`, pared to
/// what the warpui terminal view forwards — body + urgency. The view carries no
/// pane locator, so the source is captured best-effort from the tab that was
/// active when the notification arrived (used for the header label + click-to-
/// focus).
struct Toast {
    /// Monotonic id (stable across re-renders) — the dismiss / focus actions
    /// target a toast by id, not by shifting index.
    id: u64,
    body: String,
    /// True for OSC 777 (urgent) — coloured stroke + WARNING glyph.
    urgent: bool,
    /// Best-effort source breadcrumb ("project / branch  ·  tab").
    source: String,
    /// The tab to activate when the toast body is clicked, or None.
    tab_key: Option<(usize, usize, usize)>,
    /// When the toast was raised — drives the TTL sweep.
    at: std::time::Instant,
}

/// How long a toast stays on screen before the fast tick sweeps it. Long enough
/// to notice + read a background agent's "done" ping and click through to it,
/// without lingering so long a burst piles up (bounded further by `TOAST_MAX`).
const TOAST_TTL: std::time::Duration = std::time::Duration::from_secs(10);
/// Max simultaneous toasts — the oldest is dropped when a burst exceeds this.
const TOAST_MAX: usize = 5;

/// Clamp a notification body to `max` chars, appending an ellipsis when cut.
fn truncate_body(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        // ASCII ellipsis — bundled JetBrains Mono may not cover U+2026.
        out.push_str("...");
    }
    out
}

#[derive(Clone)]
pub struct TabMeta {
    pub id: usize,
    pub name: String,
    /// True once the user has explicitly renamed this tab. A renamed tab pins
    /// its chosen `name` and no longer follows the terminal's live OSC-0/2
    /// title (which would otherwise clobber the rename on the next PTY byte).
    pub renamed: bool,
    /// When a background notification / bell last arrived for this tab, driving
    /// the Left-Panel attention pulse. `Some` → the row breathes an accent glow +
    /// unread dot until the user opens the tab (which clears it). Runtime-only
    /// (not persisted). Port of old egui `Tab::attention_since`.
    pub attention_since: Option<std::time::Instant>,
}

/// A single match row in the Find-in-Files modal.
pub struct FifMatch {
    /// Absolute path of the file the match lives in.
    pub path: PathBuf,
    /// Root-relative display path (e.g. "src/warpui/shell.rs").
    pub display: String,
    /// 1-based line number of the match.
    pub line: u32,
    /// The matched line's text (trimmed of trailing newline), trimmed of
    /// leading whitespace for compact display.
    pub text: String,
}

/// State for the Cmd+Shift+F Find-in-Files modal. Keys route to `query` via the
/// same keystroke path as the commit box (`edit_find_in_files`); each edit
/// re-runs a synchronous recursive substring search over the active project.
pub struct FindInFilesState {
    pub query: String,
    pub results: Vec<FifMatch>,
    /// True when the result set was capped at `FIF_MAX_RESULTS`.
    pub truncated: bool,
    /// Highlighted result row (Enter opens it).
    pub selected: usize,
}

/// State for the Cmd+` tab switcher overlay. `entries` is the list of
/// `(project_idx, worktree_idx, tab_id)` in the active workspace; `highlight`
/// is the row that Enter / Cmd-` release activates.
pub struct TabSwitcherState {
    pub entries: Vec<(usize, usize, usize)>,
    pub highlight: usize,
}

/// State for the "Switch Branch" modal. Keys route to `query` (via
/// `edit_switch_branch`), which filters `all` (local + deduped remote short
/// names) into the rendered list. Picking a branch checks it out in the active
/// workspace; each row also offers "+ worktree" (open New Workspace pre-filled).
pub struct SwitchBranchState {
    pub query: String,
    /// The active project index (for "+ worktree" / new-branch worktree flows).
    pub project_idx: usize,
    /// Every candidate branch name (locals first, then deduped remote shorts).
    pub all: Vec<String>,
    /// Local branch names (subset of `all`) — used only to tag remote-only rows.
    pub locals: std::collections::HashSet<String>,
    /// Highlighted row (Enter checks it out).
    pub selected: usize,
    /// True while the off-thread `git branch` / `git branch -r` listing is in
    /// flight. Enter is a no-op while loading so a keystroke before the list
    /// lands can't fall through the empty `filtered` list into an accidental
    /// CreateBranchCheckout (which would create a branch the user meant to check
    /// out).
    pub loading: bool,
    /// Generation stamped when this modal opened. The async listing callback
    /// drops its result unless this still matches (a close+reopen bumps it),
    /// so a stale scan can't repopulate a newer modal.
    pub load_gen: u64,
}

/// State for the "New Workspace" modal — create a `git worktree` for a branch.
pub struct NewWorkspaceState {
    /// The project the worktree is created under.
    pub project_idx: usize,
    /// The branch name being typed / chosen.
    pub branch: String,
    /// When true, create a brand-new branch (`worktree add -b`); else check out
    /// an existing branch into the new worktree.
    pub new_branch: bool,
    /// Opened from the branch picker with an EXISTING branch — the field is
    /// read-only and the new-branch checkbox hides (a `worktree add -b
    /// <existing>` would fail; old modal's `branch_locked`).
    pub branch_locked: bool,
    /// Where the checkout lands (old `LocationMode` selector).
    pub mode: LocationMode,
    /// Parent folder for `LocationMode::Custom` (via Browse… or typed).
    pub custom_path: String,
    /// True while the custom-path field owns typing (else the branch field).
    pub path_focused: bool,
    /// Error surfaced under the field on a failed `git worktree add`.
    pub error: Option<String>,
}

/// Where a new Workspace's worktree checkout is created (old
/// `state::LocationMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocationMode {
    /// `~/.crane-worktrees/<project>/<branch>` — the default.
    Global,
    /// `<project>/.crane-worktrees/<branch>` — beside the code.
    ProjectLocal,
    /// A user-picked parent folder.
    Custom,
}

/// Hard cap on Find-in-Files matches so a broad query in a huge tree can't wedge
/// the UI (the search runs synchronously on the UI thread per keystroke).
const FIF_MAX_RESULTS: usize = 500;
/// Skip files larger than this (bytes) — almost always minified/vendored blobs.
const FIF_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// In-flight inline rename of a worktree/branch row or a Tab row. `buffer` is
/// the live edit text; typed keys route here (see `edit_rename`) while it is
/// `Some`, exactly like the commit box captures keys via `commit_focused`.
pub struct RenameState {
    pub target: RenameTarget,
    pub buffer: String,
}

/// What the active inline rename targets.
pub enum RenameTarget {
    /// A worktree/branch row — commits to a per-path display-name override.
    Worktree { pi: usize, wi: usize },
    /// A Tab row — commits to `TabMeta.name`.
    Tab { key: (usize, usize, usize) },
}

/// What a leaf pane holds (warpui port of old Crane's `PaneContent`). More
/// variants (Browser, GitLog) follow.
pub enum PaneContent {
    Terminal(ViewHandle<TerminalView>),
    File(ViewHandle<FileView>),
    /// Warp's real text editor (warp_editor) — warp-quality file editing.
    Editor(ViewHandle<crate::warpui::editor_view::WarpEditorView>),
    /// Landing / new-tab surface (wordmark + action cards + cheat-sheet).
    Welcome(ViewHandle<WarpWelcomeView>),
    /// Read-only rendered Markdown document (`.md` / `.markdown`).
    Markdown(ViewHandle<crate::warpui::markdown_view::WarpMarkdownView>),
    /// Read-only unified diff (HEAD vs working copy) for a changed file.
    Diff(ViewHandle<crate::warpui::diff_view::WarpDiffView>),
    /// Embedded browser: the view draws tab strip / URL toolbar / footer and
    /// reserves the body rect; the native WKWebViews are reconciled against it
    /// by `browser::BrowserHost` on the shell's browser tick.
    Browser(ViewHandle<crate::warpui::browser_view::WarpBrowserView>),
}

/// A mutating git-log commit operation (context-menu verbs).
#[derive(Clone, Copy)]
enum GitLogOp {
    Checkout,
    CherryPick,
    Revert,
}

/// An in-flight sidebar row drag (old `ui/projects.rs::TreeDrag`), identified
/// by PATH (not index) so a background `poll_worktrees` tick that shifts
/// indices mid-gesture can't retarget the drop.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeDrag {
    /// A standalone or grouped project row; `group` = its `group_path`.
    Project { path: String, group: Option<String> },
    /// A folder-group header — the whole block moves as a unit.
    Group { path: String },
    /// A Workspace (branch) row inside `project`.
    Worktree { project: String, path: String },
    /// A Tab row inside (`project`, `worktree`).
    Tab { project: String, worktree: String, id: usize },
}

/// Drop-scope tag on each sidebar row's painted rect (old `DropScope`): the
/// drop dispatcher filters rows to siblings of the dragged item.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeScope {
    /// Top-level row: standalone project or folder-group header.
    Root,
    /// Sub-project inside the folder group at `group`.
    InBlock { group: String },
    /// Workspace row of `project`.
    Worktree { project: String },
    /// Tab row of (`project`, `worktree`).
    Tab { project: String, worktree: String },
}

/// One release version's persisted update-prompt decision (old
/// `update/check.rs::PromptState`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum UpdatePrompt {
    /// "Skip this version" — never prompt for it again.
    Dismissed,
    /// "Remind in 7 days" — resurface once `now >= at` (epoch seconds).
    RemindAt(u64),
}

/// Old check.rs `REMIND_AFTER_SECS`.
const UPDATE_REMIND_SECS: u64 = 7 * 24 * 60 * 60;

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A completed (undoable) Files-tree file operation — old Crane's
/// `undo_last_file_op` stack. Trash is not undoable programmatically (the
/// `trash` crate has no restore), so only Move/Copy land here.
#[derive(Clone, Debug)]
enum FileOp {
    /// `fs::rename(from → to)`; undo renames back.
    Move { from: PathBuf, to: PathBuf },
    /// A path created by a copy (internal alt-copy or an external OS drop);
    /// undo moves it to the Trash (recoverable, never a permanent unlink).
    Copy { created: PathBuf },
}

/// Settings dialog sections (old `modals/settings.rs::SettingsSection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Appearance,
    Editor,
    Terminal,
    LanguageServers,
    Shortcuts,
    About,
}

impl SettingsSection {
    const ALL: [SettingsSection; 6] = [
        SettingsSection::Appearance,
        SettingsSection::Editor,
        SettingsSection::Terminal,
        SettingsSection::LanguageServers,
        SettingsSection::Shortcuts,
        SettingsSection::About,
    ];
    fn title(self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Editor => "Editor",
            SettingsSection::Terminal => "Terminal",
            SettingsSection::LanguageServers => "Language Servers",
            SettingsSection::Shortcuts => "Shortcuts",
            SettingsSection::About => "About",
        }
    }
}

/// Map a dock edge to (split direction, dragged-goes-first?). Center → None
/// (handled as a swap, not a split).
fn edge_dir_before(edge: DockEdge) -> Option<(Dir, bool)> {
    match edge {
        DockEdge::Left => Some((Dir::Horizontal, true)),
        DockEdge::Right => Some((Dir::Horizontal, false)),
        DockEdge::Top => Some((Dir::Vertical, true)),
        DockEdge::Bottom => Some((Dir::Vertical, false)),
        DockEdge::Center => None,
    }
}

impl CraneShellView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let ui_font = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| crate::warpui::bundled_fonts::ui(cache));
        let icon_font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .load_family_from_bytes(
                    "phosphor",
                    vec![include_bytes!("assets/Phosphor.ttf").to_vec()],
                )
                .expect("load phosphor")
        });
        // Monospace face for the Git Log lane graph + commit rows (fixed advance
        // keeps the graph columns and hash/subject/meta columns aligned).
        // Bundled JetBrains Mono (graceful Menlo fallback inside).
        let mono_font = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| crate::warpui::bundled_fonts::mono(cache));
        // Load warpui persisted state early so we can apply the project overlay
        // (added/removed/tints) when building the initial project list.
        let saved_state = crate::warpui::persist::load();
        let init_added: Vec<crate::warpui::persist::AddedProject> = saved_state
            .as_ref()
            .map(|s| s.added_projects.clone())
            .unwrap_or_default();
        let init_removed: Vec<String> = saved_state
            .as_ref()
            .map(|s| s.removed_project_paths.clone())
            .unwrap_or_default();
        let init_tints: HashMap<String, [u8; 3]> = saved_state
            .as_ref()
            .map(|s| s.project_tints.iter().cloned().collect())
            .unwrap_or_default();
        let init_wt_names: HashMap<String, String> = saved_state
            .as_ref()
            .map(|s| s.worktree_names.iter().cloned().collect())
            .unwrap_or_default();
        let init_wt_tints: HashMap<String, [u8; 3]> = saved_state
            .as_ref()
            .map(|s| s.worktree_tints.iter().cloned().collect())
            .unwrap_or_default();
        let init_tab_tints: HashMap<(String, usize), [u8; 3]> = saved_state
            .as_ref()
            .map(|s| s.tab_tints.iter().cloned().collect())
            .unwrap_or_default();
        let init_group_tints: HashMap<String, [u8; 3]> = saved_state
            .as_ref()
            .map(|s| s.group_tints.iter().cloned().collect())
            .unwrap_or_default();
        // LSP opt-in, restored from warpui-state.json (default OFF — read here
        // before `saved_state` is consumed by the restore block below).
        let init_lsp_enabled: bool =
            saved_state.as_ref().map(|s| s.lsp_enabled).unwrap_or(false);
        // Format-on-save opt-in — default ON (old-egui parity) when there is no
        // saved state at all; existing state files carry the persisted value.
        let init_format_on_save: bool =
            saved_state.as_ref().map(|s| s.format_on_save).unwrap_or(true);
        let init_word_wrap: bool = saved_state.as_ref().map(|s| s.word_wrap).unwrap_or(false);
        let init_trim: bool = saved_state.as_ref().map(|s| s.trim_on_save).unwrap_or(false);
        let init_syntax_override: Option<String> = saved_state
            .as_ref()
            .map(|s| s.syntax_override.clone())
            .filter(|s| !s.is_empty());
        // Base font sizes + syntax override are process-global (read per paint),
        // so seed them before the first frame.
        if let Some(st) = saved_state.as_ref() {
            if st.terminal_font > 0.0 {
                crate::warpui::fontsize::set_base(st.terminal_font);
            }
            if st.editor_font > 0.0 {
                crate::warpui::fontsize::set_editor(st.editor_font);
            }
        }
        crate::syntax::set_theme_override(init_syntax_override.clone());
        // Restore per-version update-prompt decisions ("skip" / "remind").
        let init_update_prompts: HashMap<String, UpdatePrompt> = saved_state
            .as_ref()
            .map(|s| {
                s.update_prompts
                    .iter()
                    .filter_map(|(v, p)| {
                        if p == "dismissed" {
                            Some((v.clone(), UpdatePrompt::Dismissed))
                        } else {
                            p.strip_prefix("remind:")
                                .and_then(|t| t.parse::<u64>().ok())
                                .map(|t| (v.clone(), UpdatePrompt::RemindAt(t)))
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // SHALLOW load: build the full tree STRUCTURE with zero `git` subprocess
        // so the first frame paints immediately even with many worktrees. Branch
        // labels + diff/dirty badges are filled in a moment later by the async
        // scan kicked off at the end of `new` (see `spawn_git_scan` below).
        let mut projects = crate::warpui::projects::load_projects_shallow(
            &init_added, &init_removed, &init_tints,
        );
        // Apply the persisted sidebar drag-drop ordering BEFORE any keyed
        // restore below — worktree_tabs / layouts were saved against these
        // positions. Stable sort: paths absent from the saved order (freshly
        // discovered projects/worktrees) keep their load order at the end.
        if let Some(order) = saved_state.as_ref().map(|s| s.sidebar_order.clone()) {
            if !order.is_empty() {
                let p_rank: HashMap<&str, usize> = order
                    .iter()
                    .enumerate()
                    .map(|(i, (p, _))| (p.as_str(), i))
                    .collect();
                projects.sort_by_key(|p| {
                    p_rank.get(p.path.as_str()).copied().unwrap_or(usize::MAX)
                });
                for p in projects.iter_mut() {
                    if let Some((_, wts)) =
                        order.iter().find(|(pp, _)| pp.as_str() == p.path.as_str())
                    {
                        let w_rank: HashMap<&str, usize> = wts
                            .iter()
                            .enumerate()
                            .map(|(i, w)| (w.as_str(), i))
                            .collect();
                        p.worktrees.sort_by_key(|w| {
                            w_rank.get(w.path.as_str()).copied().unwrap_or(usize::MAX)
                        });
                    }
                }
            }
        }
        // Default the terminal to the first project's first worktree folder.
        let default_cwd = projects
            .first()
            .map(|p| {
                p.worktrees
                    .first()
                    .map(|w| w.path.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| p.path.clone())
            })
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let file_rows = match &default_cwd {
            Some(root) => file_tree::build_rows(root, &HashSet::new()),
            None => Vec::new(),
        };
        let active_cwd = default_cwd.clone();
        // Open the default worktree's first tab as a single-leaf layout, seeding
        // its Tab strip from session.json (or one default tab).
        let mut panes: HashMap<PaneId, PaneContent> = HashMap::new();
        let mut layouts: HashMap<(usize, usize, usize), Node> = HashMap::new();
        let mut worktree_tabs: HashMap<(usize, usize), Vec<TabMeta>> = HashMap::new();
        let mut drag_states: HashMap<PaneId, DraggableState> = HashMap::new();
        // Shell repaint channel. Any background/child ping — a terminal's PTY
        // output (incl. OSC-0/2 title changes that feed the tab label), a
        // mouse-drag selection in an editor (feeds the Ln/Col status row), and
        // the background update check (feeds Settings > About) — sends here so the
        // CraneShell view itself re-renders. The stream handler only marks the
        // view dirty, so it stays cheap even under heavy terminal output (this
        // matches the original egui build, which repainted the whole UI per frame).
        let (ui_tx, ui_rx) = async_channel::bounded::<()>(1);
        let ui_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = ui_tx.try_send(());
        });
        // Kick off the background GitHub-releases update check once at startup,
        // handing it the shell repaint waker so an "Update available" result
        // surfaces in Settings > About immediately (not on the next incidental
        // repaint). Idempotent + non-blocking.
        {
            let wake = ui_wake.clone();
            crate::warpui::update::spawn_check(move || wake());
        }
        let mut active_tab = None;
        let mut focused = None;
        let mut selected = (0, 0, usize::MAX);
        let mut next_pane_id: PaneId = 0;
        let mut next_tab_id: usize = 0;
        // UI prefs, restored from warpui-state.json if present.
        let mut show_left = true;
        let mut show_right = true;
        // Right Panel defaults to the Changes tab (not Files) on a fresh
        // install — Changes is the more common thing to check first.
        let mut files_tab = false;
        let mut expanded_projects: HashSet<usize> = HashSet::from([0]);
        let mut expanded_worktrees: HashSet<(usize, usize)> = HashSet::from([(0, 0)]);
        let mut restored_files_pane: Option<PaneId> = None;
        let mut restored_file_paths: Vec<PathBuf> = Vec::new();
        let mut restored_active: usize = 0;
        let mut restored_editor_views: HashMap<
            PathBuf,
            ViewHandle<crate::warpui::editor_view::WarpEditorView>,
        > = HashMap::new();
        let mut restored_term_cache: HashMap<PaneId, crate::warpui::persist::STerminal> =
            HashMap::new();
        let mut restored_browsers: HashMap<PaneId, crate::warpui::persist::SBrowser> =
            HashMap::new();
        let mut saved_active: usize = 0;

        // Ensure built-in theme TOML files are written to ~/.crane/themes/ on
        // first launch so users have a working template for each palette.
        crate::theme::ensure_builtin_tomls_on_disk();

        // RESTORE: rebuild tabs + split layouts from the persisted state. Each
        // saved leaf respawns a terminal in its worktree cwd — EXCEPT the saved
        // File pane leaf, which is rebuilt as a File pane with its open files.
        if let Some(st) = saved_state {
            // Restore the active theme BEFORE building any UI so every colour
            // token call below reads the right palette.
            if !st.theme_name.is_empty() {
                if let Some(t) = crate::theme::find_by_name(&st.theme_name) {
                    crate::theme::set(t);
                }
            }
            if st.zoom_level > 0.0 {
                crate::warpui::fontsize::set_level(st.zoom_level);
                ctx.set_zoom_factor(st.zoom_level);
            }
            show_left = st.show_left;
            show_right = st.show_right;
            files_tab = st.files_tab;
            expanded_projects = st.expanded_projects.iter().copied().collect();
            expanded_worktrees = st.expanded_worktrees.iter().copied().collect();
            next_tab_id = st.next_tab_id;
            next_pane_id = st.next_pane_id;
            let saved_files_pane = st.files_pane;
            let saved_paths = st.file_pane_paths.clone();
            saved_active = st.file_pane_active;
            restored_term_cache = st.terminals.iter().cloned().collect();
            restored_browsers = st.browsers.iter().cloned().collect();
            for ((pi, wi), stabs) in &st.worktree_tabs {
                let Some(wpath) = projects
                    .get(*pi)
                    .and_then(|p| p.worktrees.get(*wi))
                    .map(|w| PathBuf::from(&w.path))
                else {
                    continue;
                };
                let mut metas = Vec::new();
                for stab in stabs {
                    let mut leaves = Vec::new();
                    stab.layout.leaves(&mut leaves);
                    for pid in leaves {
                        // The File pane leaf is rebuilt as a File pane (with its
                        // tabs); everything else is a terminal.
                        if Some(pid) == saved_files_pane && !saved_paths.is_empty() {
                            // Rebuild the file pane with Warp's REAL editor. Build a
                            // live editor for EVERY saved path (kept in editor_views)
                            // so all tabs restore and switch; show the active one.
                            let mono = warpui::fonts::Cache::handle(ctx).update(
                                ctx,
                                |cache, _| crate::warpui::bundled_fonts::mono(cache),
                            );
                            for p in &saved_paths {
                                let content = std::fs::read_to_string(p).unwrap_or_default();
                                let pc = p.clone();
                                let goto = Self::lsp_goto_cb(p.clone());
                                let h = ctx.add_typed_action_view(move |ctx| {
                                    crate::warpui::editor_view::WarpEditorView::new(
                                        ctx, content, mono, pc,
                                    )
                                    .with_goto(goto)
                                });
                                h.update(ctx, |v, vctx| {
                                    if init_word_wrap {
                                        v.set_word_wrap(true, vctx);
                                    }
                                    v.set_trim_on_save(init_trim);
                                });
                                restored_editor_views.insert(p.clone(), h);
                            }
                            let active = saved_active.min(saved_paths.len() - 1);
                            let active_h = restored_editor_views[&saved_paths[active]].clone();
                            panes.insert(pid, PaneContent::Editor(active_h));
                            restored_files_pane = Some(pid);
                            restored_file_paths = saved_paths.clone();
                            restored_active = active;
                        } else if let Some(sb) = restored_browsers.get(&pid) {
                            // Rebuild a Browser pane with its saved tabs; the
                            // webviews materialise (and start loading) on the
                            // first browser tick after the pane paints.
                            let tabs = sb.tabs.clone();
                            let active = sb.active;
                            let h = ctx.add_typed_action_view(move |_ctx| {
                                crate::warpui::browser_view::WarpBrowserView::new(
                                    pid, ui_font, icon_font, tabs, active,
                                )
                            });
                            panes.insert(pid, PaneContent::Browser(h));
                        } else if let Some(st) = restored_term_cache.get(&pid) {
                            // Restore the terminal in its saved cwd and replay its
                            // ANSI scrollback so it looks as it did last session.
                            let cwd = if st.cwd.as_os_str().is_empty() {
                                wpath.clone()
                            } else {
                                st.cwd.clone()
                            };
                            let history = st.history.clone();
                            let h = ctx.add_view(move |ctx| {
                                crate::warpui::view::TerminalView::new_restore(ctx, cwd, history)
                            });
                            panes.insert(pid, PaneContent::Terminal(h));
                        } else {
                            panes.insert(
                                pid,
                                PaneContent::Terminal(Self::spawn_terminal(
                                    ctx,
                                    wpath.clone(),
                                    ui_wake.clone(),
                                )),
                            );
                        }
                        drag_states.insert(pid, DraggableState::default());
                    }
                    layouts.insert((*pi, *wi, stab.id), stab.layout.to_node());
                    metas.push(TabMeta {
                        id: stab.id,
                        name: stab.name.clone(),
                        renamed: stab.renamed,
                        attention_since: None,
                    });
                }
                if !metas.is_empty() {
                    worktree_tabs.insert((*pi, *wi), metas);
                }
            }
            // Restore the active tab if its layout survived.
            if let Some(at) = st.active_tab {
                if layouts.contains_key(&at) {
                    active_tab = Some(at);
                    selected = at;
                    // Prefer the saved per-tab focus (if it's still a live leaf),
                    // otherwise fall back to the layout's first leaf.
                    let saved_focus = st
                        .worktree_tabs
                        .iter()
                        .find(|((pi, wi), _)| *pi == at.0 && *wi == at.1)
                        .and_then(|(_, stabs)| stabs.iter().find(|s| s.id == at.2))
                        .and_then(|s| s.focus)
                        .filter(|pid| panes.contains_key(pid));
                    focused = saved_focus.or_else(|| layouts.get(&at).map(|n| n.first_leaf()));
                }
            }
        }

        // DEFAULT SEED (only if nothing was restored).
        if active_tab.is_none()
            && let Some(path) = default_cwd
        {
            let names: Vec<String> = projects
                .first()
                .and_then(|p| p.worktrees.first())
                .map(|w| w.tabs.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| vec!["Terminal".to_string()]);
            let metas: Vec<TabMeta> = names
                .into_iter()
                .map(|name| {
                    let id = next_tab_id;
                    next_tab_id += 1;
                    TabMeta { id, name, renamed: false, attention_since: None }
                })
                .collect();
            let first_id = metas[0].id;
            worktree_tabs.insert((0, 0), metas);
            let key = (0, 0, first_id);
            let pane = next_pane_id;
            next_pane_id += 1;
            panes.insert(
                pane,
                PaneContent::Terminal(Self::spawn_terminal(ctx, path, ui_wake.clone())),
            );
            drag_states.insert(pane, DraggableState::default());
            layouts.insert(key, Node::Leaf(pane));
            active_tab = Some(key);
            focused = Some(pane);
            selected = key;
        }
        // Async git-op wake: the background thread pings this channel; the
        // spawned stream re-runs the panel refresh + repaints on the main thread.
        let (git_tx, git_rx) = async_channel::bounded::<()>(1);
        let git_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = git_tx.try_send(());
        });
        let git_repaint = ctx.spawn_stream_local(
            git_rx,
            |this: &mut Self, _item, vctx| {
                guarded_tick(this, vctx, "git-wake", |this, vctx| {
                    this.refresh_panel(vctx);
                    this.invalidate_editor_diffs(&*vctx);
                    vctx.notify();
                });
            },
            |_this, _vctx| {},
        );
        // Lightweight shell repaint stream fed by `ui_wake` (see the channel set
        // up near the top of `new`). It only marks the shell view dirty — no git
        // shell-out, no panel rebuild — so terminal-output-frequency pings stay cheap.
        let ui_repaint = ctx.spawn_stream_local(
            ui_rx,
            |_this: &mut Self, _item, vctx| {
                vctx.notify();
            },
            |_this, _vctx| {},
        );
        // LSP poll timer. warpui has no per-frame hook we can mutate the view
        // from, and the egui repaint callback is a one-shot on a frame-less
        // context, so a 300ms interval stream drives the LSP: it ticks the
        // manager, sends `did_change` when the active editor's buffer version
        // moved, pushes fresh diagnostics, and drains goto-definition results.
        // Cheap when idle (a couple hashmap lookups + lock reads; no repaint
        // unless something actually changed).
        let lsp_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(300));
        let lsp_tick = ctx.spawn_stream_local(
            lsp_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "lsp", |this, vctx| this.poll_lsp(vctx));
            },
            |_this, _vctx| {},
        );
        // Worktree auto-detection poll. Every ~1.5s it reconciles each git
        // project's in-memory worktrees against `git worktree list` (add ones
        // created outside the app, flip a loose folder to git when `.git`
        // appears, drop ones removed on disk). Cheap when idle: a per-project
        // `git worktree list` whose output is signature-cached, and heavier
        // per-worktree git (branch/diff/dirty) only for worktrees that changed.
        let wt_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(1500));
        let worktree_tick = ctx.spawn_stream_local(
            wt_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "worktree", |this, vctx| {
                    this.poll_worktrees(vctx);
                    this.poll_editor_disk_changes(vctx);
                    // Register any worktrees discovered this tick (idempotent —
                    // only canonicalizes/registers newly-seen roots).
                    this.sync_watches();
                    this.update_tick(vctx);
                    // Flush state deferred by typing-rate actions (see the
                    // handle_action tail): at most one disk write per tick.
                    if this.state_dirty.get() {
                        this.state_dirty.set(false);
                        this.save_state(&*vctx);
                    }
                });
            },
            |_this, _vctx| {},
        );
        // Filesystem watcher for external / agent edits. Roots are registered
        // below (`view.sync_watches()`); its receiver is drained on the fast tick.
        let (file_watcher, fs_events) = crate::warpui::file_watcher::FileWatcher::new();
        // Fast (250ms) tick: sweep expired toasts (so they self-dismiss without
        // user input) and drain filesystem change events (so external edits
        // refresh the active repo promptly). Both are cheap no-ops when idle.
        let fast_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(250));
        let fast_tick = ctx.spawn_stream_local(
            fast_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "fast", |this, vctx| {
                    this.drain_fs_events(vctx);
                    // Reload the Git Log graph when the Workspace switched while
                    // the dock is open (fs events don't fire on a checkout to a
                    // warm repo).
                    this.git_log_tick(vctx);
                    // Refresh terminal owner-tab keys so a background bell/
                    // notification is attributed to the right tab even with no
                    // user interaction.
                    this.sync_terminal_owners(&*vctx);
                    let before = this.toasts.len();
                    let now = std::time::Instant::now();
                    this.toasts
                        .retain(|t| now.duration_since(t.at) < TOAST_TTL);
                    // One final notify when a toast just expired, so the
                    // dismissal itself paints. Continuous animation (glow
                    // breathing, toast still visible) is driven by the
                    // dedicated 33ms `_anim_tick` below, not this 250ms tick.
                    if this.toasts.len() != before {
                        vctx.notify();
                    }
                });
            },
            |_this, _vctx| {},
        );
        // Browser reconcile tick — 33ms (~2 paint frames) keeps the native
        // WKWebView glued under its pane rect through splitter drags and
        // window resizes without a per-frame hook; `browser_tick` is a cheap
        // no-op scan while no Browser Pane exists.
        let browser_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(33));
        let browser_tick = ctx.spawn_stream_local(
            browser_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "browser", |this, vctx| this.browser_tick(vctx));
            },
            |_this, _vctx| {},
        );
        // Animation tick — 33ms while a toast or attention pulse is on screen.
        // The 250ms fast tick owns lifecycle (expiry, drain); this one only
        // repaints so the glow/dot breathe at full rate instead of 4 FPS.
        let anim_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(33));
        let anim_tick = ctx.spawn_stream_local(
            anim_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "anim", |this, vctx| {
                    if !this.toasts.is_empty() || this.any_attention_active() {
                        vctx.notify();
                    }
                });
            },
            |_this, _vctx| {},
        );
        let mut view = Self {
            ui_font,
            icon_font,
            mono_font,
            panes,
            layouts,
            focused,
            maximized: None,
            files_pane: restored_files_pane,
            file_pane_paths: restored_file_paths,
            file_pane_active: restored_active,
            editor_views: restored_editor_views,
            markdown_views: HashMap::new(),
            term_cache: RefCell::new(restored_term_cache),
            last_term_snapshot: std::cell::Cell::new(None),
            term_cleared: RefCell::new(HashSet::new()),
            drag_states,
            pane_rects: Rc::new(RefCell::new(HashMap::new())),
            drop_preview: Rc::new(RefCell::new(None)),
            next_pane_id,
            worktree_tabs,
            next_tab_id,
            active_tab,
            projects,
            // Filled by the initial async git scan (kicked off at the end of
            // `new`) — never shelled out synchronously here, so startup paints
            // without waiting on `git`.
            branch: String::new(),
            commit_msg: String::new(),
            commit_focused: false,
            show_git_log: false,
            git_log_ratio: Rc::new(Cell::new(0.7)),
            git_log_drag: Rc::new(Cell::new(false)),
            git_log_frame: None,
            git_log_repo: None,
            git_log_loading: false,
            git_log_gen: 0,
            git_log_last_reload: std::time::Instant::now(),
            fs_ref_poll_last: std::time::Instant::now(),
            bg_badge_last_scan: std::time::Instant::now(),
            pending_bg_roots: std::collections::HashSet::new(),
            git_log_selected: None,
            git_log_scroll: Rc::new(Cell::new(0.0)),
            git_log_ref_filter: None,
            git_log_filter: String::new(),
            git_log_filter_active: false,
            git_log_filtered: std::cell::RefCell::new(None),
            git_log_menu: None,
            git_log_branch_prompt: None,
            git_log_fetching: false,
            last_update_check: std::time::Instant::now(),
            update_prompts: init_update_prompts,
            update_dismissed_session: None,
            manual_update_check: false,
            state_dirty: Cell::new(false),
            git_log_refs_scroll: ClippedScrollStateHandle::new(),
            settings_section: SettingsSection::Appearance,
            word_wrap_default: init_word_wrap,
            trim_on_save: init_trim,
            syntax_override: init_syntax_override,
            settings_scroll: ClippedScrollStateHandle::new(),
            tree_drag: None,
            tree_drag_pos: Rc::new(Cell::new(None)),
            tree_zones: Rc::new(std::cell::RefCell::new(Vec::new())),
            tree_zones_last: Rc::new(std::cell::RefCell::new(Vec::new())),
            tree_drag_states: std::cell::RefCell::new(HashMap::new()),
            fs_drag: None,
            fs_drag_pos: Rc::new(Cell::new(None)),
            fs_zones: Rc::new(std::cell::RefCell::new(Vec::new())),
            fs_zones_last: Rc::new(std::cell::RefCell::new(Vec::new())),
            fs_drag_states: std::cell::RefCell::new(HashMap::new()),
            file_ops: Vec::new(),
            git_log_hover: Rc::new(Cell::new(None)),
            git_log_detail: None,
            git_log_detail_loading: false,
            git_log_detail_scroll: 0,
            git_log_detail_file: 0,
            active_cwd,
            left_ratio: Rc::new(Cell::new(0.18)),
            left_drag: Rc::new(Cell::new(false)),
            right_ratio: Rc::new(Cell::new(0.80)),
            right_drag: Rc::new(Cell::new(false)),
            selected,
            show_left,
            show_right,
            files_tab,
            expanded_dirs: HashSet::new(),
            selected_file: None,
            file_rows,
            changes: Vec::new(),
            expanded_projects,
            expanded_worktrees,
            added_projects: init_added,
            removed_project_paths: init_removed,
            project_tints: init_tints,
            menu_hover: RefCell::new(HashMap::new()),
            title_debounce: RefCell::new(HashMap::new()),
            context_menu: None,
            hover_tip: None,
            collapsed_groups: HashSet::new(),
            collapsed_change_dirs: HashSet::new(),
            commit_error: None,
            ahead_behind: None,
            file_status: HashMap::new(),
            dirty_dirs: HashSet::new(),
            git_op: Arc::new(Mutex::new(crate::warpui::git::OpStatus::default())),
            git_wake,
            _git_repaint: git_repaint,
            ui_wake: ui_wake.clone(),
            _ui_repaint: ui_repaint,
            row_menu: None,
            pending_new_entry: None,
            pending_delete: None,
            branch_picker: None,
            branch_list: Vec::new(),
            right_scroll: ClippedScrollStateHandle::new(),
            branch_scroll: ClippedScrollStateHandle::new(),
            worktree_menu: None,
            tab_menu: None,
            folder_menu: None,
            new_pane_menu_open: false,
            group_tints: init_group_tints,
            renaming: None,
            worktree_names: init_wt_names,
            worktree_tints: init_wt_tints,
            tab_tints: init_tab_tints,
            last_wt_click: None,
            last_tab_click: None,
            modal: None,
            remove_wt_info: None,
            confirmed_quit: false,
            modal_scroll: ClippedScrollStateHandle::new(),
            find_in_files: None,
            find_scroll: ClippedScrollStateHandle::new(),
            tab_switcher: None,
            switcher_scroll: ClippedScrollStateHandle::new(),
            switch_branch: None,
            switch_branch_scroll: ClippedScrollStateHandle::new(),
            new_workspace: None,
            worktree_poll_sig: HashMap::new(),
            git_scan_gen: HashMap::new(),
            _worktree_tick: worktree_tick,
            lsp: crate::lsp::LspManager::new(),
            lsp_wake: ui_wake,
            lsp_configs: crate::lsp::LanguageConfigs::default(),
            lsp_versions: HashMap::new(),
            lsp_diag_sig: HashMap::new(),
            pending_gotos: Vec::new(),
            _lsp_tick: lsp_tick,
            lsp_enabled: init_lsp_enabled,
            format_on_save: init_format_on_save,
            file_watcher,
            fs_events,
            watched: HashSet::new(),
            toasts: VecDeque::new(),
            next_toast_id: 0,
            _fast_tick: fast_tick,
            _browser_tick: browser_tick,
            _anim_tick: anim_tick,
            browser_host: crate::warpui::browser::BrowserHost::new(),
        };
        // Register every restored Project + Workspace root with the watcher so
        // external / agent edits refresh the active repo from first paint.
        view.sync_watches();
        // Backfill branch labels + diff/dirty badges for the whole (shallow-
        // loaded) tree off the UI thread. The sidebar is already visible; badges
        // stream in as this returns. ZERO synchronous `git` ran on this path.
        let scan_paths = crate::warpui::projects::scan_paths(&view.projects);
        view.spawn_git_scan(ctx, "tree".to_string(), scan_paths);
        view
    }

    /// Quit guard for the OS terminate / close-window hooks (wired in
    /// `mod.rs::run` via `AppCallbacks::on_should_terminate_app` /
    /// `on_should_close_window`). Returns `true` if the app may terminate now;
    /// `false` to CANCEL termination and raise the ConfirmQuit modal because a
    /// terminal has a live foreground process. Once the user confirms via the
    /// modal (`QuitConfirmed`), `confirmed_quit` is set so the re-issued
    /// terminate returns `true` immediately.
    pub fn approve_terminate(&mut self, vctx: &mut ViewContext<Self>) -> bool {
        if self.confirmed_quit {
            return true;
        }
        if self.count_running_terminals(vctx) > 0 {
            self.modal = Some(Modal::ConfirmQuit);
            vctx.notify();
            false
        } else {
            true
        }
    }

    /// Count terminal panes whose foreground program (alt-screen TUI) is live.
    /// Drives the quit / close-pane confirmation copy. Port of old egui
    /// `confirm_quit::count_running_terminals`.
    fn count_running_terminals(&self, app: &AppContext) -> usize {
        self.panes
            .values()
            .filter(|pc| {
                matches!(pc, PaneContent::Terminal(h) if h.as_ref(app).has_foreground_process())
            })
            .count()
    }

    /// Spawn a new persistent terminal view rooted at `path`. Each gets its own
    /// PTY + repaint waker; it is never respawned (history retained).
    fn spawn_terminal(
        ctx: &mut ViewContext<Self>,
        path: PathBuf,
        shell_wake: Arc<dyn Fn() + Send + Sync>,
    ) -> ViewHandle<TerminalView> {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: crate::warpui::controller::Wake = std::sync::Arc::new(move || {
            let _ = tx.try_send(());
            // Also repaint the shell so the tab label tracks the terminal's live
            // OSC-0/2 title (the shell renders the label, and its own view is a
            // separate entity from the TerminalView the PTY byte woke).
            shell_wake();
        });
        let cwd = Rc::new(RefCell::new(Some(path)));
        ctx.add_view(move |ctx| TerminalView::new_with(ctx, cwd, wake, rx))
    }

    /// Rebuild the cached right-panel contents for the active tab. Called from
    /// `handle_action` (never from render) so the FS walk / `git` shell-out
    /// happens once per change, not every repaint.
    /// Refresh the terminal snapshot cache from the live views. Expensive
    /// (renders every terminal's scrollback to ANSI), so callers only invoke it
    /// on "heavy" actions, not per keystroke.
    fn refresh_term_cache(&self, app: &AppContext) {
        // Debounce: at most once every 400ms so per-keystroke saves stay cheap
        // while still capturing recent command output.
        let now = std::time::Instant::now();
        if let Some(last) = self.last_term_snapshot.get() {
            if now.duration_since(last) < std::time::Duration::from_millis(400) {
                return;
            }
        }
        self.last_term_snapshot.set(Some(now));
        let cleared = self.term_cleared.borrow().clone();
        let mut cache = self.term_cache.borrow_mut();
        // Drop snapshots for panes that no longer exist.
        cache.retain(|id, _| self.panes.contains_key(id));
        for (id, pc) in self.panes.iter() {
            if let PaneContent::Terminal(h) = pc {
                let view = h.as_ref(app);
                let cwd = view.cwd();
                let hist = view.snapshot();
                // Never let a terminal's persisted history SHRINK (>64 bytes)
                // unless it was explicitly cleared (Cmd+K). This keeps a restored
                // session sticky: replaying the saved scrollback then re-snapshotting
                // can yield slightly less, which would otherwise erode the history
                // across generations until it's empty.
                let keep_old = !cleared.contains(id)
                    && cache
                        .get(id)
                        .is_some_and(|prev| hist.len() + 64 < prev.history.len());
                let history = if keep_old {
                    cache.get(id).map(|p| p.history.clone()).unwrap_or(hist)
                } else {
                    hist
                };
                cache.insert(*id, crate::warpui::persist::STerminal { cwd, history });
            }
        }
        self.term_cleared.borrow_mut().clear();
    }

    /// Snapshot the persistable UI state and write it to ~/.crane/warpui-state.json.
    /// `refresh_terminals` re-snapshots terminal scrollback first (skip on keystrokes).
    fn save_state(&self, app: &AppContext) {
        use crate::warpui::persist::{save, SNode, STab, WarpuiState};
        // Always attempt a terminal refresh; it self-debounces (400ms) so this
        // is cheap even when called on every keystroke.
        self.refresh_term_cache(app);
        let terminals: Vec<(PaneId, crate::warpui::persist::STerminal)> = self
            .term_cache
            .borrow()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let browsers: Vec<(PaneId, crate::warpui::persist::SBrowser)> = self
            .panes
            .iter()
            .filter_map(|(id, pc)| match pc {
                PaneContent::Browser(h) => {
                    let (tabs, active) = h.as_ref(app).persist_tabs();
                    Some((*id, crate::warpui::persist::SBrowser { tabs, active }))
                }
                _ => None,
            })
            .collect();
        let worktree_tabs = self
            .worktree_tabs
            .iter()
            .map(|(k, tabs)| {
                let stabs = tabs
                    .iter()
                    .filter_map(|t| {
                        let node = self.layouts.get(&(k.0, k.1, t.id))?;
                        let snode = SNode::from_node(node);
                        // Only record the focused pane if it is a leaf of THIS tab.
                        let focus = self.focused.filter(|&pid| {
                            let mut leaves = Vec::new();
                            snode.leaves(&mut leaves);
                            leaves.contains(&pid)
                        });
                        Some(STab {
                            id: t.id,
                            name: t.name.clone(),
                            layout: snode,
                            focus,
                            renamed: t.renamed,
                        })
                    })
                    .collect::<Vec<_>>();
                (*k, stabs)
            })
            .collect();
        // Read current window size via AppContext so it can be restored on next launch.
        let (window_w, window_h) = app
            .window_ids()
            .into_iter()
            .next()
            .and_then(|id| app.window_bounds(&id))
            .map(|r| (r.size().x(), r.size().y()))
            .unwrap_or((0.0, 0.0));
        save(&WarpuiState {
            show_left: self.show_left,
            show_right: self.show_right,
            files_tab: self.files_tab,
            active_tab: self.active_tab,
            expanded_projects: self.expanded_projects.iter().copied().collect(),
            browsers,
            expanded_worktrees: self.expanded_worktrees.iter().copied().collect(),
            worktree_tabs,
            next_tab_id: self.next_tab_id,
            next_pane_id: self.next_pane_id,
            files_pane: self.files_pane,
            file_pane_paths: self.file_pane_paths.clone(),
            file_pane_active: self.file_pane_active,
            terminals,
            window_w,
            window_h,
            theme_name: crate::theme::current().name.clone(),
            zoom_level: crate::warpui::fontsize::zoom_level(),
            added_projects: self.added_projects.clone(),
            removed_project_paths: self.removed_project_paths.clone(),
            project_tints: self.project_tints.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            worktree_names: self
                .worktree_names
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            worktree_tints: self
                .worktree_tints
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            tab_tints: self
                .tab_tints
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            group_tints: self
                .group_tints
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            lsp_enabled: self.lsp_enabled,
            format_on_save: self.format_on_save,
            terminal_font: crate::warpui::fontsize::base(),
            editor_font: crate::warpui::fontsize::editor(),
            word_wrap: self.word_wrap_default,
            trim_on_save: self.trim_on_save,
            syntax_override: self.syntax_override.clone().unwrap_or_default(),
            sidebar_order: self.order_snapshot(),
            update_prompts: self
                .update_prompts
                .iter()
                .map(|(v, p)| {
                    let s = match p {
                        UpdatePrompt::Dismissed => "dismissed".to_string(),
                        UpdatePrompt::RemindAt(t) => format!("remind:{t}"),
                    };
                    (v.clone(), s)
                })
                .collect(),
        });
    }

    /// Resolve the tint color for the project at `idx`: uses the user-chosen tint
    /// stored on the `ProjectNode` if present, otherwise the palette default.
    fn project_color_for(&self, idx: usize) -> ColorU {
        if let Some(p) = self.projects.get(idx) {
            if let Some([r, g, b]) = p.tint {
                return ColorU::new(r, g, b, 255);
            }
        }
        // Untinted projects use the single theme accent for the leading icon —
        // matching old egui `projects.rs` (`tint_color.unwrap_or_else(accent)`),
        // NOT a per-index rainbow.
        theme::accent()
    }

    // ---- Folder groups + attention pulse ---------------------------------

    /// Number of projects that belong to the folder group `group`.
    fn group_member_count(&self, group: &str) -> usize {
        self.projects
            .iter()
            .filter(|p| p.group_path.as_deref() == Some(group))
            .count()
    }

    /// True when the project at `pi` is one of MULTIPLE members of a folder
    /// group. Such projects hide their individual "Remove Project" menu item —
    /// the group is removed atomically via the folder header instead.
    fn in_multi_group(&self, pi: usize) -> bool {
        self.projects
            .get(pi)
            .and_then(|p| p.group_path.as_deref())
            .is_some_and(|g| self.group_member_count(g) > 1)
    }

    /// Remove EVERY member project of `group` atomically. Members are resolved
    /// by PATH and removed one at a time (each `remove_project_at` shifts the
    /// remaining indices, so we re-scan for the next member each pass).
    fn remove_group(&mut self, group: &str, ctx: &mut ViewContext<Self>) {
        self.group_tints.remove(group);
        self.collapsed_groups.remove(group);
        loop {
            let Some(idx) = self
                .projects
                .iter()
                .position(|p| p.group_path.as_deref() == Some(group))
            else {
                break;
            };
            self.remove_project_at(idx, ctx);
        }
    }

    /// Resolve the folder-group tint (icon + label) for `group`, or the muted
    /// default when the user hasn't set one. Mirrors the project tint fallback.
    fn group_color_for(&self, group: &str) -> ColorU {
        match self.group_tints.get(group) {
            Some([r, g, b]) => ColorU::new(*r, *g, *b, 255),
            None => theme::text_muted(),
        }
    }

    /// Latch attention on `key`'s tab so the Left Panel pulses. No-op when the
    /// tab is already the active one (never nag about the surface in view) or
    /// when it already has a pending timestamp (keep the first ping).
    fn flag_attention(&mut self, key: Option<(usize, usize, usize)>) {
        let Some((pi, wi, tid)) = key else { return };
        if self.active_tab == Some((pi, wi, tid)) {
            return;
        }
        if let Some(tabs) = self.worktree_tabs.get_mut(&(pi, wi)) {
            if let Some(t) = tabs.iter_mut().find(|t| t.id == tid) {
                if t.attention_since.is_none() {
                    t.attention_since = Some(std::time::Instant::now());
                }
            }
        }
    }

    /// Clear the pending-attention flag on the active tab. Called after every
    /// action so any activation path (click, shortcut, toast-focus) settles the
    /// pulse without threading a setter through each one (old egui parity).
    fn clear_active_attention(&mut self) {
        let Some((pi, wi, tid)) = self.active_tab else { return };
        if let Some(tabs) = self.worktree_tabs.get_mut(&(pi, wi)) {
            if let Some(t) = tabs.iter_mut().find(|t| t.id == tid) {
                t.attention_since = None;
            }
        }
    }

    /// Freshest pending-attention timestamp among a worktree's tabs.
    fn worktree_attention(&self, pi: usize, wi: usize) -> Option<std::time::Instant> {
        self.worktree_tabs
            .get(&(pi, wi))
            .into_iter()
            .flatten()
            .filter_map(|t| t.attention_since)
            .max()
    }

    /// Freshest pending-attention timestamp among ALL tabs of a project.
    fn project_attention(&self, pi: usize) -> Option<std::time::Instant> {
        let n = self.projects.get(pi).map(|p| p.worktrees.len()).unwrap_or(0);
        (0..n).filter_map(|wi| self.worktree_attention(pi, wi)).max()
    }

    /// Freshest pending-attention timestamp among every project in `group`.
    fn group_attention(&self, group: &str) -> Option<std::time::Instant> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, p)| p.group_path.as_deref() == Some(group))
            .filter_map(|(pi, _)| self.project_attention(pi))
            .max()
    }

    /// True while ANY tab still has pending attention — keeps the pulse repaint
    /// stream alive (33ms anim tick) so the glow breathes + decays without input.
    fn any_attention_active(&self) -> bool {
        self.worktree_tabs
            .values()
            .flatten()
            .any(|t| t.attention_since.is_some())
    }

    /// 0..1 breathing intensity for a pending-attention `since`, or None when
    /// there is no pending notification. Smooth raised-cosine that loops every
    /// ~2.6s until the tab is opened (port of old egui `AttentionViz`).
    fn attention_glow(since: Option<std::time::Instant>) -> Option<f32> {
        let e = since?.elapsed().as_secs_f32();
        let phase = (e / 2.6) * std::f32::consts::TAU;
        Some((1.0 - phase.cos()) * 0.5)
    }

    /// A small accent unread-dot, drawn as a rounded (circular) Rect — NOT a
    /// font glyph (bundled fonts lack a dot codepoint). Alpha tracks the glow.
    fn attention_dot(glow: f32) -> Box<dyn Element> {
        let a = (110.0 + 145.0 * glow).clamp(0.0, 255.0) as u8;
        let base = theme::accent();
        let dot = ColorU::new(base.r, base.g, base.b, a);
        Container::new(
            ConstrainedBox::new(
                Rect::new()
                    .with_background_color(dot)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
            )
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
        )
        .with_padding_left(6.0)
        .with_padding_top(4.0)
        .finish()
    }

    /// Wrap a built left-tree `row` with the attention pulse when `since` is set:
    /// a low-alpha accent glow layered BEHIND the row plus a right-pinned unread
    /// dot. The row is kept the TOPMOST child so its transparent hit layer still
    /// receives clicks (the glow/dot are non-interactive layers beneath it, and
    /// the row's non-selected background is transparent so both show through).
    fn attention_wrap(
        &self,
        row: Box<dyn Element>,
        since: Option<std::time::Instant>,
        row_h: f32,
    ) -> Box<dyn Element> {
        let Some(glow) = Self::attention_glow(since) else {
            return row;
        };
        let base = theme::accent();
        let ga = (30.0 * glow).clamp(0.0, 255.0) as u8;
        let glow_bg = ConstrainedBox::new(
            Rect::new()
                .with_background_color(ColorU::new(base.r, base.g, base.b, ga))
                .finish(),
        )
        .with_height(row_h)
        .finish();
        // Full-width layer that pushes the dot to the right edge; sits beneath the
        // row so it never steals the row's click.
        let dot_layer = Container::new(
            Flex::row()
                .with_child(
                    Expanded::new(
                        1.0,
                        Container::new(Text::new("", self.ui_font, 10.0).finish()).finish(),
                    )
                    .finish(),
                )
                .with_child(Self::attention_dot(glow))
                .finish(),
        )
        .with_padding_right(8.0)
        .finish();
        Stack::new()
            .with_child(glow_bg)
            .with_child(dot_layer)
            .with_child(row)
            .finish()
    }

    /// Sync each live terminal's `owner_key` from the authoritative `layouts`
    /// map so a notification/bell it dispatches names the right source tab.
    /// O(panes). Called ONLY from the shell's fast tick — NOT from `handle_action`:
    /// a terminal-dispatched notification runs `handle_action` synchronously while
    /// that terminal is taken out of the view map, and reading it there would
    /// panic ("circular view reference"). The fast tick is the shell's own stream,
    /// so every terminal is safely in the map when this reads them.
    fn sync_terminal_owners(&self, ctx: &AppContext) {
        for (key, node) in self.layouts.iter() {
            let mut leaves = Vec::new();
            node.leaves(&mut leaves);
            for id in leaves {
                if let Some(h) = self.terminal_at(id) {
                    let cell = h.read(ctx, |v, _| v.owner_cell());
                    cell.set(Some(*key));
                }
            }
        }
    }

    /// Remove the project at position `i` (context-menu Remove, or one member
    /// of a folder-group removal). Extracted from the `RemoveProject` arm so
    /// `remove_group` can call it per member. Rekeys all (pi,*)-keyed maps by
    /// PATH after the in-place `Vec::remove` (indices shift). Runs ZERO git.
    // ── Sidebar drag-drop reorder (old state.rs reorder_* ported 1:1) ──────

    /// `new_index` is computed from the *pre-removal* list; removing the
    /// source first shifts later positions down by one, so a downward drag
    /// lands at `new_index - 1` (old `move_in_vec`).
    fn move_in_vec<T>(vec: &mut Vec<T>, pos: usize, new_index: usize) {
        let target = if pos < new_index { new_index - 1 } else { new_index };
        let target = target.min(vec.len().saturating_sub(1));
        if pos != target {
            let item = vec.remove(pos);
            vec.insert(target, item);
        }
    }

    /// Root-level blocks: a standalone project, or a maximal contiguous run
    /// sharing one `group_path`. Root reordering moves blocks atomically.
    fn root_blocks(&self) -> Vec<std::ops::Range<usize>> {
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < self.projects.len() {
            let start = i;
            match self.projects[i].group_path.clone() {
                None => i += 1,
                Some(gp) => {
                    while i < self.projects.len()
                        && self.projects[i].group_path.as_ref() == Some(&gp)
                    {
                        i += 1;
                    }
                }
            }
            blocks.push(start..i);
        }
        blocks
    }

    /// Move root block `src_block_idx` to pre-removal slot `new_block_index`.
    fn move_block(&mut self, src_block_idx: usize, new_block_index: usize) {
        let blocks = self.root_blocks();
        if src_block_idx >= blocks.len() {
            return;
        }
        let target = new_block_index.min(blocks.len());
        if target == src_block_idx || target == src_block_idx + 1 {
            return;
        }
        let mut order: Vec<usize> = (0..blocks.len()).collect();
        let item = order.remove(src_block_idx);
        let insert_at = if target > src_block_idx { target - 1 } else { target };
        order.insert(insert_at, item);
        let n = self.projects.len();
        let old = std::mem::take(&mut self.projects);
        let mut taken: Vec<Option<crate::warpui::projects::ProjectNode>> =
            old.into_iter().map(Some).collect();
        let mut new_projects = Vec::with_capacity(n);
        for &b_idx in &order {
            for i in blocks[b_idx].clone() {
                if let Some(p) = taken[i].take() {
                    new_projects.push(p);
                }
            }
        }
        self.projects = new_projects;
    }

    /// Cluster each group's members contiguously at the group's first slot —
    /// without this a single `group_path` can paint as multiple FOLDER
    /// headers (the walk emits one per adjacent-row flip).
    fn consolidate_groups(&mut self) {
        let n = self.projects.len();
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut placed: HashSet<String> = HashSet::new();
        for i in 0..n {
            match &self.projects[i].group_path {
                None => order.push(i),
                Some(gp) => {
                    if placed.contains(gp) {
                        continue;
                    }
                    placed.insert(gp.clone());
                    let gp = gp.clone();
                    for j in 0..n {
                        if self.projects[j].group_path.as_ref() == Some(&gp) {
                            order.push(j);
                        }
                    }
                }
            }
        }
        if order.len() != n {
            return;
        }
        let mut slots: Vec<Option<crate::warpui::projects::ProjectNode>> =
            std::mem::take(&mut self.projects).into_iter().map(Some).collect();
        let mut reordered = Vec::with_capacity(n);
        for idx in order {
            if let Some(p) = slots[idx].take() {
                reordered.push(p);
            }
        }
        self.projects = reordered;
    }

    /// Snapshot (project path, worktree paths) — taken before a reorder so
    /// `rekey_after_reorder` can map old (pi, wi) keys to the new positions.
    fn order_snapshot(&self) -> Vec<(String, Vec<String>)> {
        self.projects
            .iter()
            .map(|p| {
                (
                    p.path.clone(),
                    p.worktrees.iter().map(|w| w.path.clone()).collect(),
                )
            })
            .collect()
    }

    /// Rekey every (pi, wi, …)-indexed structure after `self.projects` (or a
    /// project's worktrees) were REORDERED in place. A reorder never removes
    /// anything, so this is a pure rekey — no teardown.
    fn rekey_after_reorder(&mut self, old: &[(String, Vec<String>)]) {
        let new_pi: HashMap<&str, usize> = self
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (p.path.as_str(), i))
            .collect();
        let new_wi: HashMap<(&str, &str), usize> = self
            .projects
            .iter()
            .flat_map(|p| {
                p.worktrees
                    .iter()
                    .enumerate()
                    .map(move |(wi, w)| ((p.path.as_str(), w.path.as_str()), wi))
            })
            .collect();
        let remap = |pi: usize, wi: usize| -> Option<(usize, usize)> {
            let (ppath, wts) = old.get(pi)?;
            let np = *new_pi.get(ppath.as_str())?;
            let wpath = wts.get(wi)?;
            let nw = *new_wi.get(&(ppath.as_str(), wpath.as_str()))?;
            Some((np, nw))
        };
        let old_layouts = std::mem::take(&mut self.layouts);
        for ((pi, wi, tid), node) in old_layouts {
            if let Some((np, nw)) = remap(pi, wi) {
                self.layouts.insert((np, nw, tid), node);
            }
        }
        let old_tabs = std::mem::take(&mut self.worktree_tabs);
        for ((pi, wi), tabs) in old_tabs {
            if let Some((np, nw)) = remap(pi, wi) {
                self.worktree_tabs.insert((np, nw), tabs);
            }
        }
        if let Some((pi, wi, tid)) = self.active_tab {
            if let Some((np, nw)) = remap(pi, wi) {
                self.active_tab = Some((np, nw, tid));
            }
        }
        {
            let (pi, wi, tid) = self.selected;
            if let Some((np, nw)) = remap(pi, wi) {
                self.selected = (np, nw, tid);
            }
        }
        self.expanded_projects = self
            .expanded_projects
            .iter()
            .filter_map(|pi| {
                old.get(*pi)
                    .and_then(|(p, _)| new_pi.get(p.as_str()))
                    .copied()
            })
            .collect();
        self.expanded_worktrees = self
            .expanded_worktrees
            .iter()
            .filter_map(|(pi, wi)| remap(*pi, *wi))
            .collect();
    }

    /// Apply a completed sidebar drop: filter the painted zones to siblings
    /// of the dragged row, require the release Y inside the sibling span
    /// (±8px pad), count sibling centers at-or-above the release, and route
    /// to the matching reorder (old `projects.rs` global drop dispatch).
    fn apply_tree_drop(&mut self, cursor: warpui::geometry::vector::Vector2F, ctx: &mut ViewContext<Self>) {
        let Some(drag) = self.tree_drag.take() else {
            return;
        };
        self.tree_drag_pos.set(None);
        let zones = self.tree_zones.borrow().clone();
        let candidates: Vec<&(warpui::geometry::rect::RectF, TreeScope)> = zones
            .iter()
            .filter(|(_, scope)| match (&drag, scope) {
                (TreeDrag::Project { group: None, .. }, TreeScope::Root) => true,
                (TreeDrag::Group { .. }, TreeScope::Root) => true,
                (
                    TreeDrag::Project { group: Some(src), .. },
                    TreeScope::InBlock { group },
                ) => src == group,
                (
                    TreeDrag::Worktree { project, .. },
                    TreeScope::Worktree { project: zp },
                ) => project == zp,
                (
                    TreeDrag::Tab { project, worktree, .. },
                    TreeScope::Tab { project: zp, worktree: zw },
                ) => project == zp && worktree == zw,
                _ => false,
            })
            .collect();
        if candidates.is_empty() {
            ctx.notify();
            return;
        }
        let pad = 8.0;
        let first_y = candidates.first().unwrap().0.origin().y() - pad;
        let last_y = candidates.last().unwrap().0.max_y() + pad;
        if cursor.y() < first_y || cursor.y() > last_y {
            ctx.notify();
            return;
        }
        let new_index = candidates
            .iter()
            .filter(|(r, _)| r.origin().y() + r.height() / 2.0 <= cursor.y())
            .count();

        let snapshot = self.order_snapshot();
        match drag {
            TreeDrag::Project { path, group: None } => {
                let Some(pos) = self.projects.iter().position(|p| p.path == path) else {
                    return;
                };
                if self.projects[pos].group_path.is_some() {
                    return;
                }
                let blocks = self.root_blocks();
                let Some(src) = blocks.iter().position(|b| b.contains(&pos)) else {
                    return;
                };
                self.move_block(src, new_index);
                self.consolidate_groups();
            }
            TreeDrag::Group { path } => {
                let Some(pos) = self
                    .projects
                    .iter()
                    .position(|p| p.group_path.as_deref() == Some(path.as_str()))
                else {
                    return;
                };
                let blocks = self.root_blocks();
                let Some(src) = blocks.iter().position(|b| b.contains(&pos)) else {
                    return;
                };
                self.move_block(src, new_index);
                self.consolidate_groups();
            }
            TreeDrag::Project { path, group: Some(group) } => {
                let Some(pos) = self.projects.iter().position(|p| p.path == path) else {
                    return;
                };
                let blocks = self.root_blocks();
                let Some(block) = blocks
                    .iter()
                    .find(|b| {
                        b.contains(&pos)
                            && self.projects[b.start].group_path.as_deref()
                                == Some(group.as_str())
                    })
                    .cloned()
                else {
                    return;
                };
                let block_len = block.end - block.start;
                let to = block.start + new_index.min(block_len);
                Self::move_in_vec(&mut self.projects, pos, to);
            }
            TreeDrag::Worktree { project, path } => {
                let Some(p) = self.projects.iter_mut().find(|p| p.path == project) else {
                    return;
                };
                if let Some(pos) = p.worktrees.iter().position(|w| w.path == path) {
                    Self::move_in_vec(&mut p.worktrees, pos, new_index);
                }
            }
            TreeDrag::Tab { project, worktree, id } => {
                let key = self
                    .projects
                    .iter()
                    .position(|p| p.path == project)
                    .and_then(|pi| {
                        self.projects[pi]
                            .worktrees
                            .iter()
                            .position(|w| w.path == worktree)
                            .map(|wi| (pi, wi))
                    });
                if let Some(key) = key {
                    if let Some(tabs) = self.worktree_tabs.get_mut(&key) {
                        if let Some(pos) = tabs.iter().position(|t| t.id == id) {
                            Self::move_in_vec(tabs, pos, new_index);
                        }
                    }
                }
                // Tab reorder shifts no (pi, wi) keys — skip the rekey.
                self.save_state(&*ctx);
                ctx.notify();
                return;
            }
        }
        self.rekey_after_reorder(&snapshot);
        self.save_state(&*ctx);
        ctx.notify();
    }

    /// The directory a Files-tree drop at `cursor` targets: the deepest dir
    /// row under the cursor, else the tree root when the cursor is anywhere
    /// over the Files list. Zones repopulate per paint; a row's rect can only
    /// contain the cursor once, so "last containing zone" (deepest painted)
    /// is the row, with the whole-list root zone painted first as fallback.
    fn fs_drop_target(&self, cursor: warpui::geometry::vector::Vector2F) -> Option<PathBuf> {
        self.fs_zones
            .borrow()
            .iter()
            .filter(|(r, _)| r.contains_point(cursor))
            .last()
            .map(|(_, p)| p.clone())
    }

    /// `name`, or `name (2)`, `name (3)` … — the first non-colliding target in
    /// `dir` (old explorer.rs copy de-dupe naming, applied to moves too so a
    /// move never overwrites).
    fn unique_target(dir: &std::path::Path, name: &std::ffi::OsStr) -> PathBuf {
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        let base = std::path::Path::new(name);
        let stem = base
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string_lossy().into_owned());
        let ext = base.extension().map(|e| e.to_string_lossy().into_owned());
        for n in 2.. {
            let file = match &ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            };
            let candidate = dir.join(file);
            if !candidate.exists() {
                return candidate;
            }
        }
        unreachable!()
    }

    /// Complete an internal Files-tree drag: move `src` into the dir under the
    /// release cursor (never overwriting — de-dupe naming), push the op onto
    /// the undo stack, refresh the panel. No-ops: dropping onto the source's
    /// own parent, onto itself, or a dir into its own subtree.
    fn apply_fs_drop(&mut self, cursor: warpui::geometry::vector::Vector2F, ctx: &mut ViewContext<Self>) {
        let Some(src) = self.fs_drag.take() else {
            return;
        };
        self.fs_drag_pos.set(None);
        let Some(dst_dir) = self.fs_drop_target(cursor) else {
            ctx.notify();
            return;
        };
        if src.parent() == Some(dst_dir.as_path())
            || src == dst_dir
            || dst_dir.starts_with(&src)
        {
            ctx.notify();
            return;
        }
        let Some(name) = src.file_name() else {
            return;
        };
        let target = Self::unique_target(&dst_dir, name);
        match std::fs::rename(&src, &target) {
            Ok(()) => {
                self.file_ops.push(FileOp::Move {
                    from: src.clone(),
                    to: target.clone(),
                });
                // A moved open file keeps its editor tab pointing at the old
                // path — retarget the File Tab list entry so re-clicks work.
                for p in self.file_pane_paths.iter_mut() {
                    if *p == src {
                        *p = target.clone();
                    }
                }
                if self.selected_file.as_deref() == Some(src.as_path()) {
                    self.selected_file = Some(target);
                }
            }
            Err(e) => self.commit_error = Some(format!("Move: {e}")),
        }
        self.refresh_panel(ctx);
        ctx.notify();
    }

    /// Copy OS-dropped files/folders (Finder → Crane) into the dir under the
    /// drop point (or the tree root), de-dupe named, each push an undoable
    /// Copy op. Directories copy recursively.
    fn apply_fs_external_drop(
        &mut self,
        paths: Vec<String>,
        cursor: warpui::geometry::vector::Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(dst_dir) = self
            .fs_drop_target(cursor)
            .or_else(|| self.active_cwd.clone())
        else {
            return;
        };
        for p in paths {
            let src = PathBuf::from(&p);
            let Some(name) = src.file_name() else { continue };
            // Dropping something already inside the tree at the same spot
            // would just clone it next to itself — skip the degenerate case.
            if src.parent() == Some(dst_dir.as_path()) {
                continue;
            }
            let target = Self::unique_target(&dst_dir, name);
            let res = if src.is_dir() {
                Self::copy_dir_recursive(&src, &target)
            } else {
                std::fs::copy(&src, &target).map(|_| ())
            };
            match res {
                Ok(()) => self.file_ops.push(FileOp::Copy {
                    created: target.clone(),
                }),
                Err(e) => {
                    self.commit_error = Some(format!("Copy: {e}"));
                    break;
                }
            }
        }
        self.refresh_panel(ctx);
        ctx.notify();
    }

    fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if from.is_dir() {
                Self::copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }

    /// Cmd+Z fallback when no editor owns focus: undo the last Files-tree op.
    /// A Move renames back; a Copy sends the created path to the Trash
    /// (recoverable — never a permanent unlink).
    fn undo_file_op(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let Some(op) = self.file_ops.pop() else {
            return false;
        };
        match op {
            FileOp::Move { from, to } => {
                if let Err(e) = std::fs::rename(&to, &from) {
                    self.commit_error = Some(format!("Undo move: {e}"));
                } else {
                    for p in self.file_pane_paths.iter_mut() {
                        if *p == to {
                            *p = from.clone();
                        }
                    }
                    if self.selected_file.as_deref() == Some(to.as_path()) {
                        self.selected_file = Some(from);
                    }
                }
            }
            FileOp::Copy { created } => {
                if let Err(e) = trash::delete(&created) {
                    self.commit_error = Some(format!("Undo copy: {e}"));
                }
            }
        }
        self.refresh_panel(ctx);
        ctx.notify();
        true
    }

    /// Wrap a sidebar row in a `Draggable` carrying its `TreeDrag` identity —
    /// clicks pass through (drag engages past the movement threshold); the
    /// drop dispatches `TreeDrop` with the release cursor.
    fn tree_draggable(
        &self,
        key: String,
        drag: TreeDrag,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        let state = self
            .tree_drag_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .clone();
        let pos_cell = self.tree_drag_pos.clone();
        let pos_cell_drop = self.tree_drag_pos.clone();
        let drag_start = drag.clone();
        let state_drag = state.clone();
        let state_drop = state.clone();
        Box::new(
            Draggable::new(state, child)
                .on_drag_start(move |ctx, _app, _rect| {
                    ctx.dispatch_typed_action(CraneShellAction::TreeDragStart(
                        drag_start.clone(),
                    ));
                })
                .on_drag(move |_ctx, _app, rect, _data| {
                    let off = state_drag
                        .cursor_offset_within_element()
                        .unwrap_or_else(|| vec2f(0.0, 0.0));
                    pos_cell.set(Some(rect.origin() + off));
                })
                .on_drop(move |ctx, _app, rect, _data| {
                    let off = state_drop
                        .cursor_offset_within_element()
                        .unwrap_or_else(|| vec2f(0.0, 0.0));
                    let cursor = rect.origin() + off;
                    pos_cell_drop.set(None);
                    ctx.dispatch_typed_action(CraneShellAction::TreeDrop {
                        x: cursor.x(),
                        y: cursor.y(),
                    });
                }),
        )
    }

    /// Wrap a sidebar row in a `ZoneProbe` recording its rect + drop scope.
    fn tree_zone(&self, scope: TreeScope, child: Box<dyn Element>) -> Box<dyn Element> {
        Box::new(crate::warpui::rect_probe::ZoneProbe::new(
            child,
            self.tree_zones.clone(),
            scope,
        ))
    }

    /// The 2px accent drop-line painted between sibling rows while a sidebar
    /// drag is in flight (old `paint_drop_line`): the gap nearest the cursor
    /// among scope-matching rows.
    fn tree_drop_line_overlay(&self) -> Option<Box<dyn Element>> {
        let drag = self.tree_drag.as_ref()?;
        let cursor = self.tree_drag_pos.get()?;
        let zones = self.tree_zones_last.borrow();
        let candidates: Vec<&(warpui::geometry::rect::RectF, TreeScope)> = zones
            .iter()
            .filter(|(_, scope)| match (drag, scope) {
                (TreeDrag::Project { group: None, .. }, TreeScope::Root) => true,
                (TreeDrag::Group { .. }, TreeScope::Root) => true,
                (
                    TreeDrag::Project { group: Some(src), .. },
                    TreeScope::InBlock { group },
                ) => src == group,
                (TreeDrag::Worktree { project, .. }, TreeScope::Worktree { project: zp }) => {
                    project == zp
                }
                (
                    TreeDrag::Tab { project, worktree, .. },
                    TreeScope::Tab { project: zp, worktree: zw },
                ) => project == zp && worktree == zw,
                _ => false,
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let pad = 8.0;
        let first_y = candidates.first().unwrap().0.origin().y() - pad;
        let last_y = candidates.last().unwrap().0.max_y() + pad;
        if cursor.y() < first_y || cursor.y() > last_y {
            return None;
        }
        // The line sits at the top edge of the first row whose center is
        // below the cursor, or the bottom edge of the last row.
        let below_count = candidates
            .iter()
            .filter(|(r, _)| r.origin().y() + r.height() / 2.0 <= cursor.y())
            .count();
        let (x, w, y) = if below_count == candidates.len() {
            let (r, _) = candidates.last().unwrap();
            (r.origin().x() + 6.0, r.width() - 12.0, r.max_y() - 1.0)
        } else {
            let (r, _) = candidates[below_count];
            (r.origin().x() + 6.0, r.width() - 12.0, r.origin().y())
        };
        Some(
            Container::new(
                ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
                    .with_width(w.max(20.0))
                    .with_height(2.0)
                    .finish(),
            )
            .with_padding_left(x)
            .with_padding_top(y)
            .finish(),
        )
    }

    fn remove_project_at(&mut self, i: usize, ctx: &mut ViewContext<Self>) {
        let Some(removed_path) = self.projects.get(i).map(|p| p.path.clone()) else {
            return;
        };
                self.added_projects.retain(|ap| ap.path != removed_path);
                if !self.removed_project_paths.contains(&removed_path) {
                    self.removed_project_paths.push(removed_path.clone());
                }
                // Positional-index remap: every (pi, *)-keyed map (layouts,
                // worktree_tabs, active_tab, selected, expanded_*) is keyed by a
                // project's POSITION in `self.projects`. Removing a non-last (or
                // the active) project shifts later projects' indices, so we must
                // rekey those maps to the post-removal positions and tear down the
                // vanished project's panes/PTYs. We match by PATH (robust to the
                // single-index shift).
                //
                // IMPORTANT (perf / OG parity): removal is done IN PLACE via
                // `Vec::remove`. We deliberately do NOT call `reload_projects()`
                // here — that would re-shell `git status`/`git diff` for EVERY
                // project + worktree + discovered child repo in the whole tree
                // (the reported freeze). A single-project remove must run ZERO git
                // subprocesses. `removed_project_paths` was already updated above
                // so any *future* reload (e.g. a later Add Project) still excludes
                // this path.
                let old_paths: Vec<String> =
                    self.projects.iter().map(|p| p.path.clone()).collect();
                if i < self.projects.len() {
                    self.projects.remove(i);
                }
                // Reconcile watches: unwatch the removed Project + its Workspaces.
                self.sync_watches();
                let new_index: HashMap<String, usize> = self
                    .projects
                    .iter()
                    .enumerate()
                    .map(|(ni, p)| (p.path.clone(), ni))
                    .collect();
                // old project index -> new project index (None = project gone).
                let remap = |pi: usize| -> Option<usize> {
                    old_paths.get(pi).and_then(|path| new_index.get(path).copied())
                };
                // 1) Tear down layouts (+ PTYs) for projects that vanished.
                let dead_layouts: Vec<(usize, usize, usize)> = self
                    .layouts
                    .keys()
                    .copied()
                    .filter(|(pi, _, _)| remap(*pi).is_none())
                    .collect();
                for key in dead_layouts {
                    self.tear_down_layout(key);
                }
                // 2) Rekey the surviving layouts to their new project indices.
                let old_layouts = std::mem::take(&mut self.layouts);
                for ((pi, wi, tid), node) in old_layouts {
                    if let Some(np) = remap(pi) {
                        self.layouts.insert((np, wi, tid), node);
                    }
                }
                // 3) Rekey worktree_tabs.
                let old_tabs = std::mem::take(&mut self.worktree_tabs);
                for ((pi, wi), tabs) in old_tabs {
                    if let Some(np) = remap(pi) {
                        self.worktree_tabs.insert((np, wi), tabs);
                    }
                }
                // 4) Rekey expand state.
                self.expanded_projects =
                    self.expanded_projects.iter().filter_map(|pi| remap(*pi)).collect();
                self.expanded_worktrees = self
                    .expanded_worktrees
                    .iter()
                    .filter_map(|(pi, wi)| remap(*pi).map(|np| (np, *wi)))
                    .collect();
                // 5) Repoint active_tab / selected.
                self.active_tab =
                    self.active_tab.and_then(|(pi, wi, tid)| remap(pi).map(|np| (np, wi, tid)));
                let (spi, swi, stid) = self.selected;
                self.selected = match remap(spi) {
                    Some(np) => (np, swi, stid),
                    None => (0, 0, usize::MAX),
                };
                // 6) Clear any focused/files pane whose backing pane was town down.
                if let Some(fp) = self.files_pane {
                    if !self.panes.contains_key(&fp) {
                        self.files_pane = None;
                        self.file_pane_paths.clear();
                        self.file_pane_active = 0;
                    }
                }
                if let Some(f) = self.focused {
                    if !self.panes.contains_key(&f) {
                        self.focused = None;
                    }
                }
                // 7) If the active tab survived but lost focus, refocus its first
                // leaf. If it was removed, drop the panel's cwd context.
                match self.active_tab {
                    Some(at) => {
                        if self.focused.is_none() {
                            self.focused = self.layouts.get(&at).map(|n| n.first_leaf());
                        }
                    }
                    None => {
                        self.focused = None;
                        self.active_cwd = None;
                    }
                }
                // Rebuild the right panel ONLY when the active worktree changed.
                // Removing a *non-active* project leaves the active repo's
                // branch/changes/files untouched, so we skip the git shell-out
                // entirely. When the active tab itself was torn down, active_cwd
                // is None and refresh_panel clears the panel with ZERO git. Either
                // way a remove runs no git subprocess.
                if self.active_tab.is_none() {
                    self.refresh_panel(ctx);
                }
    }

    /// A single row inside the context menu (icon + label). Dispatches
    /// CloseContextMenu then the real `action` when clicked.
    /// Get-or-create the persistent hover state for a menu/modal row keyed by a
    /// stable string. The `MouseStateHandle` must outlive a single render so the
    /// `Hoverable` sees a stable is_hovered across the mouse-in → repaint gap.
    fn hover_handle(&self, key: &str) -> MouseStateHandle {
        let mut map = self.menu_hover.borrow_mut();
        if let Some(h) = map.get(key) {
            return h.clone();
        }
        let h = MouseStateHandle::default();
        map.insert(key.to_string(), h.clone());
        h
    }

    /// Thin wrapper: a plain menu row with no shortcut hint and no danger accent.
    fn menu_item(&self, glyph: &str, label: &str, action: CraneShellAction) -> Box<dyn Element> {
        self.menu_item_hint(glyph, label, None, false, action)
    }

    /// A menu row with an optional right-aligned shortcut `hint` and a `danger`
    /// accent (destructive actions). Hover paints `selection_wash()` with
    /// `text_hover()` text — or `danger_wash()` with `error()` text when danger.
    /// Dispatches CloseContextMenu then the real `action` when clicked.
    fn menu_item_hint(
        &self,
        glyph: &str,
        label: &str,
        hint: Option<&str>,
        danger: bool,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        // Key the hover state by the row label. Only one context menu is open at
        // a time and labels are unique within a menu, so this is a stable key.
        let state = self.hover_handle(&format!("mi:{label}"));
        let glyph = glyph.to_string();
        let label = label.to_string();
        let hint = hint.map(|h| h.to_string());
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        Hoverable::new(state, move |ms| {
            let hovered = ms.is_hovered();
            let bg = if hovered {
                if danger {
                    theme::danger_wash()
                } else {
                    theme::selection_wash()
                }
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let text_color = if danger {
                theme::error()
            } else if hovered {
                theme::text_hover()
            } else {
                theme::text()
            };
            let glyph_color = if danger {
                theme::error()
            } else {
                theme::text_muted()
            };
            let mut row = Flex::row()
                .with_child(
                    Container::new(
                        Text::new(glyph.clone(), icon_font, 12.0)
                            .with_color(glyph_color)
                            .finish(),
                    )
                    .with_padding_right(8.0)
                    .finish(),
                )
                .with_child(
                    Text::new(label.clone(), ui_font, 12.0)
                        .with_color(text_color)
                        .finish(),
                );
            if let Some(h) = &hint {
                // Expanded spacer pushes the muted hint to the row's right edge.
                row = row
                    .with_child(
                        Expanded::new(
                            1.0,
                            ConstrainedBox::new(Rect::new().finish())
                                .with_height(1.0)
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Text::new(h.clone(), ui_font, 10.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    );
            }
            Container::new(row.finish())
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .with_padding_left(10.0)
                .with_padding_right(20.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }

    /// A small-caps muted section label (e.g. "HIGHLIGHT") for grouping menu rows.
    fn menu_label(&self, text: &'static str) -> Box<dyn Element> {
        Container::new(
            Text::new(text.to_string(), self.ui_font, 10.0)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_top(6.0)
        .with_padding_bottom(3.0)
        .finish()
    }

    /// A thin horizontal divider for use inside the context menu.
    fn menu_separator(&self) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                .with_height(1.0)
                .finish(),
        )
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    }

    /// One circular tint swatch (18×18, radius 9). `color = None` renders the
    /// hollow "none"/clear swatch (transparent fill, muted border). `active`
    /// (this tint is the current one) draws a `text_hover()` ring, as does hover;
    /// a colored swatch at rest has a transparent ring so the row never reflows.
    /// Dispatches CloseContextMenu then `action` on click.
    fn tint_swatch(
        &self,
        key: &str,
        color: Option<[u8; 3]>,
        active: bool,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        let state = self.hover_handle(key);
        Hoverable::new(state, move |ms| {
            let hovered = ms.is_hovered();
            let bg = match color {
                Some(rgb) => ColorU::new(rgb[0], rgb[1], rgb[2], 255),
                None => ColorU::new(0, 0, 0, 0),
            };
            // Distinct state rings at a CONSTANT 2px width — Container includes
            // border width in its laid-out size, so a per-state width change
            // would make hovered swatches shrink and the row jitter. States are
            // told apart by color only: ACTIVE (this tint is applied) = strong
            // text_hover ring, wins over hover; HOVER = lighter text_muted
            // preview ring; idle colored dots = transparent ring. The hollow
            // "none" swatch's resting body outline (border()) is REPLACED by
            // the state ring so the two strokes never visually merge.
            let ring = if active {
                theme::text_hover()
            } else if hovered {
                theme::text_muted()
            } else if color.is_none() {
                theme::border()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let dot = Container::new(
                ConstrainedBox::new(Rect::new().finish())
                    .with_width(18.0)
                    .with_height(18.0)
                    .finish(),
            )
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_border(Border::all(2.0).with_border_color(ring))
            .finish();
            Container::new(dot).with_uniform_padding(3.0).finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }

    /// A single row of circular tint swatches: a leading hollow "none" swatch
    /// wired to `clear`, then 8 colored dots. `current` marks the active dot.
    /// `prefix` keeps hover-state keys distinct per menu. `on_pick` maps a chosen
    /// RGB to the action dispatched (after closing the menu).
    fn tint_palette_row<F>(
        &self,
        prefix: &str,
        current: Option<[u8; 3]>,
        clear: CraneShellAction,
        on_pick: F,
    ) -> Box<dyn Element>
    where
        F: Fn([u8; 3]) -> CraneShellAction,
    {
        const PALETTE: [[u8; 3]; 8] = [
            [239, 83, 80],
            [255, 152, 0],
            [255, 202, 40],
            [102, 187, 106],
            [38, 166, 154],
            [66, 165, 245],
            [171, 71, 188],
            [236, 64, 122],
        ];
        let mut swatches = Flex::row().with_child(self.tint_swatch(
            &format!("{prefix}:none"),
            None,
            current.is_none(),
            clear,
        ));
        for rgb in PALETTE {
            swatches = swatches.with_child(self.tint_swatch(
                &format!("{prefix}:{}:{}:{}", rgb[0], rgb[1], rgb[2]),
                Some(rgb),
                current == Some(rgb),
                on_pick(rgb),
            ));
        }
        Container::new(swatches.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(2.0)
            .with_padding_bottom(4.0)
            .finish()
    }

    /// Build the project context menu overlay anchored at the stored click position.
    fn project_context_menu(&self, pm: &ProjectContextMenu) -> Box<dyn Element> {
        let pi = pm.project_idx;
        let is_loose = self.projects.get(pi).map(|p| p.is_loose).unwrap_or(false);
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        items = items.with_child(self.menu_item(
            icons::FOLDER_OPEN,
            "Reveal in Finder",
            CraneShellAction::RevealProjectInFinder(pi),
        ));
        items = items.with_child(self.menu_item(
            icons::COPY,
            "Copy Path",
            CraneShellAction::CopyProjectPath(pi),
        ));
        items = items.with_child(self.menu_separator());

        // Loose projects (no .git) get an "Initialize Git" option that runs
        // `git init` and reloads so the project flips to non-loose immediately.
        if is_loose {
            items = items.with_child(self.menu_item(
                icons::GIT_BRANCH,
                "Initialize Git",
                CraneShellAction::InitGitProject(pi),
            ));
            items = items.with_child(self.menu_separator());
        }

        // Highlight: a leading hollow "none" swatch + 8 circular tint dots,
        // grouped under a small-caps section label. The current tint is ringed.
        let current = self.projects.get(pi).and_then(|p| p.tint);
        items = items.with_child(self.menu_label("HIGHLIGHT"));
        items = items.with_child(self.tint_palette_row(
            "psw",
            current,
            CraneShellAction::SetProjectTint(pi, None),
            move |rgb| CraneShellAction::SetProjectTint(pi, Some(rgb)),
        ));
        // Atomic-group rule: when this project is one of MULTIPLE members of a
        // folder group, hide its individual "Remove Project" — the group is
        // removed whole via the folder header's "Remove folder group". A single-
        // member group (or a standalone project) keeps the individual remove.
        if !self.in_multi_group(pi) {
            items = items.with_child(self.menu_separator());
            items = items.with_child(self.menu_item_hint(
                icons::TRASH,
                "Remove Project",
                None,
                true,
                CraneShellAction::RemoveProject(pi),
            ));
        }

        let menu_box = ConstrainedBox::new(
            Container::new(items.finish())
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                    theme::menu_shadow(),
                ))
                .with_uniform_padding(5.0)
                .finish(),
        )
        // FIX: fixed 220px width so the menu is a compact popover anchored at
        // the click point, not a full-panel-width strip. The menu rows use
        // padded Containers (no Expanded/full-width fills) so nothing stretches.
        .with_width(220.0)
        .finish();

        let positioned = Container::new(menu_box)
            .with_padding_top(pm.y)
            .with_padding_left(pm.x)
            .finish();

        self.menu_dismiss(positioned)
    }

    /// Wrap a built menu column in the standard 220px popover chrome +
    /// dismiss-on-outside-click overlay, positioned at (x, y).
    fn menu_popover(&self, items: Box<dyn Element>, x: f32, y: f32) -> Box<dyn Element> {
        let menu_box = ConstrainedBox::new(
            Container::new(items)
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                    theme::menu_shadow(),
                ))
                .with_uniform_padding(5.0)
                .finish(),
        )
        .with_width(220.0)
        .finish();
        // Popover clamps on-screen: pulled left of the right edge, flipped
        // ABOVE the click when the menu would cross the window bottom.
        let positioned =
            Box::new(crate::warpui::rect_probe::Popover::new(menu_box, x, y));
        self.menu_dismiss(positioned)
    }

    /// Wrap an already-positioned menu popover in a full-window dismiss backdrop.
    /// A LEFT click outside the menu closes it and is CONSUMED (StopPropagation),
    /// so it can't also activate whatever sits behind the menu. A RIGHT click
    /// outside closes it but PROPAGATES to the row beneath, which opens that
    /// row's own menu in the SAME click — so right-clicking a different row
    /// relocates the menu instead of the two-click dance (first click only
    /// dismisses). Replaces the framework `Dismiss`, whose
    /// `prevent_interaction_with_other_elements` swallows the right-click.
    fn menu_dismiss(&self, positioned: Box<dyn Element>) -> Box<dyn Element> {
        let backdrop = EventHandler::new(
            Rect::new()
                .with_background_color(ColorU::new(0, 0, 0, 0))
                .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
            DispatchEventResult::StopPropagation
        })
        .on_right_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
            DispatchEventResult::PropagateToParent
        })
        // Absorb scroll so a wheel over the dim area (outside the menu popover)
        // can't bleed through to the pane underneath. The menu's own scrollable
        // content still scrolls: the positioned popover is the higher Stack child
        // and consumes the wheel first (Waterfall dispatches top child first).
        .on_scroll_wheel(|_ctx, _app, _delta, _mods| DispatchEventResult::StopPropagation)
        .with_always_handle()
        .finish();
        Box::new(Stack::new().with_child(backdrop).with_child(positioned))
    }

    /// Right-Panel row context menu (Changes row or Files row).
    fn row_menu_overlay(&self, menu: &RowMenu) -> Box<dyn Element> {
        match menu {
            RowMenu::Change { path, staged, has_unstaged, x, y } => {
                let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
                // Offer Stage when a worktree change exists and Unstage when an
                // index change exists — for an `MM` file BOTH appear, so the
                // user can stage the worktree edit or unstage the index edit
                // independently.
                if *has_unstaged {
                    items = items.with_child(self.menu_item(
                        icons::PLUS,
                        "Stage",
                        CraneShellAction::StagePaths(vec![path.clone()]),
                    ));
                }
                if *staged {
                    items = items.with_child(self.menu_item(
                        icons::MINUS,
                        "Unstage",
                        CraneShellAction::UnstagePaths(vec![path.clone()]),
                    ));
                }
                let abs = self
                    .active_cwd
                    .as_ref()
                    .map(|c| c.join(path))
                    .unwrap_or_else(|| PathBuf::from(path));
                items = items.with_child(self.menu_item(
                    icons::GIT_DIFF,
                    "Open Diff",
                    CraneShellAction::OpenDiff(abs.clone()),
                ));
                items = items.with_child(self.menu_item(
                    icons::FILE,
                    "Open as File",
                    CraneShellAction::OpenFileAt(abs.clone()),
                ));
                items = items.with_child(self.menu_separator());
                // Copy the ABSOLUTE path (matches the Files-row menu), so the
                // pasted value is usable outside the repo root.
                items = items.with_child(self.menu_item(
                    icons::COPY,
                    "Copy Path",
                    CraneShellAction::CopyPathStr(abs.to_string_lossy().to_string()),
                ));
                self.menu_popover(items.finish(), *x, *y)
            }
            RowMenu::File { path, is_dir, x, y } => {
                let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
                if !*is_dir {
                    items = items.with_child(self.menu_item(
                        icons::FILE,
                        "Open",
                        CraneShellAction::OpenFileAt(path.clone()),
                    ));
                }
                items = items.with_child(self.menu_item(
                    icons::FOLDER_OPEN,
                    "Reveal in Finder",
                    CraneShellAction::RevealPathInFinder(path.clone()),
                ));
                items = items.with_child(self.menu_item(
                    icons::COPY,
                    "Copy Path",
                    CraneShellAction::CopyPathStr(path.to_string_lossy().to_string()),
                ));
                items = items.with_child(self.menu_separator());
                // New entries land in the dir itself (folder row) or the parent.
                let parent = if *is_dir {
                    path.clone()
                } else {
                    path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.clone())
                };
                items = items.with_child(self.menu_item(
                    icons::FILE,
                    "New File…",
                    CraneShellAction::NewEntry { parent: parent.clone(), is_folder: false },
                ));
                items = items.with_child(self.menu_item(
                    icons::FOLDER_PLUS,
                    "New Folder…",
                    CraneShellAction::NewEntry { parent, is_folder: true },
                ));
                items = items.with_child(self.menu_separator());
                items = items.with_child(self.menu_item_hint(
                    icons::TRASH,
                    "Delete",
                    None,
                    true,
                    CraneShellAction::RequestDelete(path.clone()),
                ));
                self.menu_popover(items.finish(), *x, *y)
            }
        }
    }

    /// The branch-picker overlay: a scrollable list of local + remote branches;
    /// clicking one checks it out. (Simple list — no fuzzy filter input yet.)
    fn branch_picker_overlay(&self, x: f32, y: f32) -> Box<dyn Element> {
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if self.branch_list.is_empty() {
            items = items.with_child(
                Container::new(
                    Text::new("(no branches)".to_string(), self.ui_font, 12.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_uniform_padding(8.0)
                .finish(),
            );
        }
        for b in &self.branch_list {
            let is_current = *b == self.branch;
            let glyph = if is_current { icons::CHECK } else { icons::GIT_BRANCH };
            let color = if is_current { theme::accent() } else { theme::text() };
            let branch = b.clone();
            let label = b.clone();
            let ui_font = self.ui_font;
            let icon_font = self.icon_font;
            let state = self.hover_handle(&format!("bp:{b}"));
            let item = Hoverable::new(state, move |ms| {
                let bg = if ms.is_hovered() {
                    theme::row_hover()
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let row = Flex::row()
                    .with_child(
                        Container::new(
                            Text::new(glyph.to_string(), icon_font, 12.0)
                                .with_color(color)
                                .finish(),
                        )
                        .with_padding_right(8.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new(label.clone(), ui_font, 12.0).with_color(color).finish(),
                    )
                    .finish();
                Container::new(row)
                    .with_background_color(bg)
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(5.0)
                    .with_padding_bottom(5.0)
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                ctx.dispatch_typed_action(CraneShellAction::CheckoutBranch(branch.clone()));
            })
            .finish();
            items = items.with_child(item);
        }
        // Cap the list height and make it scroll so a long branch list (or one
        // opened near the window bottom) stays fully reachable instead of drawing
        // off-screen. The (x, y) origin is clamped on-screen by the caller.
        const ROW_H: f32 = 27.0;
        const MAX_H: f32 = 300.0;
        let content_h = (self.branch_list.len().max(1) as f32) * ROW_H;
        let body: Box<dyn Element> = if content_h > MAX_H {
            ConstrainedBox::new(
                ClippedScrollable::vertical(
                    self.branch_scroll.clone(),
                    items.finish(),
                    ScrollbarWidth::Auto,
                    Fill::Solid(theme::border()),
                    Fill::Solid(theme::text_muted()),
                    Fill::None,
                )
                .finish(),
            )
            .with_height(MAX_H)
            .finish()
        } else {
            items.finish()
        };
        self.menu_popover(body, x, y)
    }

    /// Estimated pixel height of the branch-picker popover — used to clamp its
    /// origin so it stays on-screen.
    fn branch_picker_height(&self) -> f32 {
        let rows = self.branch_list.len().max(1) as f32;
        (rows * 27.0).min(300.0) + 12.0
    }

    /// Wrap a Tab row so a right-click opens the Tab context menu.
    fn tab_right_click(
        &self,
        base: Box<dyn Element>,
        key: (usize, usize, usize),
    ) -> Box<dyn Element> {
        EventHandler::new(base)
            .on_right_mouse_down(move |ctx, _, pos| {
                ctx.dispatch_typed_action(CraneShellAction::ShowTabMenu {
                    key,
                    x: pos.x(),
                    y: pos.y(),
                });
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// The worktree/branch-row context menu overlay. Reveal in Finder + Copy
    /// Path reuse the existing path actions; both operate on the worktree's
    /// checkout directory.
    fn worktree_menu_overlay(&self, pi: usize, wi: usize, x: f32, y: f32) -> Box<dyn Element> {
        let wt_path = self
            .projects
            .get(pi)
            .and_then(|p| p.worktrees.get(wi))
            .map(|w| w.path.clone())
            .unwrap_or_default();
        let path = PathBuf::from(&wt_path);
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        items = items.with_child(self.menu_item(
            icons::FOLDER_OPEN,
            "Reveal in Finder",
            CraneShellAction::RevealPathInFinder(path.clone()),
        ));
        items = items.with_child(self.menu_item(
            icons::COPY,
            "Copy Path",
            CraneShellAction::CopyPathStr(wt_path.clone()),
        ));
        items = items.with_child(self.menu_separator());
        // Inline rename → per-path display-name override on commit.
        items = items.with_child(self.menu_item(
            icons::PENCIL_SIMPLE,
            "Rename",
            CraneShellAction::StartRenameWorktree { pi, wi },
        ));
        // Highlight: hollow "none" reset + circular tint dots, ringed on the
        // current tint. Keyed by the worktree path so it survives reload shifts.
        let current = self.worktree_tints.get(&wt_path).copied();
        items = items.with_child(self.menu_label("HIGHLIGHT"));
        items = items.with_child(self.tint_palette_row(
            "wsw",
            current,
            CraneShellAction::SetWorktreeTint { pi, wi, tint: None },
            move |rgb| CraneShellAction::SetWorktreeTint { pi, wi, tint: Some(rgb) },
        ));
        // Remove Worktree runs `git worktree remove --force` after a confirm.
        // The primary working tree can't be detached (the handler would no-op),
        // so hide the item there instead of offering a dead action — removing
        // the project is the operation that applies to the main checkout.
        let is_main = self
            .projects
            .get(pi)
            .map(|p| p.path == wt_path)
            .unwrap_or(true);
        if !is_main {
            items = items.with_child(self.menu_separator());
            items = items.with_child(self.menu_item_hint(
                icons::TRASH,
                "Remove Worktree",
                None,
                true,
                CraneShellAction::RemoveWorktree { pi, wi },
            ));
        }
        self.menu_popover(items.finish(), x, y)
    }

    /// The Tab-row context menu overlay. Close Tab / Close Other Tabs are wired;
    /// Rename + Highlight are deferred (need modal / tint infra).
    fn tab_menu_overlay(
        &self,
        key: (usize, usize, usize),
        x: f32,
        y: f32,
    ) -> Box<dyn Element> {
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        items = items.with_child(self.menu_item(
            icons::X,
            "Close Tab",
            CraneShellAction::CloseTab(key),
        ));
        items = items.with_child(self.menu_item(
            icons::X,
            "Close Other Tabs",
            CraneShellAction::CloseOtherTabs(key),
        ));
        items = items.with_child(self.menu_separator());
        // Inline rename → updates TabMeta.name (persisted via worktree_tabs).
        items = items.with_child(self.menu_item(
            icons::PENCIL_SIMPLE,
            "Rename",
            CraneShellAction::StartRenameTab { key },
        ));
        // Highlight: hollow "none" reset + circular tint dots, keyed by
        // (worktree_path, tab_id) and ringed on the current tint.
        let (pi, wi, tid) = key;
        let current = self
            .projects
            .get(pi)
            .and_then(|p| p.worktrees.get(wi))
            .and_then(|w| self.tab_tints.get(&(w.path.clone(), tid)).copied());
        items = items.with_child(self.menu_label("HIGHLIGHT"));
        items = items.with_child(self.tint_palette_row(
            "tsw",
            current,
            CraneShellAction::SetTabTint { key, tint: None },
            move |rgb| CraneShellAction::SetTabTint { key, tint: Some(rgb) },
        ));
        self.menu_popover(items.finish(), x, y)
    }

    /// The folder-group header context menu: a "HIGHLIGHT" section label, a
    /// circular tint palette (hollow "none" reset + 8 dots, ringed on the current
    /// tint), a separator, and a destructive "Remove folder group" (removes every
    /// member project atomically).
    fn folder_menu_overlay(&self, group: &str, x: f32, y: f32) -> Box<dyn Element> {
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        let current = self.group_tints.get(group).copied();
        items = items.with_child(self.menu_label("HIGHLIGHT"));
        let g_pick = group.to_string();
        let g_clear = group.to_string();
        items = items.with_child(self.tint_palette_row(
            "gsw",
            current,
            CraneShellAction::SetGroupTint { group: g_clear, tint: None },
            move |rgb| CraneShellAction::SetGroupTint { group: g_pick.clone(), tint: Some(rgb) },
        ));
        items = items.with_child(self.menu_separator());
        items = items.with_child(self.menu_item_hint(
            icons::TRASH,
            "Remove folder group",
            None,
            true,
            CraneShellAction::RemoveGroup(group.to_string()),
        ));
        self.menu_popover(items.finish(), x, y)
    }

    /// A centred confirm overlay for deleting a file/folder.
    fn delete_confirm_overlay(&self, path: &std::path::Path) -> Box<dyn Element> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let cancel = EventHandler::new(
            Container::new(
                Text::new("Cancel".to_string(), self.ui_font, 12.0)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_background_color(theme::surface())
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::CancelDelete);
            DispatchEventResult::StopPropagation
        })
        .finish();
        let del = EventHandler::new(
            Container::new(
                Text::new("Move to Trash".to_string(), self.ui_font, 12.0)
                    .with_color(ColorU::new(255, 255, 255, 255))
                    .finish(),
            )
            .with_background_color(theme::error())
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::ConfirmDelete);
            DispatchEventResult::StopPropagation
        })
        .finish();
        let dialog = Container::new(
            Flex::column()
                .with_child(
                    Text::new(format!("Delete \"{name}\"?"), self.ui_font, 13.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_child(Self::spacer(6.0))
                .with_child(
                    Text::new(
                        "Moves to the system Trash — recoverable from there."
                            .to_string(),
                        self.ui_font,
                        11.0,
                    )
                    .with_color(theme::text_muted())
                    .finish(),
                )
                .with_child(Self::spacer(14.0))
                .with_child(
                    Flex::row()
                        .with_child(Expanded::new(1.0, ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish()).finish())
                        .with_child(cancel)
                        .with_child(Self::spacer(8.0))
                        .with_child(del)
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme::surface())
        .with_border(Border::all(1.0).with_border_color(theme::border()))
        .with_uniform_padding(16.0)
        .finish();
        let boxed = ConstrainedBox::new(dialog).with_width(320.0).finish();
        // Centre-ish: pad from the top/left. (Full centring would need window
        // size; a fixed offset reads fine for a small confirm.)
        let positioned = Container::new(boxed)
            .with_padding_top(120.0)
            .with_padding_left(120.0)
            .finish();
        Box::new(
            Dismiss::new(positioned)
                .prevent_interaction_with_other_elements()
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(CraneShellAction::CancelDelete);
                }),
        )
    }

    // ---- Notification toasts ---------------------------------------------
    //
    // Bottom-right stack of small rounded cards, one per live toast. Unlike the
    // menus/modals above it must NOT absorb clicks: the overlay is built from
    // transparent, non-interactive spacers (Flex/Expanded/Rect all report
    // events unhandled) so clicks fall through to the panes behind; only each
    // card's own EventHandler consumes. Auto-dismiss is driven by the fast tick
    // sweeping `self.toasts`; render only paints the still-live ones.

    /// The bottom-right toast stack overlay. Newest toast at the bottom.
    fn toast_overlay(&self) -> Box<dyn Element> {
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::End);
        for t in self.toasts.iter().filter(|t| t.at.elapsed() < TOAST_TTL) {
            col = col.with_child(self.toast_card(t));
            col = col.with_child(Self::spacer(8.0));
        }
        let cards = ConstrainedBox::new(col.finish()).with_width(360.0).finish();
        // Right-align the fixed-width card column, with a right margin. The
        // leading Expanded spacer uses Empty, not Rect: Rect always registers
        // a hit-test region on paint even with no background, so a bare Rect
        // here would swallow clicks across the ENTIRE window (everything left
        // of the toast column) while any toast is showing. Empty renders
        // nothing and never registers a hit region.
        let row = Flex::row()
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(cards)
            .with_child(Self::spacer(20.0))
            .finish();
        // Push the whole thing to the bottom with a bottom margin. Same
        // click-through reasoning as the row's leading spacer above.
        Flex::column()
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(row)
            .with_child(Self::spacer(24.0))
            .finish()
    }

    /// One toast card: header (source label + urgency glyph + X dismiss) and the
    /// notification body. The card body is click-to-focus; the X dismisses.
    fn toast_card(&self, t: &Toast) -> Box<dyn Element> {
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        let (stroke, accent, glyph) = if t.urgent {
            (theme::error(), theme::error(), icons::WARNING)
        } else {
            (theme::border(), theme::accent(), icons::INFO)
        };
        let id = t.id;
        // Far-right X dismiss button.
        let close = EventHandler::new(
            Container::new(
                Text::new(icons::X.to_string(), icon_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_uniform_padding(4.0)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::DismissToast(id));
            DispatchEventResult::StopPropagation
        })
        .finish();
        // Header row: urgency glyph + source breadcrumb, X pinned right.
        let header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    Text::new(glyph.to_string(), icon_font, 13.0)
                        .with_color(accent)
                        .finish(),
                )
                .with_padding_right(6.0)
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(t.source.clone(), ui_font, 11.0)
                        .with_color(accent)
                        .finish(),
                )
                .finish(),
            )
            .with_child(close)
            .finish();
        let body = Text::new(truncate_body(&t.body, 180), ui_font, 12.0)
            .with_color(theme::text())
            .finish();
        let card = Container::new(
            Flex::column()
                .with_child(header)
                .with_child(Self::spacer(4.0))
                .with_child(body)
                .finish(),
        )
        .with_background_color(theme::surface())
        .with_border(Border::all(1.0).with_border_color(stroke))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(12.0)
        .finish();
        // Clicking the card body focuses the originating tab (best-effort).
        EventHandler::new(card)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FocusToastSource(id));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    // ---- Modal framework -------------------------------------------------
    //
    // A modal is the same overlay idea as the context menus above, but
    // full-screen: a dim backdrop Rect (semi-transparent black) that covers the
    // whole window and ABSORBS clicks, with a centered content card on top.
    // Backdrop click or Escape dispatches `CloseModal`; the card's own buttons
    // drive confirm/cancel. Rendered as the LAST (topmost) child of the root
    // stack. Port of old egui `src/modals/*`.

    /// A pill-style modal button. `primary` renders it filled with the accent
    /// (default action) or, when `danger`, the error colour; otherwise a plain
    /// surface pill. Ported from the old egui modal button rows.
    fn modal_button(
        &self,
        label: &str,
        style: ModalBtn,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        let (bg, fg) = match style {
            ModalBtn::Danger => (theme::error(), ColorU::new(255, 255, 255, 255)),
            ModalBtn::Plain => (theme::surface(), theme::text()),
            ModalBtn::Primary => (theme::accent(), ColorU::new(255, 255, 255, 255)),
        };
        EventHandler::new(
            Container::new(
                Text::new(label.to_string(), self.ui_font, 12.0)
                    .with_color(fg)
                    .finish(),
            )
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(theme::border()))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    /// Wrap a built card in the dim full-screen backdrop + centering scaffold.
    /// The backdrop closes the modal on click; empty-Flex spacers around the card
    /// record no hits so an outside click falls through to the backdrop. The card
    /// itself swallows clicks so interacting inside never dismisses it.
    fn modal_scaffold(&self, card: Box<dyn Element>) -> Box<dyn Element> {
        // The card absorbs clicks (so clicking its chrome doesn't close the modal).
        let card = EventHandler::new(card)
            .on_left_mouse_down(|_ctx, _app, _pos| DispatchEventResult::StopPropagation)
            .with_always_handle()
            .finish();
        // Centre the card with flexible empty-Flex spacers (no hit recording, so
        // clicks in the margins reach the backdrop below).
        let centered = Flex::column()
            .with_child(Expanded::new(1.0, Flex::column().finish()).finish())
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .with_child(card)
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Flex::column().finish()).finish())
            .finish();
        // Dim backdrop: semi-transparent black filling the window, click-to-close.
        let backdrop = EventHandler::new(
            Rect::new()
                .with_background_color(ColorU::new(0, 0, 0, 150))
                .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::CloseModal);
            DispatchEventResult::StopPropagation
        })
        // Absorb scroll so a wheel over the dim backdrop (in the margins around
        // the card) can't bleed through to the panes behind the modal. The
        // card's own scrollable list still scrolls: the centered card is the
        // higher Stack child and consumes the wheel first (Waterfall dispatches
        // the top child first, so this backdrop only sees wheels the card missed).
        .on_scroll_wheel(|_ctx, _app, _delta, _mods| DispatchEventResult::StopPropagation)
        .with_always_handle()
        .finish();
        Box::new(Stack::new().with_child(backdrop).with_child(centered))
    }

    /// Render the active modal as the topmost root-stack child.
    fn modal_overlay(&self, m: &Modal, app: &AppContext) -> Box<dyn Element> {
        let card = match m {
            Modal::ConfirmQuit => self.confirm_quit_card(app),
            Modal::ConfirmClosePane(id) => self.confirm_close_pane_card(*id, app),
            Modal::Settings => self.settings_card(),
            Modal::Help => self.help_card(),
            Modal::FindInFiles => self.find_in_files_card(),
            Modal::TabSwitcher => self.tab_switcher_card(),
            Modal::SwitchBranch => self.switch_branch_card(),
            Modal::NewWorkspace => self.new_workspace_card(),
            Modal::ConfirmRemoveWorktree { pi, wi } => {
                self.confirm_remove_worktree_card(*pi, *wi)
            }
            Modal::ConfirmCloseTab { key } => self.confirm_close_tab_card(*key, app),
            Modal::ConfirmCloseFileTab { index } => self.confirm_close_file_tab_card(*index),
        };
        self.modal_scaffold(card)
    }

    /// Standard modal card chrome: the premium popover treatment shared with
    /// menus — surface background, 1px border, 10px corners, drop shadow, and a
    /// consistent 16px inner inset. Fixed width; height fits its body.
    fn modal_card(&self, width: f32, body: Box<dyn Element>) -> Box<dyn Element> {
        ConstrainedBox::new(
            Container::new(body)
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                    theme::menu_shadow(),
                ))
                .with_uniform_padding(16.0)
                .finish(),
        )
        .with_width(width)
        .finish()
    }

    /// A modal heading with a close (×) button pinned to the far right. The
    /// close reuses the shared 20×20 hover-lit `icon_button`; its hover key is
    /// derived from the title so each modal's close has an independent state.
    fn modal_header(&self, title: &str) -> Box<dyn Element> {
        let close = self.icon_button(
            &format!("modalclose:{title}"),
            icons::X,
            CraneShellAction::CloseModal,
        );
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(title.to_string(), self.ui_font, 13.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
            .with_child(close)
            .finish()
    }

    /// ConfirmQuit card — "A process is still running. Quit anyway?"
    fn confirm_quit_card(&self, app: &AppContext) -> Box<dyn Element> {
        let running = self.count_running_terminals(app);
        let body = if running > 0 {
            format!(
                "{running} running terminal process{} will be killed.",
                if running == 1 { "" } else { "es" }
            )
        } else {
            "All open panes will close.".to_string()
        };
        let col = Flex::column()
            .with_child(
                Text::new("Quit Crane?".to_string(), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(body, self.ui_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_child(Self::spacer(16.0))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .with_child(self.modal_button(
                        "Cancel",
                        ModalBtn::Plain,
                        CraneShellAction::CloseModal,
                    ))
                    .with_child(Self::spacer(8.0))
                    .with_child(self.modal_button(
                        "Quit",
                        ModalBtn::Danger,
                        CraneShellAction::QuitConfirmed,
                    ))
                    .finish(),
            )
            .finish();
        self.modal_card(360.0, col)
    }

    /// ConfirmClosePane card — Cmd+W over a terminal running a foreground program.
    fn confirm_close_pane_card(&self, id: PaneId, _app: &AppContext) -> Box<dyn Element> {
        let col = Flex::column()
            .with_child(
                Text::new("Close this pane?".to_string(), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(
                    "A process is still running in this terminal.".to_string(),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(Self::spacer(16.0))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .with_child(self.modal_button(
                        "Cancel",
                        ModalBtn::Plain,
                        CraneShellAction::CloseModal,
                    ))
                    .with_child(Self::spacer(8.0))
                    .with_child(self.modal_button(
                        "Close",
                        ModalBtn::Danger,
                        CraneShellAction::ConfirmClosePane(id),
                    ))
                    .finish(),
            )
            .finish();
        self.modal_card(360.0, col)
    }

    /// ConfirmCloseFileTab card — the file chip's × over a buffer with unsaved
    /// edits. Cancel keeps the tab; Close discards the edits.
    fn confirm_close_file_tab_card(&self, index: usize) -> Box<dyn Element> {
        let name = self
            .file_pane_paths
            .get(index)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this file".to_string());
        let col = Flex::column()
            .with_child(
                Text::new(format!("Close “{name}”?"), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(
                    "It has unsaved changes that will be lost.".to_string(),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(Self::spacer(16.0))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .with_child(self.modal_button(
                        "Cancel",
                        ModalBtn::Plain,
                        CraneShellAction::CloseModal,
                    ))
                    .with_child(Self::spacer(8.0))
                    .with_child(self.modal_button(
                        "Close",
                        ModalBtn::Danger,
                        CraneShellAction::FileTabCloseConfirmed(index),
                    ))
                    .finish(),
            )
            .finish();
        self.modal_card(360.0, col)
    }

    /// ConfirmRemoveWorktree card — raised from the worktree-row menu before
    /// `git worktree remove`. Reads the precomputed `remove_wt_info` so it can
    /// warn about uncommitted / unpushed work without shelling out per frame.
    fn confirm_remove_worktree_card(&self, pi: usize, wi: usize) -> Box<dyn Element> {
        let info = self.remove_wt_info.as_ref();
        let label = info.map(|i| i.label.clone()).unwrap_or_default();
        let path = info.map(|i| i.path.clone()).unwrap_or_default();
        let dirty = info.map(|i| i.dirty).unwrap_or(false);
        let ahead = info.map(|i| i.ahead).unwrap_or(0);

        let mut col = Flex::column()
            .with_child(
                Text::new("Remove Worktree?".to_string(), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(
                    format!("“{label}” will be detached with git worktree remove."),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            );
        if !path.is_empty() {
            col = col.with_child(Self::spacer(6.0)).with_child(
                Text::new(path, self.ui_font, 11.0)
                    .with_color(theme::text_muted())
                    .finish(),
            );
        }
        // WARN when there is work that would be lost — dirty tree and/or
        // commits not yet pushed. These drive the `--force` removal, so the
        // user gets an explicit heads-up before confirming.
        if dirty {
            col = col.with_child(Self::spacer(10.0)).with_child(
                Text::new(
                    format!(
                        "{}  Uncommitted changes here will be lost.",
                        icons::WARNING
                    ),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::warning())
                .finish(),
            );
        }
        if ahead > 0 {
            col = col.with_child(Self::spacer(6.0)).with_child(
                Text::new(
                    format!(
                        "{}  {ahead} unpushed commit{} on this branch.",
                        icons::WARNING,
                        if ahead == 1 { "" } else { "s" }
                    ),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::warning())
                .finish(),
            );
        }
        col = col.with_child(Self::spacer(16.0)).with_child(
            Flex::row()
                .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                .with_child(self.modal_button(
                    "Cancel",
                    ModalBtn::Plain,
                    CraneShellAction::CloseModal,
                ))
                .with_child(Self::spacer(8.0))
                .with_child(self.modal_button(
                    "Remove",
                    ModalBtn::Danger,
                    CraneShellAction::RemoveWorktreeConfirmed { pi, wi },
                ))
                .finish(),
        );
        self.modal_card(380.0, col.finish())
    }

    /// ConfirmCloseTab card — raised before tearing down a tab that holds a
    /// running terminal or an editor with unsaved edits. Cancel keeps the tab.
    fn confirm_close_tab_card(
        &self,
        key: (usize, usize, usize),
        app: &AppContext,
    ) -> Box<dyn Element> {
        let (running, unsaved) = self.tab_close_hazards(key, app);
        let body = if running && unsaved {
            "This tab has a running terminal and unsaved editor changes."
        } else if running {
            "A process is still running in this tab's terminal."
        } else if unsaved {
            "This tab has an editor with unsaved changes."
        } else {
            "Close this tab and its panes?"
        };
        let col = Flex::column()
            .with_child(
                Text::new("Close this tab?".to_string(), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(body.to_string(), self.ui_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_child(Self::spacer(16.0))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
                    .with_child(self.modal_button(
                        "Cancel",
                        ModalBtn::Plain,
                        CraneShellAction::CloseModal,
                    ))
                    .with_child(Self::spacer(8.0))
                    .with_child(self.modal_button(
                        "Close Tab",
                        ModalBtn::Danger,
                        CraneShellAction::CloseTabConfirmed(key),
                    ))
                    .finish(),
            )
            .finish();
        self.modal_card(380.0, col)
    }

    /// `(has_running_terminal, has_unsaved_editor)` across a tab's panes —
    /// drives whether closing it needs a confirm and what the card says.
    fn tab_close_hazards(
        &self,
        key: (usize, usize, usize),
        app: &AppContext,
    ) -> (bool, bool) {
        let Some(node) = self.layouts.get(&key) else {
            return (false, false);
        };
        let mut leaves = Vec::new();
        node.leaves(&mut leaves);
        let mut running = false;
        let mut unsaved = false;
        for id in leaves {
            if let Some(h) = self.terminal_at(id) {
                if h.as_ref(app).has_foreground_process() {
                    running = true;
                }
            }
            if let Some(h) = self.editor_at(id) {
                if h.as_ref(app).is_dirty(app) {
                    unsaved = true;
                }
            }
        }
        (running, unsaved)
    }

    /// Help / keyboard cheat-sheet card — a 2-column chord → description grid.
    fn help_card(&self) -> Box<dyn Element> {
        const ROWS: &[(&str, &str)] = &[
            ("Cmd+O", "Open external file"),
            ("Cmd+Shift+O", "Add project folder"),
            ("Cmd+T", "Split active pane with a terminal"),
            ("Cmd+Shift+U", "Split active pane with a browser"),
            ("Cmd+Shift+T", "New tab in active workspace"),
            ("Cmd+D", "Split pane side-by-side"),
            ("Cmd+Shift+D", "Split pane stacked"),
            ("Cmd+W", "Close focused pane / file tab"),
            ("Cmd+Shift+W", "Close active tab"),
            ("Cmd+[  /  Cmd+]", "Focus previous / next pane"),
            ("Cmd+B", "Toggle Left Panel"),
            ("Cmd+/", "Comment line / toggle Right Panel"),
            ("Cmd+Shift+N", "Open Welcome pane"),
            ("Cmd+9", "Toggle Git Log dock"),
            ("Cmd+K", "Terminal: clear screen + scrollback"),
            ("Cmd+S", "Save focused file"),
            ("Cmd+A", "Select all"),
            ("Cmd+Z  /  Cmd+Shift+Z", "Undo / redo"),
            ("Cmd+C  /  Cmd+X  /  Cmd+V", "Copy / cut / paste"),
            ("Cmd+F", "Find in file"),
            ("Cmd+H", "Replace in file"),
            ("Cmd+G", "Go to line"),
            ("Cmd+=  /  Cmd+-  /  Cmd+0", "Font zoom in / out / reset"),
            ("Cmd+Opt+W", "Toggle editor word-wrap"),
            ("Escape", "Close this dialog"),
        ];
        let mut grid = Flex::column();
        for (chord, desc) in ROWS {
            let row = Flex::row()
                .with_child(
                    ConstrainedBox::new(
                        Text::new(chord.to_string(), self.ui_font, 12.0)
                            .with_color(theme::accent())
                            .finish(),
                    )
                    .with_width(200.0)
                    .finish(),
                )
                .with_child(
                    Text::new(desc.to_string(), self.ui_font, 12.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .finish();
            grid = grid.with_child(
                Container::new(row)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .finish(),
            );
        }
        let scrolled = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.modal_scroll.clone(),
                grid.finish(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish(),
        )
        .with_height(420.0)
        .finish();
        let col = Flex::column()
            .with_child(self.modal_header("Keyboard Shortcuts"))
            .with_child(Self::spacer(6.0))
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_height(1.0)
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(scrolled)
            .finish();
        self.modal_card(480.0, col)
    }

    /// Settings > About: the actionable auto-update row. Reads the updater's
    /// state accessor fresh every render (the shell re-renders on the update
    /// waker fed to `spawn_check` / `start_download`), so the lifecycle —
    /// available → downloading → ready → restart — surfaces live here.
    fn update_row(&self) -> Box<dyn Element> {
        use crate::warpui::update::{self, UpdateState};
        match update::update_state() {
            UpdateState::UpdateAvailable { version } => Flex::column()
                .with_child(
                    Text::new(format!("Update available: {version}"), self.ui_font, 12.0)
                        .with_color(theme::accent())
                        .finish(),
                )
                .with_child(Self::spacer(6.0))
                .with_child(self.modal_button(
                    "Download & Install",
                    ModalBtn::Primary,
                    CraneShellAction::StartUpdateDownload,
                ))
                .finish(),
            UpdateState::Downloading { received, total } => {
                let label = if total > 0 {
                    let pct = ((received.saturating_mul(100)) / total).min(100);
                    format!("Downloading… {pct}%")
                } else {
                    format!("Downloading… {} KB", received / 1024)
                };
                Text::new(label, self.ui_font, 12.0)
                    .with_color(theme::text())
                    .finish()
            }
            UpdateState::Ready { path } => Flex::column()
                .with_child(
                    Text::new("Update ready to install.".to_string(), self.ui_font, 12.0)
                        .with_color(theme::accent())
                        .finish(),
                )
                .with_child(Self::spacer(6.0))
                .with_child(self.modal_button(
                    "Install & Restart",
                    ModalBtn::Primary,
                    CraneShellAction::ApplyUpdate(path),
                ))
                .finish(),
            UpdateState::Failed { msg } => Flex::column()
                .with_child(
                    Text::new(format!("Update failed: {msg}"), self.ui_font, 11.0)
                        .with_color(theme::error())
                        .finish(),
                )
                .with_child(Self::spacer(6.0))
                .with_child(self.modal_button(
                    "Retry",
                    ModalBtn::Plain,
                    CraneShellAction::StartUpdateDownload,
                ))
                .finish(),
            UpdateState::Checking => Text::new(
                "Checking for updates…".to_string(),
                self.ui_font,
                11.0,
            )
            .with_color(theme::text_muted())
            .finish(),
            UpdateState::Idle => Text::new("Up to date".to_string(), self.ui_font, 11.0)
                .with_color(theme::text_muted())
                .finish(),
        }
    }

    /// Settings card — Appearance (theme picker + zoom) + About (version).
    /// Generic Settings bordered-checkbox toggle row (LSP / format-on-save /
    /// word-wrap / trim-on-save all share this shape): accent-filled CHECK box,
    /// title + On/Off tag, muted hint line, row hover, click dispatches.
    fn settings_toggle_row(
        &self,
        key: &str,
        title: &'static str,
        hint: &'static str,
        on: bool,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        let state = self.hover_handle(key);
        Hoverable::new(state, move |ms| {
            let row_bg = if ms.is_hovered() {
                theme::row_hover()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let check_inner: Box<dyn Element> = if on {
                Text::new(icons::CHECK.to_string(), icon_font, 11.0)
                    .with_color(ColorU::new(255, 255, 255, 255))
                    .finish()
            } else {
                Rect::new().finish()
            };
            let check_bg = if on {
                theme::accent()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let checkbox = ConstrainedBox::new(
                Container::new(check_inner)
                    .with_background_color(check_bg)
                    .with_border(Border::all(1.0).with_border_color(theme::border()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
            )
            .with_width(16.0)
            .with_height(16.0)
            .finish();
            let title_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new(title.to_string(), ui_font, 12.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_child(Self::spacer(8.0))
                .with_child(
                    Text::new(
                        if on { "On".to_string() } else { "Off".to_string() },
                        ui_font,
                        11.0,
                    )
                    .with_color(if on { theme::accent() } else { theme::text_muted() })
                    .finish(),
                )
                .finish();
            let text_col = Flex::column()
                .with_child(title_row)
                .with_child(Self::spacer(2.0))
                .with_child(
                    Text::new(hint.to_string(), ui_font, 10.5)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .finish();
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(checkbox)
                    .with_child(Self::spacer(10.0))
                    .with_child(text_col)
                    .finish(),
            )
            .with_background_color(row_bg)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }

    /// A `label   [−]  N pt  [+]` stepper row for one of the base font sizes.
    fn settings_font_row(&self, label: &'static str, value: f32, editor: bool) -> Box<dyn Element> {
        let step = |glyph: &str, delta: f32| -> Box<dyn Element> {
            EventHandler::new(
                Container::new(
                    Text::new(glyph.to_string(), self.icon_font, 11.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FontBaseStep { editor, delta });
                DispatchEventResult::StopPropagation
            })
            .finish()
        };
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(label.to_string(), self.ui_font, 12.0)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
            .with_child(step(icons::MINUS, -1.0))
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(format!("{value:.0} pt"), self.ui_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_child(Self::spacer(8.0))
            .with_child(step(icons::PLUS, 1.0))
            .finish()
    }

    /// Muted section heading inside a Settings body.
    fn settings_heading(&self, label: &'static str) -> Box<dyn Element> {
        Container::new(
            Text::new(label.to_string(), self.ui_font, 11.0)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_padding_top(10.0)
        .with_padding_bottom(4.0)
        .finish()
    }

    /// Settings > Appearance: theme rows with color swatches, zoom + base font
    /// sizes, syntax-theme override, themes-folder shortcut.
    fn settings_appearance(&self) -> Box<dyn Element> {
        let current_theme = crate::theme::current().name;
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        col = col.with_child(self.settings_heading("Theme"));
        for t in crate::theme::load_all() {
            let is_active = t.name == current_theme;
            let name = t.name.clone();
            // Five color swatches preview the theme (old settings.rs swatch row).
            let mut swatches = Flex::row();
            for c in [t.bg, t.surface, t.accent, t.text, t.surface_hi] {
                swatches = swatches.with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Rect::new().with_background_color(c.to_warp()).finish(),
                        )
                        .with_width(12.0)
                        .with_height(12.0)
                        .finish(),
                    )
                    .with_padding_right(4.0)
                    .finish(),
                );
            }
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(swatches.finish())
                .with_child(Self::spacer(8.0))
                .with_child(
                    Text::new(name.clone(), self.ui_font, 12.5)
                        .with_color(if is_active { theme::text() } else { theme::text_muted() })
                        .finish(),
                );
            row = row.with_child(Expanded::new(1.0, Flex::row().finish()).finish());
            if is_active {
                row = row.with_child(self.icon(icons::CHECK, 12.0, theme::accent()));
            }
            let row = EventHandler::new(
                Container::new(row.finish())
                    .with_background_color(if is_active {
                        theme::row_active()
                    } else {
                        ColorU::new(0, 0, 0, 0)
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::SetTheme(name.clone()));
                DispatchEventResult::StopPropagation
            })
            .finish();
            col = col.with_child(row);
        }
        // Themes folder hint + open shortcut.
        let themes_dir = crate::theme::themes_dir();
        col = col.with_child(Self::spacer(6.0)).with_child(
            Text::new(
                format!("Custom themes (.toml) live in {}", themes_dir.display()),
                self.ui_font,
                10.5,
            )
            .with_color(theme::text_muted())
            .finish(),
        );
        col = col.with_child(Self::spacer(4.0)).with_child(
            EventHandler::new(
                Container::new(
                    Text::new("Open themes folder".to_string(), self.ui_font, 11.0)
                        .with_color(theme::accent())
                        .finish(),
                )
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .finish(),
            )
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::OpenThemesFolder);
                DispatchEventResult::StopPropagation
            })
            .finish(),
        );

        col = col.with_child(self.settings_heading("Fonts"));
        let zoom_pct = (crate::warpui::fontsize::zoom_level() * 100.0).round() as i32;
        col = col.with_child(
            Text::new(
                format!("Zoom: {zoom_pct}%   (Cmd+= / Cmd+- / Cmd+0)"),
                self.ui_font,
                11.0,
            )
            .with_color(theme::text_muted())
            .finish(),
        );
        col = col.with_child(Self::spacer(8.0)).with_child(self.settings_font_row(
            "Terminal font size",
            crate::warpui::fontsize::base(),
            false,
        ));
        col = col.with_child(Self::spacer(6.0)).with_child(self.settings_font_row(
            "Editor font size",
            crate::warpui::fontsize::editor(),
            true,
        ));

        col = col.with_child(self.settings_heading("Syntax highlighting"));
        // "Auto" row + every installed syntect theme; the active row checks.
        let auto_active = self.syntax_override.is_none();
        let auto_label = format!("Auto (pair with UI theme: {})", crate::theme::current().syntax_theme);
        col = col.with_child(self.syntax_theme_row(auto_label, auto_active, None));
        for name in crate::syntax::theme_names() {
            let active = self.syntax_override.as_deref() == Some(name.as_str());
            col = col.with_child(self.syntax_theme_row(name.clone(), active, Some(name)));
        }
        col.finish()
    }

    /// One clickable row in the syntax-theme override list.
    fn syntax_theme_row(
        &self,
        label: String,
        active: bool,
        value: Option<String>,
    ) -> Box<dyn Element> {
        let mut row = Flex::row().with_child(
            ConstrainedBox::new(if active {
                self.icon(icons::CHECK, 11.0, theme::accent())
            } else {
                Flex::row().finish()
            })
            .with_width(18.0)
            .finish(),
        );
        row = row.with_child(
            Text::new(label, self.ui_font, 11.5)
                .with_color(if active { theme::text() } else { theme::text_muted() })
                .finish(),
        );
        EventHandler::new(
            Container::new(row.finish())
                .with_padding_left(6.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::SetSyntaxOverride(value.clone()));
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    /// Settings > Editor: behavior toggles.
    fn settings_editor(&self) -> Box<dyn Element> {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.settings_toggle_row(
                "settings:wrap_toggle",
                "Word wrap",
                "Soft-wrap long lines to the pane width. Cmd+Opt+W still toggles per file.",
                self.word_wrap_default,
                CraneShellAction::ToggleWordWrapDefault,
            ))
            .with_child(Self::spacer(6.0))
            .with_child(self.settings_toggle_row(
                "settings:trim_toggle",
                "Trim trailing whitespace on save",
                "Strips spaces/tabs at line ends before every write.",
                self.trim_on_save,
                CraneShellAction::ToggleTrimOnSave,
            ))
            .with_child(Self::spacer(6.0))
            .with_child(self.settings_toggle_row(
                "settings:format_toggle",
                "Format on save",
                "Cmd+S runs rustfmt / prettier / ruff / gofmt off-thread when installed. \
                 A formatter failure never corrupts the file.",
                self.format_on_save,
                CraneShellAction::ToggleFormatOnSave,
            ))
            .finish()
    }

    /// Settings > Terminal: placeholder — 1:1 with the old dialog, which also
    /// only promised shell/cursor/scrollback prefs "will land here".
    fn settings_terminal(&self) -> Box<dyn Element> {
        Container::new(
            Text::new(
                "Shell override, cursor style and scrollback size will land here."
                    .to_string(),
                self.ui_font,
                11.5,
            )
            .with_color(theme::text_muted())
            .finish(),
        )
        .with_padding_top(6.0)
        .finish()
    }

    /// Settings > Language Servers: the opt-in toggle + a live status line per
    /// running server (old settings_lsp.rs, minus the per-server install UI).
    fn settings_lsp(&self) -> Box<dyn Element> {
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.settings_toggle_row(
                "settings:lsp_toggle",
                "Language Server (LSP)",
                "Off by default — the agent handles code intelligence. \
                 Spawns rust-analyzer etc. when on.",
                self.lsp_enabled,
                CraneShellAction::ToggleLsp,
            ));
        if self.lsp_enabled {
            col = col.with_child(self.settings_heading("Servers"));
            let statuses = self.lsp.statuses();
            if statuses.is_empty() {
                col = col.with_child(
                    Text::new(
                        "No servers running yet — one starts when a matching file opens."
                            .to_string(),
                        self.ui_font,
                        11.0,
                    )
                    .with_color(theme::text_muted())
                    .finish(),
                );
            }
            for (key, status) in statuses {
                col = col.with_child(
                    Container::new(
                        Text::new(
                            format!("{key:?} — {status:?}"),
                            self.ui_font,
                            11.0,
                        )
                        .with_color(theme::text_muted())
                        .finish(),
                    )
                    .with_padding_top(2.0)
                    .finish(),
                );
            }
        }
        col.finish()
    }

    /// Settings > Shortcuts: the canonical chord table (old settings.rs list).
    fn settings_shortcuts(&self) -> Box<dyn Element> {
        const ROWS: &[(&str, &str)] = &[
            ("Cmd+T", "Split active Pane with new terminal"),
            ("Cmd+Shift+U", "Split active pane with a browser"),
            ("Cmd+Shift+T", "New Tab in active Workspace"),
            ("Cmd+D / Cmd+Shift+D", "Split Pane side-by-side / stacked"),
            ("Cmd+W / Cmd+Shift+W", "Close focused Pane / active Tab"),
            ("Cmd+[ / Cmd+]", "Focus prev / next Pane"),
            ("Cmd+B / Cmd+/", "Toggle Left / Right Panel"),
            ("Cmd+Shift+B", "Switch Branch"),
            ("Cmd+= / Cmd+- / Cmd+0", "Zoom in / out / reset"),
            ("Cmd+S", "Save file (formats when Format-on-save is on)"),
            ("Cmd+F / Cmd+H / Cmd+G", "Find / Replace / Goto line in editor"),
            ("Cmd+Shift+F", "Find in Files (project-wide)"),
            ("Cmd+`", "Tab switcher (Shift or ~ steps back)"),
            ("Cmd+K", "Terminal: clear screen + scrollback"),
            ("Cmd+Opt+W", "Toggle editor word-wrap (per file)"),
            ("Cmd+Opt+T", "Browser: new tab (opens a Browser Pane)"),
            ("Shift+Tab", "Terminal: back-tab (CSI Z) for TUIs"),
            ("F12", "LSP goto definition at caret"),
            ("Escape", "Close modal / menu / restore maximized pane"),
        ];
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (chord, desc) in ROWS {
            col = col.with_child(
                Container::new(
                    Flex::row()
                        .with_child(
                            ConstrainedBox::new(
                                Text::new(chord.to_string(), self.mono_font, 11.0)
                                    .with_color(theme::text())
                                    .finish(),
                            )
                            .with_width(190.0)
                            .finish(),
                        )
                        .with_child(
                            Text::new(desc.to_string(), self.ui_font, 11.0)
                                .with_color(theme::text_muted())
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
            );
        }
        // Full keyboard reference — opens the standalone Help modal (a superset
        // of the rows above). Opening it replaces the Settings modal (single-modal
        // system), which is acceptable.
        col = col.with_child(Self::spacer(10.0)).with_child(
            EventHandler::new(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(self.icon(icons::KEYBOARD, 12.0, theme::accent()))
                        .with_child(Self::spacer(6.0))
                        .with_child(
                            Text::new(
                                "Open full keyboard reference".to_string(),
                                self.ui_font,
                                11.5,
                            )
                            .with_color(theme::accent())
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .finish(),
            )
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::OpenHelp);
                DispatchEventResult::StopPropagation
            })
            .finish(),
        );
        col.finish()
    }

    /// Settings > About: version, tagline, updater lifecycle, project links,
    /// manual re-check.
    fn settings_about(&self) -> Box<dyn Element> {
        let link = |label: &'static str, url: &'static str| -> Box<dyn Element> {
            EventHandler::new(
                Container::new(
                    Text::new(label.to_string(), self.ui_font, 11.5)
                        .with_color(theme::accent())
                        .finish(),
                )
                .with_padding_right(14.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::OpenUrl(url.to_string()));
                DispatchEventResult::StopPropagation
            })
            .finish()
        };
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Text::new(format!("Crane {}", env!("CARGO_PKG_VERSION")), self.ui_font, 14.0)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_child(Self::spacer(4.0))
            .with_child(
                Text::new(
                    "Native GPU-rendered development environment.".to_string(),
                    self.ui_font,
                    11.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(Self::spacer(10.0))
            .with_child(self.update_row())
            .with_child(Self::spacer(12.0))
            .with_child(
                Flex::row()
                    .with_child(link("GitHub", "https://github.com/rajpootathar/Crane"))
                    .with_child(link(
                        "Releases",
                        "https://github.com/rajpootathar/Crane/releases",
                    ))
                    .with_child(
                        EventHandler::new(
                            Container::new(
                                Text::new(
                                    "Check for updates".to_string(),
                                    self.ui_font,
                                    11.5,
                                )
                                .with_color(theme::accent())
                                .finish(),
                            )
                            .finish(),
                        )
                        .on_left_mouse_down(|ctx, _app, _pos| {
                            ctx.dispatch_typed_action(CraneShellAction::UpdateCheckNow);
                            DispatchEventResult::StopPropagation
                        })
                        .finish(),
                    )
                    .finish(),
            )
            .finish()
    }

    /// The Settings dialog: a 6-section sidebar (old modals/settings.rs) beside
    /// the active section's scrollable body.
    fn settings_card(&self) -> Box<dyn Element> {
        // Sidebar — one row per section, active row highlighted.
        let mut side = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        side = side.with_child(
            Container::new(
                Text::new("Settings".to_string(), self.ui_font, 15.0)
                    .with_color(theme::text_header())
                    .finish(),
            )
            .with_padding_bottom(10.0)
            .finish(),
        );
        for section in SettingsSection::ALL {
            let active = section == self.settings_section;
            let row = EventHandler::new(
                Container::new(
                    Text::new(section.title().to_string(), self.ui_font, 12.0)
                        .with_color(if active { theme::text() } else { theme::text_muted() })
                        .finish(),
                )
                .with_background_color(if active {
                    theme::row_active()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::SettingsGoto(section));
                DispatchEventResult::StopPropagation
            })
            .finish();
            side = side.with_child(Container::new(row).with_padding_bottom(2.0).finish());
        }
        let sidebar = ConstrainedBox::new(
            Container::new(side.finish()).with_padding_right(10.0).finish(),
        )
        .with_width(150.0)
        .finish();

        let body = match self.settings_section {
            SettingsSection::Appearance => self.settings_appearance(),
            SettingsSection::Editor => self.settings_editor(),
            SettingsSection::Terminal => self.settings_terminal(),
            SettingsSection::LanguageServers => self.settings_lsp(),
            SettingsSection::Shortcuts => self.settings_shortcuts(),
            SettingsSection::About => self.settings_about(),
        };
        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new(
                        self.settings_section.title().to_string(),
                        self.ui_font,
                        13.0,
                    )
                    .with_color(theme::text_header())
                    .finish(),
                )
                .with_padding_bottom(6.0)
                .finish(),
            )
            .with_child(body)
            .finish();
        let body_scrolled = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.settings_scroll.clone(),
                Container::new(body).with_padding_right(6.0).finish(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish(),
        )
        .with_height(430.0)
        .finish();

        let row = Flex::row()
            .with_child(sidebar)
            .with_child(
                // Height-bound the divider: a bare Rect has no intrinsic size,
                // and the modal column imposes no height constraint — an
                // unbounded Rect here panics warpui's scene ("y is_infinite",
                // the known modal Expanded/Rect crash).
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_width(1.0)
                    .with_height(430.0)
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Container::new(body_scrolled).with_padding_left(12.0).finish(),
                )
                .finish(),
            )
            .finish();
        self.modal_card(640.0, Flex::column().with_child(row).finish())
    }

    /// Find-in-Files card — Cmd+Shift+F. A read-only query field (keys route in
    /// via `edit_find_in_files`, like the commit box), a status line, and a
    /// scrollable list of matches grouped by file. Clicking a match opens the
    /// file at that line. Port of the old egui `src/modals/find_in_files.rs`
    /// (simplified: synchronous substring search, no regex/case/scope options).
    fn find_in_files_card(&self) -> Box<dyn Element> {
        let Some(st) = self.find_in_files.as_ref() else {
            return self.modal_card(640.0, Flex::column().finish());
        };
        // Query field — mirrors the commit box's editable-looking Text field.
        let (qtext, qcolor) = if st.query.is_empty() {
            ("Search files in the active project…".to_string(), theme::text_muted())
        } else {
            (format!("{}|", st.query), theme::text())
        };
        let query_field = Container::new(
            Flex::row()
                .with_child(self.icon(icons::MAGNIFYING_GLASS, 13.0, theme::text_muted()))
                .with_child(Self::spacer(8.0))
                .with_child(Text::new(qtext, self.ui_font, 13.0).with_color(qcolor).finish())
                .finish(),
        )
        .with_background_color(theme::row_active())
        .with_border(Border::all(1.0).with_border_color(theme::border()))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .finish();

        // Status line: match count / cap notice / empty-query hint.
        let status = if st.query.trim().is_empty() {
            "Type to search across the active project".to_string()
        } else if st.results.is_empty() {
            "No matches".to_string()
        } else if st.truncated {
            format!("{}+ matches (capped at {})", st.results.len(), FIF_MAX_RESULTS)
        } else {
            format!("{} matches", st.results.len())
        };

        // Result list — grouped by file. A non-clickable file header precedes
        // each file's clickable match rows; the selected row is highlighted.
        let mut list = Flex::column();
        let mut last_display: Option<&str> = None;
        for (i, m) in st.results.iter().enumerate() {
            if last_display != Some(m.display.as_str()) {
                last_display = Some(m.display.as_str());
                list = list.with_child(
                    Container::new(
                        Text::new(m.display.clone(), self.ui_font, 11.5)
                            .with_color(theme::accent())
                            .finish(),
                    )
                    .with_padding_left(4.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(2.0)
                    .finish(),
                );
            }
            let is_sel = i == st.selected;
            let path = m.path.clone();
            let line = m.line;
            let row_text = format!("{}:  {}", m.line, m.text);
            let mut bg = Rect::new();
            if is_sel {
                bg = bg.with_background_color(theme::row_active());
            }
            let bg_layer = ConstrainedBox::new(bg.finish()).with_height(20.0).finish();
            let label = Container::new(
                Text::new(row_text, self.ui_font, 12.0)
                    .with_color(if is_sel { theme::text() } else { theme::text_muted() })
                    .finish(),
            )
            .with_padding_left(18.0)
            .with_padding_top(3.0)
            .finish();
            let row = EventHandler::new(
                Stack::new().with_child(bg_layer).with_child(label).finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::OpenFifMatch {
                    path: path.clone(),
                    line,
                });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }
        let scrolled = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.find_scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish(),
        )
        .with_height(360.0)
        .finish();

        let col = Flex::column()
            .with_child(self.modal_header("Find in Files"))
            .with_child(Self::spacer(8.0))
            .with_child(query_field)
            .with_child(Self::spacer(6.0))
            .with_child(
                Text::new(status, self.ui_font, 11.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_child(Self::spacer(6.0))
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_height(1.0)
                    .finish(),
            )
            .with_child(Self::spacer(4.0))
            .with_child(scrolled)
            .with_child(Self::spacer(6.0))
            .with_child(
                Text::new(
                    "Enter opens · Up/Down to move · Esc closes".to_string(),
                    self.ui_font,
                    10.5,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .finish();
        self.modal_card(640.0, col)
    }

    /// Tab-switcher card — Cmd+`. Lists every tab in the active workspace and
    /// highlights the row that Enter (or another Cmd+`) will activate. Escape
    /// cancels. NOTE(simplification): the old egui build committed on Cmd
    /// *release* (alt-tab muscle memory); warpui does not surface a reliable
    /// modifier-release event to the shell, so this uses an explicit
    /// Enter-to-commit list instead (Cmd+` / Up / Down move the highlight).
    fn tab_switcher_card(&self) -> Box<dyn Element> {
        let Some(st) = self.tab_switcher.as_ref() else {
            return self.modal_card(460.0, Flex::column().finish());
        };
        let mut list = Flex::column();
        for (i, (pi, wi, tid)) in st.entries.iter().enumerate() {
            let is_hl = i == st.highlight;
            let label = self.switcher_label(*pi, *wi, *tid);
            let key = (*pi, *wi, *tid);
            let path = self
                .projects
                .get(*pi)
                .and_then(|p| p.worktrees.get(*wi))
                .map(|w| PathBuf::from(&w.path))
                .unwrap_or_default();
            let mut bg = Rect::new();
            if is_hl {
                bg = bg.with_background_color(theme::row_active());
            }
            let bg_layer = ConstrainedBox::new(bg.finish()).with_height(24.0).finish();
            let inner = Container::new(
                Text::new(label, self.ui_font, 12.5)
                    .with_color(if is_hl { theme::text() } else { theme::text_muted() })
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_top(5.0)
            .finish();
            let row = EventHandler::new(
                Stack::new().with_child(bg_layer).with_child(inner).finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::ActivateSwitcherTab {
                    key,
                    path: path.clone(),
                });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }
        let scrolled = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.switcher_scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish(),
        )
        .with_height(320.0)
        .finish();
        let col = Flex::column()
            .with_child(self.modal_header("Switch Tab"))
            .with_child(Self::spacer(6.0))
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_height(1.0)
                    .finish(),
            )
            .with_child(Self::spacer(6.0))
            .with_child(scrolled)
            .with_child(Self::spacer(6.0))
            .with_child(
                Text::new(
                    "Cmd+` next · Cmd+Shift+` prev · Enter activates · Esc cancels".to_string(),
                    self.ui_font,
                    10.5,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .finish();
        self.modal_card(460.0, col)
    }

    /// "Switch Branch" card — a search field over local + remote branches. Each
    /// row checks the branch out in the active workspace; a trailing "+ worktree"
    /// button opens the New-Workspace modal pre-filled with that branch. A typed
    /// query that matches no branch surfaces a "Create new branch …" row.
    fn switch_branch_card(&self) -> Box<dyn Element> {
        let Some(st) = self.switch_branch.as_ref() else {
            return self.modal_card(520.0, Flex::column().finish());
        };
        let q = st.query.trim().to_lowercase();
        let filtered: Vec<String> = st
            .all
            .iter()
            .filter(|b| q.is_empty() || b.to_lowercase().contains(&q))
            .cloned()
            .collect();
        let exact = st.all.iter().any(|b| b.to_lowercase() == q);

        // Search field (mirrors Find-in-Files).
        let (qtext, qcolor) = if st.query.is_empty() {
            ("Search branches…".to_string(), theme::text_muted())
        } else {
            (format!("{}|", st.query), theme::text())
        };
        let query_field = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(self.icon(icons::MAGNIFYING_GLASS, 13.0, theme::text_muted()))
                .with_child(Self::spacer(8.0))
                .with_child(Text::new(qtext, self.ui_font, 13.0).with_color(qcolor).finish())
                .finish(),
        )
        .with_background_color(theme::sidebar_bg())
        .with_border(Border::all(1.0).with_border_color(theme::border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(5.5)
        .with_padding_bottom(5.5)
        .finish();

        let pi = st.project_idx;
        // Row height + how many rows before the list starts scrolling. The body
        // is min(content, cap): short lists draw their natural height (no dead
        // space), long lists cap at ROW_CAP rows and scroll.
        const ROW_H: f32 = 30.0;
        const ROW_CAP: usize = 12;
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        let mut row_count = 0usize;
        for (i, b) in filtered.iter().enumerate() {
            let is_current = *b == self.branch;
            let is_sel = i == st.selected;
            let is_local = st.locals.contains(b);
            let glyph = if is_current { icons::CHECK } else { icons::GIT_BRANCH };
            let color = if is_current { theme::accent() } else { theme::text() };
            let branch = b.clone();
            let label = b.clone();
            let wt_branch = b.clone();
            let ui_font = self.ui_font;
            let icon_font = self.icon_font;
            let state = self.hover_handle(&format!("sb:{b}"));
            // The WHOLE row is one Hoverable: click = checkout; the "+ worktree"
            // affordance is rendered ONLY while the row is hovered (a nested
            // EventHandler that stops propagation so it never triggers checkout).
            // Keyboard users reach worktrees via the footer-documented flow.
            let row = Hoverable::new(state, move |ms| {
                let hovered = ms.is_hovered();
                let bg = if hovered || is_sel {
                    theme::hover_wash()
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let mut left = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Container::new(
                            Text::new(glyph.to_string(), icon_font, 12.0)
                                .with_color(color)
                                .finish(),
                        )
                        .with_padding_right(8.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new(label.clone(), ui_font, 12.5).with_color(color).finish(),
                    );
                if !is_local {
                    // Small muted "remote" chip.
                    left = left.with_child(Self::spacer(8.0)).with_child(
                        Container::new(
                            Text::new("remote".to_string(), ui_font, 10.0)
                                .with_color(theme::text_muted())
                                .finish(),
                        )
                        .with_background_color(theme::surface())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(1.0)
                        .with_padding_bottom(1.0)
                        .finish(),
                    );
                }
                let mut inner = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(Expanded::new(1.0, left.finish()).finish());
                if hovered {
                    let wt = wt_branch.clone();
                    let reveal = EventHandler::new(
                        Container::new(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_child(
                                    Text::new(icons::PLUS.to_string(), icon_font, 11.0)
                                        .with_color(theme::text_muted())
                                        .finish(),
                                )
                                .with_child(Self::spacer(4.0))
                                .with_child(
                                    Text::new("worktree".to_string(), ui_font, 11.0)
                                        .with_color(theme::text_muted())
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(3.0)
                        .with_padding_bottom(3.0)
                        .finish(),
                    )
                    .on_left_mouse_down(move |ctx, _app, _pos| {
                        ctx.dispatch_typed_action(CraneShellAction::OpenNewWorkspace {
                            pi,
                            branch: Some(wt.clone()),
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .with_always_handle()
                    .finish();
                    inner = inner.with_child(reveal).with_child(Self::spacer(4.0));
                }
                Container::new(inner.finish())
                    .with_background_color(bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .with_padding_left(10.0)
                    .with_padding_right(6.0)
                    .with_padding_top(6.5)
                    .with_padding_bottom(6.5)
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::CloseModal);
                ctx.dispatch_typed_action(CraneShellAction::CheckoutBranch(branch.clone()));
            })
            .finish();
            list = list.with_child(row);
            row_count += 1;
        }
        // "Create new branch" row when the query names no existing branch.
        if !q.is_empty() && !exact {
            let new_name = st.query.trim().to_string();
            list = list.with_child(self.create_branch_row(new_name));
            row_count += 1;
        }
        if filtered.is_empty() && q.is_empty() {
            list = list.with_child(
                Container::new(
                    Text::new("(no branches)".to_string(), self.ui_font, 12.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_uniform_padding(8.0)
                .finish(),
            );
            row_count += 1;
        }
        // Body height fits content: draw naturally up to ROW_CAP rows, then cap
        // and scroll. The ClippedScrollable keeps a stable scroll handle.
        let list = list.finish();
        let content_h = row_count.max(1) as f32 * ROW_H;
        let scrolled: Box<dyn Element> = if row_count > ROW_CAP {
            ConstrainedBox::new(
                ClippedScrollable::vertical(
                    self.switch_branch_scroll.clone(),
                    list,
                    ScrollbarWidth::Auto,
                    Fill::Solid(theme::border()),
                    Fill::Solid(theme::text_muted()),
                    Fill::None,
                )
                .finish(),
            )
            .with_height(ROW_CAP as f32 * ROW_H)
            .finish()
        } else {
            ConstrainedBox::new(list).with_height(content_h).finish()
        };
        let col = Flex::column()
            .with_child(self.modal_header("Switch Branch"))
            .with_child(Self::spacer(8.0))
            .with_child(query_field)
            .with_child(Self::spacer(8.0))
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_height(1.0)
                    .finish(),
            )
            .with_child(Self::spacer(4.0))
            .with_child(scrolled)
            .with_child(Self::spacer(6.0))
            .with_child(
                Text::new(
                    "Enter checks out · + worktree creates a workspace · Esc closes".to_string(),
                    self.ui_font,
                    10.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .finish();
        self.modal_card(520.0, col)
    }

    /// The "Create new branch '<name>'" row shown in the Switch-Branch modal.
    fn create_branch_row(&self, name: String) -> Box<dyn Element> {
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        let label = format!("Create new branch \"{name}\"");
        let state = self.hover_handle("sb:__create__");
        Hoverable::new(state, move |ms| {
            let bg = if ms.is_hovered() {
                theme::row_hover()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Container::new(
                            Text::new(icons::PLUS.to_string(), icon_font, 12.0)
                                .with_color(theme::accent())
                                .finish(),
                        )
                        .with_padding_right(8.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new(label.clone(), ui_font, 12.5)
                            .with_color(theme::accent())
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(bg)
            .with_padding_left(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::CreateBranchCheckout(name.clone()));
        })
        .finish()
    }

    /// "New Workspace" card — create a git worktree for a branch. A branch field,
    /// a "create new branch" toggle, the computed worktree path, and Create /
    /// Cancel buttons. Port of old egui `src/modals/new_workspace.rs`.
    fn new_workspace_card(&self) -> Box<dyn Element> {
        let Some(st) = self.new_workspace.as_ref() else {
            return self.modal_card(460.0, Flex::column().finish());
        };
        let project = self.projects.get(st.project_idx);
        let pname = project.map(|p| p.name.clone()).unwrap_or_default();

        let branch_caret = if st.path_focused || st.branch_locked { "" } else { "|" };
        let (btext, bcolor) = if st.branch.is_empty() {
            (format!("branch name…{branch_caret}"), theme::text_muted())
        } else {
            (format!("{}{branch_caret}", st.branch), theme::text())
        };
        let branch_field = EventHandler::new(
            Container::new(
                Flex::row()
                    .with_child(self.icon(icons::GIT_BRANCH, 13.0, theme::text_muted()))
                    .with_child(Self::spacer(8.0))
                    .with_child(Text::new(btext, self.ui_font, 13.0).with_color(bcolor).finish())
                    .finish(),
            )
            .with_background_color(theme::row_active())
            .with_border(Border::all(1.0).with_border_color(
                if st.path_focused || st.branch_locked {
                    theme::border()
                } else {
                    theme::accent()
                },
            ))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceFocusPath(false));
            DispatchEventResult::StopPropagation
        })
        .finish();

        // "Create new branch" toggle row (a small checkbox box + label). Built
        // from a bordered square (filled accent + CHECK when on) rather than a
        // dedicated glyph so it never depends on an unbundled phosphor codepoint.
        let checkbox = {
            let inner: Box<dyn Element> = if st.new_branch {
                self.icon(icons::CHECK, 11.0, ColorU::new(255, 255, 255, 255))
            } else {
                Rect::new().finish()
            };
            let bg = if st.new_branch {
                theme::accent()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            ConstrainedBox::new(
                Container::new(inner)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(theme::border()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
            )
            .with_width(16.0)
            .with_height(16.0)
            .finish()
        };
        let toggle = EventHandler::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(checkbox)
                    .with_child(Self::spacer(8.0))
                    .with_child(
                        Text::new(
                            "Create as a new branch".to_string(),
                            self.ui_font,
                            12.0,
                        )
                        .with_color(theme::text())
                        .finish(),
                    )
                    .finish(),
            )
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceToggleNewBranch);
            DispatchEventResult::StopPropagation
        })
        .finish();

        // Location mode selector (old modal's Global / Project-local / Custom).
        let mode_pill = |label: &'static str, mode: LocationMode, hint: &'static str| -> Box<dyn Element> {
            let active = st.mode == mode;
            let _ = hint; // tooltips pending a warpui tooltip primitive
            EventHandler::new(
                Container::new(
                    Text::new(label.to_string(), self.ui_font, 11.5)
                        .with_color(if active { theme::text() } else { theme::text_muted() })
                        .finish(),
                )
                .with_background_color(if active {
                    theme::row_active()
                } else {
                    theme::surface()
                })
                .with_border(Border::all(1.0).with_border_color(if active {
                    theme::accent()
                } else {
                    theme::border()
                }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceSetMode(mode));
                DispatchEventResult::StopPropagation
            })
            .finish()
        };
        let location_row = Flex::row()
            .with_child(mode_pill(
                "Global",
                LocationMode::Global,
                "~/.crane-worktrees/<project>/<branch>",
            ))
            .with_child(Self::spacer(6.0))
            .with_child(mode_pill(
                "Project-local",
                LocationMode::ProjectLocal,
                "<project>/.crane-worktrees/<branch>",
            ))
            .with_child(Self::spacer(6.0))
            .with_child(mode_pill("Custom", LocationMode::Custom, "Pick any folder"))
            .finish();

        // Custom mode: editable parent-path field + Browse… (OS folder picker).
        let custom_row: Option<Box<dyn Element>> = (st.mode == LocationMode::Custom).then(|| {
            let caret = if st.path_focused { "|" } else { "" };
            let (ptext, pcolor) = if st.custom_path.is_empty() {
                (format!("/path/to/parent{caret}"), theme::text_muted())
            } else {
                (format!("{}{caret}", st.custom_path), theme::text())
            };
            let field = EventHandler::new(
                Container::new(
                    Text::new(ptext, self.ui_font, 12.0).with_color(pcolor).finish(),
                )
                .with_background_color(theme::row_active())
                .with_border(Border::all(1.0).with_border_color(if st.path_focused {
                    theme::accent()
                } else {
                    theme::border()
                }))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish(),
            )
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceFocusPath(true));
                DispatchEventResult::StopPropagation
            })
            .finish();
            let browse = EventHandler::new(
                Container::new(
                    Text::new("Browse…".to_string(), self.ui_font, 11.5)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
            )
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceBrowse);
                DispatchEventResult::StopPropagation
            })
            .finish();
            Flex::row()
                .with_child(Expanded::new(1.0, field).finish())
                .with_child(Self::spacer(6.0))
                .with_child(browse)
                .finish()
        });

        // Live target preview: mode-resolved parent + the (flattened) branch.
        let ppath = project.map(|p| p.path.clone()).unwrap_or_default();
        let parent = Self::resolved_worktree_parent(st.mode, &st.custom_path, &ppath, &pname);
        let shown_branch = if st.branch.is_empty() {
            "<branch>".to_string()
        } else {
            st.branch.replace('/', "-")
        };
        let path_text = format!(
            "→ {}/{shown_branch}",
            parent.display().to_string().trim_end_matches('/')
        );

        let mut col = Flex::column()
            .with_child(self.modal_header("New Workspace"))
            .with_child(Self::spacer(6.0))
            .with_child(
                Text::new(
                    format!("Project: {pname}"),
                    self.ui_font,
                    11.5,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(Self::spacer(10.0))
            .with_child(branch_field)
            .with_child(Self::spacer(8.0));
        if st.branch_locked {
            // Existing branch from the picker: the checkbox hides — the only
            // valid action is checking it out into a new worktree.
            col = col.with_child(
                Text::new(
                    "Checking out existing branch into a new worktree".to_string(),
                    self.ui_font,
                    11.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            );
        } else {
            col = col.with_child(toggle);
        }
        col = col
            .with_child(Self::spacer(10.0))
            .with_child(
                Text::new("Location".to_string(), self.ui_font, 11.5)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_child(Self::spacer(4.0))
            .with_child(location_row);
        if let Some(row) = custom_row {
            col = col.with_child(Self::spacer(6.0)).with_child(row);
        }
        col = col.with_child(Self::spacer(8.0)).with_child(
            Text::new(path_text, self.ui_font, 10.5)
                .with_color(theme::text_muted())
                .finish(),
        );
        if let Some(err) = &st.error {
            col = col.with_child(Self::spacer(6.0)).with_child(
                Text::new(err.clone(), self.ui_font, 11.0)
                    .with_color(theme::error())
                    .finish(),
            );
        }
        let buttons = Flex::row()
            .with_child(Expanded::new(1.0, ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish()).finish())
            .with_child(self.modal_button("Cancel", ModalBtn::Plain, CraneShellAction::CloseModal))
            .with_child(Self::spacer(8.0))
            .with_child(self.modal_button(
                "Create",
                ModalBtn::Primary,
                CraneShellAction::NewWorkspaceConfirm,
            ))
            .finish();
        col = col.with_child(Self::spacer(16.0)).with_child(buttons);
        self.modal_card(460.0, col.finish())
    }


    /// "project / workspace / tab" label for a tab-switcher entry.
    fn switcher_label(&self, pi: usize, wi: usize, tid: usize) -> String {
        let project = self.projects.get(pi).map(|p| p.name.clone()).unwrap_or_default();
        let ws = self
            .projects
            .get(pi)
            .and_then(|p| p.worktrees.get(wi))
            .map(|w| {
                self.worktree_names
                    .get(&w.path)
                    .cloned()
                    .unwrap_or_else(|| w.name.clone())
            })
            .unwrap_or_default();
        let tab = self
            .worktree_tabs
            .get(&(pi, wi))
            .and_then(|tabs| tabs.iter().find(|t| t.id == tid))
            .map(|t| t.name.clone())
            .unwrap_or_default();
        format!("{project} / {ws} / {tab}")
    }

    /// Run a synchronous, recursive, case-insensitive substring search over the
    /// active project's files and store the results. Called on every query edit.
    /// Skips `.git` / `target` / `node_modules`, oversized files, and non-UTF-8
    /// (binary) files. Capped at `FIF_MAX_RESULTS`.
    fn run_find_in_files(&mut self) {
        let Some(st) = self.find_in_files.as_mut() else { return };
        st.results.clear();
        st.truncated = false;
        st.selected = 0;
        let needle = st.query.trim().to_lowercase();
        if needle.is_empty() {
            return;
        }
        let Some(root) = self.active_cwd.clone() else { return };
        let mut results: Vec<FifMatch> = Vec::new();
        let mut truncated = false;
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        'walk: while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut children: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    if matches!(name.as_ref(), ".git" | "target" | "node_modules" | ".svn" | ".hg") {
                        continue;
                    }
                    stack.push(path);
                } else {
                    children.push(path);
                }
            }
            children.sort();
            for path in children {
                // Skip oversized / unreadable files.
                if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > FIF_MAX_FILE_BYTES {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&path) else { continue };
                let display = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                for (idx, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&needle) {
                        if results.len() >= FIF_MAX_RESULTS {
                            truncated = true;
                            break 'walk;
                        }
                        let text = line.trim_start();
                        let text: String = text.chars().take(200).collect();
                        results.push(FifMatch {
                            path: path.clone(),
                            display: display.clone(),
                            line: (idx + 1) as u32,
                            text,
                        });
                    }
                }
            }
        }
        if let Some(st) = self.find_in_files.as_mut() {
            st.results = results;
            st.truncated = truncated;
        }
    }

    /// Apply a keystroke to the Find-in-Files query buffer (modal focused).
    /// Enter opens the highlighted match; Up/Down move; printable chars / Backspace
    /// edit the query and re-run the search. Escape is handled by the modal gate.
    fn edit_find_in_files(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        match ks.key.as_str() {
            "up" => {
                if let Some(st) = self.find_in_files.as_mut() {
                    st.selected = st.selected.saturating_sub(1);
                }
            }
            "down" => {
                if let Some(st) = self.find_in_files.as_mut() {
                    let len = st.results.len();
                    if len > 0 {
                        st.selected = (st.selected + 1).min(len - 1);
                    }
                }
            }
            "enter" | "return" | "numpadenter" => {
                let target = self.find_in_files.as_ref().and_then(|st| {
                    st.results.get(st.selected).map(|m| (m.path.clone(), m.line))
                });
                if let Some((path, line)) = target {
                    self.open_fif_match(path, line, ctx);
                }
            }
            "backspace" => {
                if let Some(st) = self.find_in_files.as_mut() {
                    st.query.pop();
                }
                self.run_find_in_files();
            }
            k if k.chars().count() == 1 && !ks.cmd && !ks.ctrl => {
                if let Some(st) = self.find_in_files.as_mut() {
                    st.query.push_str(k);
                }
                self.run_find_in_files();
            }
            _ => {}
        }
    }

    /// Open a Find-in-Files match: open the file in the editor, scroll to the
    /// matched line, and close the modal.
    fn open_fif_match(&mut self, path: PathBuf, line: u32, ctx: &mut ViewContext<Self>) {
        self.modal = None;
        self.find_in_files = None;
        self.selected_file = Some(path.clone());
        self.open_file(path.clone(), ctx);
        // Scroll the (now-active) editor to the matched line.
        if let Some(h) = self.editor_views.get(&path) {
            let h = h.clone();
            h.update(ctx, |view, vctx| view.goto_line(line as usize, vctx));
        }
    }

    /// Advance the tab-switcher highlight, opening it on the first press. Returns
    /// nothing; the overlay commits on Enter / click (see `edit_tab_switcher`).
    fn advance_tab_switcher(&mut self, backward: bool) {
        // Build the entry list for the active workspace (worktree_tabs of the
        // active project/worktree), current tab first.
        let Some((api, awi, atid)) = self.active_tab else { return };
        let mut entries: Vec<(usize, usize, usize)> = self
            .worktree_tabs
            .get(&(api, awi))
            .map(|tabs| tabs.iter().map(|t| (api, awi, t.id)).collect())
            .unwrap_or_default();
        if entries.len() < 2 {
            return;
        }
        match self.tab_switcher.as_mut() {
            None => {
                // Order so the current tab is first; open highlighting the next
                // (or previous) so a single extra press lands on the neighbour.
                if let Some(cur) = entries.iter().position(|&(_, _, t)| t == atid) {
                    entries.rotate_left(cur);
                }
                let len = entries.len();
                let highlight = if backward { len - 1 } else { 1 };
                self.modal = Some(Modal::TabSwitcher);
                self.tab_switcher = Some(TabSwitcherState { entries, highlight });
            }
            Some(state) => {
                let len = state.entries.len();
                state.highlight = if backward {
                    (state.highlight + len - 1) % len
                } else {
                    (state.highlight + 1) % len
                };
            }
        }
    }

    /// Apply a keystroke to the open tab switcher (modal focused). Enter / `
    /// activate the highlight; Up/Down move it; Escape is handled by the gate.
    fn edit_tab_switcher(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        match ks.key.as_str() {
            "up" => {
                if let Some(st) = self.tab_switcher.as_mut() {
                    let len = st.entries.len();
                    if len > 0 {
                        st.highlight = (st.highlight + len - 1) % len;
                    }
                }
            }
            "down" => {
                if let Some(st) = self.tab_switcher.as_mut() {
                    let len = st.entries.len();
                    if len > 0 {
                        st.highlight = (st.highlight + 1) % len;
                    }
                }
            }
            "enter" | "return" | "numpadenter" => {
                let target = self.tab_switcher.as_ref().and_then(|st| {
                    st.entries.get(st.highlight).copied().map(|(pi, wi, tid)| {
                        let path = self
                            .projects
                            .get(pi)
                            .and_then(|p| p.worktrees.get(wi))
                            .map(|w| PathBuf::from(&w.path))
                            .unwrap_or_default();
                        ((pi, wi, tid), path)
                    })
                });
                if let Some((key, path)) = target {
                    self.activate_switcher_tab(key, path, ctx);
                }
            }
            _ => {}
        }
    }

    /// Commit the tab switcher: activate the chosen tab and close the overlay.
    fn activate_switcher_tab(
        &mut self,
        key: (usize, usize, usize),
        path: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        self.modal = None;
        self.tab_switcher = None;
        let a = CraneShellAction::Select { sel: key, path };
        self.handle_action(&a, ctx);
    }

    /// Build the branch candidate list for the active repo: local branches first,
    /// then remote branches with the `<remote>/` prefix stripped and deduped
    /// against the locals (so `git checkout <short>` DWIMs to a tracking branch).
    /// Returns `(all_candidates, locals_set)`.
    fn branch_candidates(root: &std::path::Path) -> (Vec<String>, HashSet<String>) {
        let locals = crate::warpui::git::list_local_branches(root);
        let locals_set: HashSet<String> = locals.iter().cloned().collect();
        let mut seen = locals_set.clone();
        let mut all = locals;
        for r in crate::warpui::git::list_remote_branches(root) {
            let short = r.splitn(2, '/').nth(1).unwrap_or(r.as_str()).to_string();
            if short.is_empty() || short == "HEAD" {
                continue;
            }
            if seen.insert(short.clone()) {
                all.push(short);
            }
        }
        (all, locals_set)
    }

    /// Open the "Switch Branch" modal for the active workspace.
    ///
    /// The branch listing (`git branch` + `git branch -r`) runs OFF the UI thread
    /// so opening the modal never stalls. The modal appears immediately in a
    /// loading state (empty list); the async result fills `all`/`locals` and
    /// clears `loading` when it lands, guarded by `load_gen` so a close+reopen
    /// drops the stale scan.
    fn open_switch_branch(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(root) = self.active_cwd.clone() else {
            return;
        };
        let pi = self.active_tab.map(|(p, _, _)| p).unwrap_or(0);
        let generation = self.bump_scan_gen("switchbranch");
        self.switch_branch = Some(SwitchBranchState {
            query: String::new(),
            project_idx: pi,
            all: Vec::new(),
            locals: HashSet::new(),
            selected: 0,
            loading: true,
            load_gen: generation,
        });
        self.modal = Some(Modal::SwitchBranch);
        let fut = async move { Self::branch_candidates(&root) };
        ctx.spawn(fut, move |this, (all, locals), vctx| {
            if let Some(st) = this.switch_branch.as_mut() {
                if st.load_gen == generation {
                    st.all = all;
                    st.locals = locals;
                    st.loading = false;
                    vctx.notify();
                }
            }
        });
    }

    /// Route a keystroke into the Switch-Branch search field. Up/Down move the
    /// highlight over the FILTERED list; Enter checks the highlighted branch out
    /// (or creates it when the query names no branch); typing filters.
    fn edit_switch_branch(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        // Compute the currently-visible (filtered) list to bound navigation +
        // resolve Enter's target.
        let filtered: Vec<String> = match self.switch_branch.as_ref() {
            Some(st) => {
                let q = st.query.trim().to_lowercase();
                st.all
                    .iter()
                    .filter(|b| q.is_empty() || b.to_lowercase().contains(&q))
                    .cloned()
                    .collect()
            }
            None => return,
        };
        match ks.key.as_str() {
            "up" => {
                if let Some(st) = self.switch_branch.as_mut() {
                    st.selected = st.selected.saturating_sub(1);
                }
            }
            "down" => {
                if let Some(st) = self.switch_branch.as_mut() {
                    if !filtered.is_empty() {
                        st.selected = (st.selected + 1).min(filtered.len() - 1);
                    }
                }
            }
            "enter" | "return" | "numpadenter" => {
                // Ignore Enter until the branch list has loaded — otherwise the
                // empty `filtered` list would route a typed query straight to
                // CreateBranchCheckout (creating a branch the user meant to check
                // out from the not-yet-loaded list).
                if self.switch_branch.as_ref().is_some_and(|st| st.loading) {
                    return;
                }
                let (query, sel) = self
                    .switch_branch
                    .as_ref()
                    .map(|st| (st.query.trim().to_string(), st.selected))
                    .unwrap_or_default();
                if let Some(branch) = filtered.get(sel).cloned() {
                    // Close the modal first (the click path dispatches CloseModal;
                    // CheckoutBranch itself doesn't clear the modal).
                    self.modal = None;
                    self.switch_branch = None;
                    let a = CraneShellAction::CheckoutBranch(branch);
                    self.handle_action(&a, ctx);
                } else if !query.is_empty() {
                    // CreateBranchCheckout's handler clears the modal itself.
                    let a = CraneShellAction::CreateBranchCheckout(query);
                    self.handle_action(&a, ctx);
                }
            }
            "backspace" => {
                if let Some(st) = self.switch_branch.as_mut() {
                    st.query.pop();
                    st.selected = 0;
                }
            }
            k if k.chars().count() == 1 && !ks.cmd && !ks.ctrl => {
                if let Some(st) = self.switch_branch.as_mut() {
                    st.query.push_str(k);
                    st.selected = 0;
                }
            }
            _ => {}
        }
    }

    /// Route a keystroke into the New-Workspace branch field. Enter confirms;
    /// typing edits the branch name.
    fn edit_new_workspace(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        match ks.key.as_str() {
            "enter" | "return" | "numpadenter" => {
                self.confirm_new_workspace(ctx);
            }
            "backspace" => {
                if let Some(st) = self.new_workspace.as_mut() {
                    if st.path_focused {
                        st.custom_path.pop();
                    } else if !st.branch_locked {
                        st.branch.pop();
                    }
                    st.error = None;
                }
            }
            "space" if self.new_workspace.as_ref().is_some_and(|st| st.path_focused) => {
                if let Some(st) = self.new_workspace.as_mut() {
                    st.custom_path.push(' ');
                    st.error = None;
                }
            }
            k if k.chars().count() == 1 && !ks.cmd && !ks.ctrl => {
                if let Some(st) = self.new_workspace.as_mut() {
                    if st.path_focused {
                        st.custom_path.push_str(k);
                    } else if !st.branch_locked {
                        st.branch.push_str(k);
                    }
                    st.error = None;
                }
            }
            _ => {}
        }
    }

    /// Resolve the parent dir a new Workspace's checkout goes under (old
    /// `NewWorkspaceModal::resolved_parent`): Global → `~/.crane-worktrees/
    /// <project>`, Project-local → `<project>/.crane-worktrees`, Custom → the
    /// picked folder (with a leading `~` expanded).
    fn resolved_worktree_parent(
        mode: LocationMode,
        custom_path: &str,
        project_path: &str,
        project_name: &str,
    ) -> PathBuf {
        match mode {
            LocationMode::Global => {
                let home = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(home).join(".crane-worktrees").join(project_name)
            }
            LocationMode::ProjectLocal => PathBuf::from(project_path).join(".crane-worktrees"),
            LocationMode::Custom => {
                let p = custom_path.trim();
                if let Some(rest) = p.strip_prefix("~/") {
                    let home = std::env::var("HOME").unwrap_or_default();
                    PathBuf::from(home).join(rest)
                } else {
                    PathBuf::from(p)
                }
            }
        }
    }

    /// Confirm the New-Workspace modal: compute the target path, run
    /// `git worktree add`, insert the new worktree into the project in-memory,
    /// and open it. On failure, surface the git error under the field.
    fn confirm_new_workspace(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(st) = self.new_workspace.as_ref() else {
            return;
        };
        let pi = st.project_idx;
        let branch = st.branch.trim().to_string();
        let create_branch = st.new_branch;
        if branch.is_empty() {
            if let Some(st) = self.new_workspace.as_mut() {
                st.error = Some("Enter a branch name.".to_string());
            }
            return;
        }
        let mode = st.mode;
        let custom_path = st.custom_path.clone();
        let Some(project) = self.projects.get(pi) else {
            return;
        };
        if mode == LocationMode::Custom && custom_path.trim().is_empty() {
            if let Some(st) = self.new_workspace.as_mut() {
                st.error = Some("Pick a parent folder for the Custom location.".to_string());
            }
            return;
        }
        let main = PathBuf::from(&project.path);
        // Location-mode-resolved parent + the branch (slashes flattened so
        // nested refs stay one directory), matching the card's preview.
        let safe_branch = branch.replace('/', "-");
        let path = Self::resolved_worktree_parent(mode, &custom_path, &project.path, &project.name)
            .join(safe_branch);
        // TODO(threading, audit new-workspace #3): `add_worktree` (`git worktree
        // add`) is a MUTATING op that can take multiple seconds on a large repo,
        // so it blocks the UI here. Deferred deliberately, not overlooked: a naive
        // `ctx.spawn` would be the fire-and-forget "non-blocking pill" UX the user
        // explicitly rejected (see memory `feedback_blocking_modal_for_heavy_ops`).
        // The correct fix is a JetBrains-style blocking progress modal (disable
        // input, show "Creating worktree…", move the in-memory insert + `add_tab`
        // into the completion callback, keep the modal open on error) — a modal
        // state-machine change out of scope for this threading pass. Threading it
        // without that spinner would look frozen and let a second Enter fire a
        // duplicate `git worktree add`.
        // `git worktree add` (shell-out). Never libgit2, per project rules.
        if let Err(e) = crate::warpui::git::add_worktree(&main, &branch, &path, create_branch) {
            if let Some(st) = self.new_workspace.as_mut() {
                st.error = Some(e);
            }
            return;
        }
        // Success — close the modal, insert the worktree in-memory, and open it.
        self.modal = None;
        self.new_workspace = None;
        let wpath = path.to_string_lossy().to_string();
        let diff_stat = crate::warpui::git::diff_numstat(&path);
        let dirty = crate::warpui::git::is_dirty(&path);
        let wi = {
            let Some(project) = self.projects.get_mut(pi) else {
                return;
            };
            // Dedup: if a worktree with this path already exists, reuse it.
            if let Some(existing) = project.worktrees.iter().position(|w| w.path == wpath) {
                existing
            } else {
                project.worktrees.push(crate::warpui::projects::WorktreeNode {
                    name: branch.clone(),
                    path: wpath.clone(),
                    tabs: Vec::new(),
                    diff_stat,
                    dirty,
                });
                project.worktrees.len() - 1
            }
        };
        // Refresh the poll signature so the auto-detect tick doesn't see this as a
        // brand-new external change and redo the work.
        self.worktree_poll_sig.remove(&main.to_string_lossy().to_string());
        // Open (expand + create a first tab in) the new worktree.
        self.add_tab(pi, wi, ctx);
        self.invalidate_editor_diffs(&*ctx);
    }

    /// Background worktree auto-detection tick (~1.5s). For each git project it
    /// reconciles the in-memory worktrees against `git worktree list`: appends
    /// worktrees created outside the app, drops one whose checkout dir vanished,
    /// and flips a loose folder to a git project when a `.git` entry appears.
    /// Cheap when idle — the per-project `git worktree list` output is signature-
    /// cached, and heavier per-worktree git only runs for worktrees that changed.
    /// Proactively surface the editor's external-change reload banner. The editor
    /// only re-stats its file during its own `render`, and it never re-renders on
    /// its own — so without this an edit made in another program wouldn't show the
    /// banner until the user next interacts with that pane. On the worktree-poll
    /// cadence we re-stat each open editor and notify (re-render) the ones whose
    /// file changed on disk; the notify makes the banner appear on its own. Only
    /// fires while a change is pending, and stops once the user hits Reload/Keep
    /// (both reset the editor's mtime baseline via `refresh_disk_mtime`).
    /// Reconcile the native WKWebViews against the Browser Panes: pull each
    /// pane's active/inactive tab slots + painted body rect, decide visibility
    /// (active Tab's layout only; a maximized non-browser pane hides them; any
    /// overlay — modal / context menu / branch picker / drag preview — hides
    /// all, because the WKWebView composites ABOVE the GPU surface), then hand
    /// everything to `BrowserHost::sync`. Runs on a 33ms tick; cheap no-op
    /// while no Browser Pane exists.
    ///
    /// NOTE(choice): toasts do NOT hide the webviews — a 4s blink per
    /// notification is worse than a toast sliding under a browser pane that
    /// happens to occupy the bottom-right corner.
    #[cfg(target_os = "macos")]
    fn browser_tick(&mut self, ctx: &mut ViewContext<Self>) {
        let has_browser = self
            .panes
            .values()
            .any(|p| matches!(p, PaneContent::Browser(_)));
        if !has_browser && self.browser_host.is_idle() {
            return;
        }
        // Bridge starts from the queued nav actions (views push those on
        // click); alive/inactive slots are pulled from the views here.
        let mut bridge = crate::warpui::browser::take_bridge();
        // Visible = leaves of the active Tab's layout, narrowed to just the
        // maximized pane when one is maximized.
        let visible: HashSet<PaneId> = match self.maximized {
            Some(m) => std::iter::once(m).collect(),
            None => self
                .active_tab
                .and_then(|t| self.layouts.get(&t))
                .map(|n| {
                    let mut v = Vec::new();
                    n.leaves(&mut v);
                    v.into_iter().collect()
                })
                .unwrap_or_default(),
        };
        let mut all_keys: HashSet<crate::warpui::browser::SlotKey> = HashSet::new();
        let handles: Vec<(PaneId, ViewHandle<crate::warpui::browser_view::WarpBrowserView>)> =
            self.panes
                .iter()
                .filter_map(|(id, pc)| match pc {
                    PaneContent::Browser(h) => Some((*id, h.clone())),
                    _ => None,
                })
                .collect();
        for (id, h) in &handles {
            h.read(&*ctx, |v, _| {
                for k in v.all_keys() {
                    all_keys.insert(k);
                }
                let (key, rect, url) = v.active_slot();
                // The RectProbe records PRE-ZOOM layout coordinates. warpui's GPU
                // compositor magnifies everything by the global zoom factor when
                // drawing (so terminals/editors land correctly), but the native
                // WKWebView is positioned in real window points — so it must be
                // given the layout rect scaled UP by the zoom, or it renders
                // shifted + undersized by exactly the zoom factor.
                let zoom = crate::warpui::fontsize::zoom_level();
                let rect = if zoom != 1.0 {
                    warpui::geometry::rect::RectF::new(
                        rect.origin() * zoom,
                        vec2f(rect.width() * zoom, rect.height() * zoom),
                    )
                } else {
                    rect
                };
                let is_alive = visible.contains(id) && rect.width() > 1.0 && rect.height() > 1.0;
                if is_alive {
                    bridge.alive.push((key, rect, url));
                } else {
                    bridge.inactive.push((key, url));
                }
                for (k, u) in v.inactive_slots() {
                    bridge.inactive.push((k, u));
                }
            });
        }
        let hide_all = self.modal.is_some()
            || self.context_menu.is_some()
            || self.row_menu.is_some()
            || self.worktree_menu.is_some()
            || self.tab_menu.is_some()
            || self.folder_menu.is_some()
            || self.branch_picker.is_some()
            || self.new_pane_menu_open
            || self.drop_preview.borrow().is_some();
        let Some(win) = crate::warpui::browser::HostWindow::current() else {
            return;
        };
        let wake = self.ui_wake.clone();
        self.browser_host
            .sync(&win, &wake, bridge, hide_all, &all_keys);
        // Apply WKWebView-reported URL changes (redirects, link clicks, SPA
        // routes) back to the owning tabs so the URL bar tracks the page.
        let updates = self.browser_host.drain_url_updates();
        if !updates.is_empty() {
            for (_, h) in &handles {
                h.update(ctx, |v, vctx| {
                    let mut changed = false;
                    for (k, u) in &updates {
                        changed |= v.apply_url_update(*k, u);
                    }
                    if changed {
                        vctx.notify();
                    }
                });
            }
        }
        crate::warpui::browser::set_loading_snapshot(self.browser_host.loading_set());
        if has_browser {
            crate::warpui::browser::set_memory_snapshot(self.browser_host.memory.snapshot());
        }
        // Keep the tab-chip spinners animating / footer fresh while loading.
        if !self.browser_host.loading_set().is_empty() {
            ctx.notify();
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn browser_tick(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// In-session update awareness (rides the 1.5s poll tick): re-check the
    /// GitHub latest release every 6 hours — a release published while Crane
    /// runs gets noticed without a relaunch — and surface a one-shot toast the
    /// first time a newer version lands (install lives in Settings > About).
    fn update_tick(&mut self, ctx: &mut ViewContext<Self>) {
        const RECHECK: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
        if self.last_update_check.elapsed() >= RECHECK {
            self.last_update_check = std::time::Instant::now();
            let wake = self.ui_wake.clone();
            crate::warpui::update::spawn_recheck(move || wake());
        }
        // The banner overlay (update_banner) is render-driven off
        // update_state() + the prompt decisions; just repaint when a discovery
        // could have landed so it appears without user input.
        if matches!(
            crate::warpui::update::update_state(),
            crate::warpui::update::UpdateState::UpdateAvailable { .. }
        ) {
            ctx.notify();
        }
    }

    /// Old check.rs `should_show`: the update banner appears for an available
    /// version unless it was closed this session, skipped forever, or is
    /// inside its 7-day remind window. Once the user engages (download /
    /// ready / failed), the banner persists through those states unless
    /// closed this session.
    fn update_banner_should_show(&self) -> bool {
        use crate::warpui::update::UpdateState;
        let session_dismissed = |v: &str| self.update_dismissed_session.as_deref() == Some(v);
        match crate::warpui::update::update_state() {
            UpdateState::UpdateAvailable { version } => {
                if session_dismissed(&version) {
                    return false;
                }
                match self.update_prompts.get(&version) {
                    None => true,
                    Some(UpdatePrompt::Dismissed) => false,
                    Some(UpdatePrompt::RemindAt(t)) => now_epoch_secs() >= *t,
                }
            }
            UpdateState::Downloading { .. } | UpdateState::Ready { .. }
            | UpdateState::Failed { .. } => {
                let v = crate::warpui::update::latest_available().unwrap_or_default();
                !session_dismissed(&v)
            }
            // A routine background check that finds nothing stays silent, but
            // a check the user explicitly asked for (menu / Settings button)
            // always gets a visible answer — old check.rs `manual_check`
            // semantics, surfaced in the same persistent banner rather than a
            // separate toast type.
            UpdateState::Idle => self.manual_update_check,
            UpdateState::Checking => false,
        }
    }

    /// The persistent bottom-right update card (old `modals/update_toast.rs`):
    /// Install / Remind-in-7-days / Skip-this-version on discovery, live
    /// progress while downloading, Restart-now / Later when staged, error +
    /// Retry on failure. Backed by the same lifecycle Settings > About shows.
    fn update_banner(&self) -> Box<dyn Element> {
        use crate::warpui::update::{self, UpdateState};
        let small = |label: &str, action: CraneShellAction| -> Box<dyn Element> {
            EventHandler::new(
                Container::new(
                    Text::new(label.to_string(), self.ui_font, 11.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(9.0)
                .with_padding_right(9.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
        };
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        // Header row: title + close ×.
        let title = match update::update_state() {
            UpdateState::UpdateAvailable { ref version } => {
                format!("Crane v{version} is available")
            }
            UpdateState::Downloading { .. } => "Downloading update…".to_string(),
            UpdateState::Ready { .. } => "Update ready".to_string(),
            UpdateState::Failed { .. } => "Update failed".to_string(),
            UpdateState::Idle => format!("You're up to date — v{}", env!("CARGO_PKG_VERSION")),
            UpdateState::Checking => String::new(),
        };
        let close = EventHandler::new(
            Container::new(self.icon(icons::X, 10.0, theme::text_muted()))
                .with_padding_left(8.0)
                .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::UpdateDismissSession);
            DispatchEventResult::StopPropagation
        })
        .finish();
        col = col.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new(title, self.ui_font, 12.5)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_child(Expanded::new(
                    1.0,
                    ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish(),
                )
                .finish())
                .with_child(close)
                .finish(),
        );
        col = col.with_child(Self::spacer(8.0));
        match update::update_state() {
            UpdateState::UpdateAvailable { .. } => {
                col = col.with_child(
                    Flex::row()
                        .with_child(small(
                            "Install update",
                            CraneShellAction::StartUpdateDownload,
                        ))
                        .with_child(Self::spacer(6.0))
                        .with_child(small(
                            "Remind in 7 days",
                            CraneShellAction::UpdateRemindLater,
                        ))
                        .with_child(Self::spacer(6.0))
                        .with_child(small(
                            "Skip this version",
                            CraneShellAction::UpdateSkipVersion,
                        ))
                        .finish(),
                );
            }
            UpdateState::Downloading { received, total } => {
                let label = if total > 0 {
                    format!("{}%", ((received.saturating_mul(100)) / total).min(100))
                } else {
                    format!("{} KB", received / 1024)
                };
                col = col.with_child(
                    Text::new(label, self.ui_font, 11.5)
                        .with_color(theme::text_muted())
                        .finish(),
                );
            }
            UpdateState::Ready { path } => {
                col = col.with_child(
                    Flex::row()
                        .with_child(small(
                            "Restart now",
                            CraneShellAction::ApplyUpdate(path),
                        ))
                        .with_child(Self::spacer(6.0))
                        .with_child(small("Later", CraneShellAction::UpdateDismissSession))
                        .finish(),
                );
            }
            UpdateState::Failed { msg } => {
                col = col
                    .with_child(
                        Text::new(msg, self.ui_font, 10.5)
                            .with_color(theme::error())
                            .finish(),
                    )
                    .with_child(Self::spacer(6.0))
                    .with_child(small("Retry", CraneShellAction::StartUpdateDownload));
            }
            UpdateState::Idle => {
                col = col.with_child(
                    Text::new(
                        "No new release yet — you'll get a banner the moment one ships."
                            .to_string(),
                        self.ui_font,
                        10.5,
                    )
                    .with_color(theme::text_muted())
                    .finish(),
                );
            }
            UpdateState::Checking => {}
        }
        Container::new(
            ConstrainedBox::new(
                Container::new(col.finish())
                    .with_background_color(theme::surface())
                    .with_border(Border::all(1.0).with_border_color(theme::accent()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(10.0)
                    .with_padding_bottom(10.0)
                    .finish(),
            )
            .with_width(360.0)
            .finish(),
        )
        .finish()
    }

    fn poll_editor_disk_changes(&mut self, ctx: &mut ViewContext<Self>) {
        let handles: Vec<_> = self.editor_views.values().cloned().collect();
        for h in handles {
            if h.read(ctx, |v, _app| v.disk_changed()) {
                // Clean buffer → adopt the external edit silently (old Crane's
                // `file_save.rs` behavior); the reload banner is reserved for
                // buffers with in-flight edits that a reload would clobber.
                h.update(ctx, |v, vctx| {
                    if v.is_dirty(&*vctx) {
                        vctx.notify();
                    } else {
                        v.reload_from_disk(vctx);
                    }
                });
            }
        }
    }

    fn poll_worktrees(&mut self, ctx: &mut ViewContext<Self>) {
        let mut changed = false;
        // 1) Loose → git flip: cheap `.git` existence check (fs stat, no git).
        let loose_flips: Vec<usize> = self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_loose && std::path::Path::new(&p.path).join(".git").exists())
            .map(|(i, _)| i)
            .collect();
        for i in loose_flips {
            if let Some(p) = self.projects.get_mut(i) {
                p.is_loose = false;
            }
            changed = true;
        }
        // 2) Per git-project worktree reconciliation.
        // Collect (pi, main_path) for git projects to avoid borrow conflicts.
        let git_projects: Vec<(usize, String)> = self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_loose)
            .map(|(i, p)| (i, p.path.clone()))
            .collect();
        // A dead worktree to remove this tick (at most one — removal remaps many
        // index-keyed structures, so we do one per tick and let the next tick
        // catch the rest).
        let mut dead_remove: Option<(usize, usize)> = None;
        for (pi, main_path) in git_projects {
            let main = std::path::Path::new(&main_path);
            let listed = crate::warpui::git::list_worktrees(main);
            // Signature = the git output; skip the heavy path when unchanged.
            let sig: String = listed
                .iter()
                .map(|(p, b)| format!("{}|{}", p.display(), b))
                .collect::<Vec<_>>()
                .join("\n");
            if self.worktree_poll_sig.get(&main_path) == Some(&sig) {
                continue;
            }
            self.worktree_poll_sig.insert(main_path.clone(), sig);
            let listed_paths: HashSet<String> =
                listed.iter().map(|(p, _)| p.to_string_lossy().to_string()).collect();
            // 2a) Reconcile each worktree git knows about. NEW paths get a row;
            //     EXISTING paths whose branch moved (a `git checkout` in the
            //     worktree's terminal) have their label + diff/dirty refreshed
            //     in place. The previous code only appended new rows and dropped
            //     dead ones, so a branch switch left the sidebar showing the
            //     stale branch forever.
            for (wpath, wbranch) in &listed {
                let wps = wpath.to_string_lossy().to_string();
                let name = if wbranch == "(detached)" || wbranch.is_empty() {
                    crate::warpui::projects::basename_of(wpath)
                } else {
                    wbranch.clone()
                };
                // A user-renamed worktree keeps its label from `worktree_names`
                // (which wins at render); don't clobber `w.name` with the raw
                // branch here — matches the other two branch-update paths
                // (`apply_git_scan`, `sync_worktree_branch_label`).
                let renamed = self.worktree_names.contains_key(&wps);
                let Some(p) = self.projects.get_mut(pi) else { continue };
                if let Some(w) = p.worktrees.iter_mut().find(|w| w.path == wps) {
                    if !renamed && w.name != name {
                        w.name = name;
                        w.diff_stat = crate::warpui::git::diff_numstat(wpath);
                        w.dirty = crate::warpui::git::is_dirty(wpath);
                        changed = true;
                    }
                } else {
                    let diff_stat = crate::warpui::git::diff_numstat(wpath);
                    let dirty = crate::warpui::git::is_dirty(wpath);
                    p.worktrees.push(crate::warpui::projects::WorktreeNode {
                        name,
                        path: wps,
                        tabs: Vec::new(),
                        diff_stat,
                        dirty,
                    });
                    changed = true;
                }
            }
            // 2b) Detect a worktree whose checkout dir vanished on disk AND is no
            //     longer in git's list — remove it (never the primary checkout).
            if dead_remove.is_none() {
                if let Some(p) = self.projects.get(pi) {
                    for (wi, w) in p.worktrees.iter().enumerate() {
                        if w.path == main_path {
                            continue; // primary working tree — never auto-remove.
                        }
                        if !listed_paths.contains(&w.path)
                            && !std::path::Path::new(&w.path).exists()
                        {
                            dead_remove = Some((pi, wi));
                            break;
                        }
                    }
                }
            }
        }
        // Apply at most one dead-worktree removal via the full teardown path
        // (it remaps every (pi, wi, *)-keyed structure). The checkout already
        // vanished on disk, so tear down directly — no confirm modal.
        if let Some((pi, wi)) = dead_remove {
            let a = CraneShellAction::RemoveWorktreeConfirmed { pi, wi };
            self.handle_action(&a, ctx);
            // handle_action already notified; nothing more to do.
            return;
        }
        if changed {
            ctx.notify();
        }
    }

    /// Reload the project list from session.json + the current overlay
    /// (added / removed / tints). Call after mutating any of those three fields.
    // ── Keyed async git-scan primitive ───────────────────────────────────────
    // The reusable dedup / cancel-on-supersede building block. `spawn_git_scan`
    // bumps a per-scope generation, runs the (branch + diff + dirty) shell-outs
    // for a set of paths OFF the UI thread, and on the main-thread callback drops
    // the result if a newer scan for the same scope has since superseded it —
    // else applies the git fields into the matching `self.projects` nodes by
    // path. This is warpui-native (built on `ctx.spawn`), no thread pool needed.

    /// Bump `scope`'s generation and return the new value. The in-flight scan
    /// captures this; a later scan for the same scope makes it stale.
    fn bump_scan_gen(&mut self, scope: &str) -> u64 {
        let g = self.git_scan_gen.entry(scope.to_string()).or_insert(0);
        *g += 1;
        *g
    }

    /// Run the git scan for `paths` off-thread under `scope`, applying the
    /// results back into `self.projects` by path when they land (unless a newer
    /// scan for the same scope superseded this one). No-op for an empty set.
    fn spawn_git_scan(&mut self, ctx: &mut ViewContext<Self>, scope: String, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let generation = self.bump_scan_gen(&scope);
        let fut = async move {
            paths
                .into_iter()
                .map(|p| {
                    let info = crate::warpui::projects::scan_repo_git(&p);
                    (p, info)
                })
                .collect::<Vec<(PathBuf, crate::warpui::projects::RepoGitInfo)>>()
        };
        ctx.spawn(fut, move |this, results, vctx| {
            this.apply_git_scan(&scope, generation, results, vctx);
        });
    }

    /// Apply an async git-scan result. Drops it if `scope`'s generation moved on
    /// (a newer scan superseded this one). Otherwise fills each scanned path's
    /// branch label (unless the user renamed the row) + diff/dirty badge into the
    /// matching worktree node, and keeps the status-bar `self.branch` in sync when
    /// a scanned path is the active repo.
    fn apply_git_scan(
        &mut self,
        scope: &str,
        generation: u64,
        results: Vec<(PathBuf, crate::warpui::projects::RepoGitInfo)>,
        vctx: &mut ViewContext<Self>,
    ) {
        if self.git_scan_gen.get(scope).copied() != Some(generation) {
            return; // superseded — a newer scan for this scope is in flight.
        }
        for (path, info) in results {
            let path_str = path.to_string_lossy().to_string();
            for p in self.projects.iter_mut() {
                for w in p.worktrees.iter_mut() {
                    if w.path == path_str {
                        // Fill the branch label only when git returned one AND the
                        // user hasn't pinned a custom name — else keep the shallow
                        // folder-name fallback / rename.
                        if !info.branch.is_empty() && !self.worktree_names.contains_key(&w.path) {
                            w.name = info.branch.clone();
                        }
                        w.diff_stat = info.diff_stat;
                        w.dirty = info.dirty;
                    }
                }
            }
            if !info.branch.is_empty()
                && self.active_cwd.as_deref() == Some(path.as_path())
            {
                self.branch = info.branch.clone();
            }
        }
        vctx.notify();
    }

    /// Re-scan git fields for the whole current tree under the `"tree"` scope.
    /// Called after a structural reload (which resets all badges to shallow).
    fn rescan_all_git(&mut self, ctx: &mut ViewContext<Self>) {
        let paths = crate::warpui::projects::scan_paths(&self.projects);
        self.spawn_git_scan(ctx, "tree".to_string(), paths);
    }

    /// Structural reload of the project tree (SHALLOW — zero `git` on the UI
    /// thread). Rebuilds names/paths/worktrees/grouping only; branch labels +
    /// diff/dirty badges reset to their shallow defaults. Callers that want the
    /// badges back must follow with `rescan_all_git`.
    fn reload_projects(&mut self) {
        self.projects = crate::warpui::projects::load_projects_shallow(
            &self.added_projects,
            &self.removed_project_paths,
            &self.project_tints,
        );
        self.sync_watches();
    }

    /// Reconcile the filesystem watcher's registered roots against the current
    /// Project + Workspace set: unwatch roots that vanished, watch newly-seen
    /// ones. Cheap on a no-op — `watched` (original path strings) gates so a
    /// `canonicalize` + OS (un)watch only runs on an actual change. Called on
    /// startup, after project add/remove, and each worktree poll tick.
    fn sync_watches(&mut self) {
        let current: HashSet<String> = self
            .projects
            .iter()
            .flat_map(|p| {
                std::iter::once(p.path.clone())
                    .chain(p.worktrees.iter().map(|w| w.path.clone()))
            })
            .collect();
        // Drop watches for roots that are no longer present.
        let stale: Vec<String> = self
            .watched
            .iter()
            .filter(|r| !current.contains(*r))
            .cloned()
            .collect();
        for path in stale {
            self.file_watcher.unwatch(std::path::Path::new(&path));
            self.watched.remove(&path);
        }
        // Register roots we are not yet watching (recursive; both Projects and
        // Workspaces route as their own root).
        let add: Vec<String> = current
            .into_iter()
            .filter(|r| !self.watched.contains(r))
            .collect();
        for path in add {
            let _ = self
                .file_watcher
                .watch_project(std::path::Path::new(&path));
            self.watched.insert(path);
        }
    }

    /// Canonicalized checkout roots of every worktree in the project that owns
    /// `root` — where `root` is a watched root that saw a git-internal write.
    /// A project "owns" `root` when its primary checkout (`p.path`) OR any of
    /// its worktree checkouts canonicalizes to `root`. Used to fan a commit's
    /// ref write (which lands under the main repo, not the committed linked
    /// worktree's checkout) out to all sibling worktree badges so the right one
    /// refreshes. Returns canonical paths so they match the bg-scan mapping,
    /// which keys on `canonicalize(w.path)`.
    fn project_worktree_checkout_roots(&self, root: &std::path::Path) -> Vec<PathBuf> {
        for p in &self.projects {
            let owns = std::fs::canonicalize(&p.path).ok().as_deref() == Some(root)
                || p.worktrees.iter().any(|w| {
                    !w.path.is_empty()
                        && std::fs::canonicalize(&w.path).ok().as_deref() == Some(root)
                });
            if owns {
                return p
                    .worktrees
                    .iter()
                    .filter(|w| !w.path.is_empty())
                    .filter_map(|w| std::fs::canonicalize(&w.path).ok())
                    .collect();
            }
        }
        Vec::new()
    }

    /// Drain coalesced filesystem change events. Always empties the receiver
    /// (keeps the mpsc bounded) and, if any batch belongs to the ACTIVE repo,
    /// refreshes its Changes/Files panel + marks editor diffs dirty — so
    /// external / agent edits update the Right Panel and diff panes without a
    /// manual reload. Only the active repo is refreshed (cheap; other repos
    /// refresh when focused).
    fn drain_fs_events(&mut self, ctx: &mut ViewContext<Self>) {
        let active_canon = self
            .active_cwd
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok());
        let mut active_touched = false;
        // Whether any changed path under the active repo was git-internal (a
        // ref/HEAD/packed-refs write from a commit / fetch / branch switch) —
        // that, and only that, needs the Git Log graph reloaded.
        let mut git_refs_touched = false;
        // Every distinct non-active root touched this tick — folded into
        // `pending_bg_roots` below so background repos' sidebar badges refresh
        // live too (see the block after the active-repo one).
        let mut bg_touched: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        // Roots under which a git-internal ref/HEAD/worktrees write landed this
        // tick. A commit / fetch / checkout in a LINKED worktree writes its
        // refs + index + logs under the MAIN repo's `.git` (the worktree's own
        // `.git` is just a file pointing there) — the worktree *checkout* dir
        // sees no event at all, so its badge could never clear from a
        // checkout-scoped watch. We collect the roots that saw such writes and,
        // below, rescan EVERY worktree of the owning project so whichever
        // linked worktree was committed to gets its +N/-M refreshed.
        let mut git_meta_roots: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        let is_git_meta = |paths: &[PathBuf]| paths.iter().any(|p| git_meta_path(p));
        while let Ok(ev) = self.fs_events.try_recv() {
            if is_git_meta(&ev.paths) {
                git_meta_roots.insert(ev.root.clone());
            }
            if let Some(ac) = active_canon.as_ref() {
                if &ev.root == ac {
                    active_touched = true;
                    if is_git_meta(&ev.paths) {
                        git_refs_touched = true;
                    }
                    continue;
                }
            }
            bg_touched.insert(ev.root);
        }
        // Expand each git-meta root to every worktree checkout of its owning
        // project and fold those into the bg badge rescan set. This is the only
        // path that clears a LINKED worktree's badge after a commit, since the
        // triggering event surfaced under the main repo, not the checkout.
        if !git_meta_roots.is_empty() {
            git_refs_touched = true;
            for root in &git_meta_roots {
                for wp in self.project_worktree_checkout_roots(root) {
                    bg_touched.insert(wp);
                }
            }
            // If the ACTIVE repo is a linked worktree that was just committed
            // to, its checkout saw no event, so `active_touched` is still false
            // — but its Changes/Files panel must refresh too. Promote it.
            if let Some(ac) = active_canon.as_ref()
                && bg_touched.contains(ac)
            {
                active_touched = true;
            }
        }
        if active_touched {
            self.refresh_panel(ctx);
            self.invalidate_editor_diffs(&*ctx);
            // Also refresh the sidebar +N/-M badge for the active worktree. A
            // plain working-tree edit changes the diff but not a ref, so the
            // periodic worktree poll (branch-change only) and the startup scan
            // would leave the badge stale — the tree keeps changing under the
            // user with the sidebar frozen at its launch value. Scoped to the
            // active repo's checkout path(s); async + applied by path so it only
            // touches the matching node(s), on its own scan generation so it
            // never cancels the full-tree scan.
            if let Some(root) = active_canon.as_ref() {
                let paths: Vec<PathBuf> = self
                    .projects
                    .iter()
                    .flat_map(|p| p.worktrees.iter())
                    .filter(|w| {
                        !w.path.is_empty()
                            && std::fs::canonicalize(&w.path).ok().as_deref()
                                == Some(root.as_path())
                    })
                    .map(|w| PathBuf::from(&w.path))
                    .collect();
                if !paths.is_empty() {
                    self.spawn_git_scan(ctx, "active-diff".to_string(), paths);
                }
            }
        }
        // Refresh sidebar badges for NON-active repos too — an agent or a git
        // op in a background workspace should tick its +N/-M without a
        // project switch. Roots touched this tick join any roots left over
        // from a previous tick that missed the debounce window, so a root
        // never gets stranded waiting for a fs event that may not come again.
        self.pending_bg_roots.extend(bg_touched);
        if !self.pending_bg_roots.is_empty()
            && self.bg_badge_last_scan.elapsed() >= std::time::Duration::from_millis(500)
        {
            let bg_paths: Vec<PathBuf> = self
                .pending_bg_roots
                .iter()
                .flat_map(|root| {
                    self.projects
                        .iter()
                        .flat_map(|p| p.worktrees.iter())
                        .filter(|w| {
                            !w.path.is_empty()
                                && std::fs::canonicalize(&w.path).ok().as_deref()
                                    == Some(root.as_path())
                        })
                        .map(|w| PathBuf::from(&w.path))
                })
                .collect();
            self.pending_bg_roots.clear();
            self.bg_badge_last_scan = std::time::Instant::now();
            // Own scope ("bg-diff") — a distinct generation counter from
            // "active-diff" / "tree", so this can never cancel or starve the
            // active-repo scan (each scope's generation only supersedes prior
            // scans under that same scope; see `bump_scan_gen`).
            if !bg_paths.is_empty() {
                self.spawn_git_scan(ctx, "bg-diff".to_string(), bg_paths);
            }
        }
        // A ref/HEAD write in the active repo (commit / fetch / branch switch)
        // also moved a worktree's branch — refresh the sidebar branch labels on
        // the fast (~250ms) tick instead of waiting for the 1.5s worktree poll,
        // so a `git checkout` in the terminal shows up near-instantly.
        // Rate-limited: `poll_worktrees` shells `git worktree list` per project
        // SYNCHRONOUSLY on this (UI-thread) tick — unthrottled, an event storm
        // would run it every 250ms and freeze the app (seen live with ~12
        // projects). 1s keeps a checkout's label refresh feeling instant while
        // capping the worst case at one poll per second.
        if git_refs_touched
            && self.fs_ref_poll_last.elapsed() >= std::time::Duration::from_secs(1)
        {
            self.fs_ref_poll_last = std::time::Instant::now();
            self.poll_worktrees(ctx);
        }
        // Auto-reload the Git Log on a ref change while the dock is open,
        // debounced (the watcher already coalesces bursts; this guards against a
        // rapid rebase spamming reloads).
        if git_refs_touched
            && self.show_git_log
            && self.git_log_last_reload.elapsed() >= std::time::Duration::from_millis(250)
        {
            self.reload_git_log(ctx);
        }
    }

    /// Best-effort "project / branch  ·  tab" breadcrumb for the tab identified
    /// by `key` — used as a notification toast's source label. Any leg that has
    /// gone stale falls back to a placeholder so the toast still names something.
    fn notif_source_label(&self, key: (usize, usize, usize)) -> String {
        let (pi, wi, tid) = key;
        let proj = self
            .projects
            .get(pi)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "—".to_string());
        let branch = self
            .projects
            .get(pi)
            .and_then(|p| p.worktrees.get(wi))
            .map(|w| w.name.clone())
            .unwrap_or_default();
        let tab = self
            .worktree_tabs
            .get(&(pi, wi))
            .and_then(|tabs| tabs.iter().find(|t| t.id == tid))
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let mut out = proj;
        if !branch.is_empty() {
            out.push_str(" / ");
            out.push_str(&branch);
        }
        if !tab.is_empty() {
            // ASCII separator only — the bundled fonts don't cover glyphs like
            // the middle dot, so they'd render as tofu (see CLAUDE.md).
            out.push_str(" / ");
            out.push_str(&tab);
        }
        out
    }

    /// After a `git checkout` in `root`, refresh the matching left-panel worktree
    /// row's branch label in place. `refresh_panel` rebuilds Changes/Files/`self.branch`
    /// but never `self.projects`, and `poll_worktrees` skips already-known paths — so
    /// without this the row keeps the OLD branch name until restart. Skips rows the
    /// user has explicitly renamed (their per-path override wins over the branch name).
    fn sync_worktree_branch_label(&mut self, root: &std::path::Path) {
        let new_branch = crate::warpui::git::current_branch(root);
        if new_branch.is_empty() {
            return;
        }
        for p in self.projects.iter_mut() {
            for w in p.worktrees.iter_mut() {
                if std::path::Path::new(&w.path) == root
                    && !self.worktree_names.contains_key(&w.path)
                {
                    w.name = new_branch.clone();
                }
            }
        }
    }

    /// Refresh the Right Panel (Changes + Files tree + branch/ahead-behind) for
    /// the active worktree.
    ///
    /// The heavy git — `git rev-parse` (branch), `git status --porcelain`
    /// (changes) and `git rev-list @{u}...HEAD` (ahead/behind) — runs OFF the UI
    /// thread via `ctx.spawn`, keyed by the repo path so a rapid focus switch to
    /// another repo drops this stale result (the `"panel:<repo>"` scope +
    /// generation). The cheap synchronous part — the mtime-cached Files-tab FS
    /// walk — runs immediately using the git status we already have, then the
    /// async callback recolours it once fresh changes land. This is the OG
    /// `refresh_active_git_status` background-job pattern, warpui-native.
    fn refresh_panel(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(root) = self.active_cwd.clone() else {
            self.file_rows.clear();
            self.changes.clear();
            self.file_status.clear();
            self.dirty_dirs.clear();
            self.ahead_behind = None;
            self.branch.clear();
            return;
        };
        // Rebuild the Files-tab FS tree NOW (cheap: mtime-cached walk, zero git),
        // coloured from whatever change set we currently hold. When the async git
        // result lands it recolours via `apply_panel_git`. This keeps the tree on
        // screen with no flicker while the git shell-outs run.
        if self.files_tab {
            self.rebuild_file_rows(&root);
        }
        // Fetch branch + changes + ahead/behind off-thread, keyed by repo path.
        let scope = format!("panel:{}", root.to_string_lossy());
        let generation = self.bump_scan_gen(&scope);
        let root_for_fut = root.clone();
        let fut = async move {
            let branch = crate::warpui::git::current_branch(&root_for_fut);
            let changes = crate::warpui::git::changes(&root_for_fut);
            let ahead_behind = crate::warpui::git::ahead_behind(&root_for_fut);
            (branch, changes, ahead_behind)
        };
        ctx.spawn(fut, move |this, (branch, changes, ahead_behind), vctx| {
            // Drop if a newer panel refresh (this repo or a switch to another)
            // superseded us, or the active repo changed under us.
            if this.git_scan_gen.get(&scope).copied() != Some(generation) {
                return;
            }
            if this.active_cwd.as_deref() != Some(root.as_path()) {
                return;
            }
            this.branch = branch;
            this.changes = changes;
            this.ahead_behind = ahead_behind;
            this.rebuild_file_status();
            if this.files_tab {
                this.rebuild_file_rows(&root);
            }
            vctx.notify();
        });
    }

    /// Rebuild `file_status` (rel-path → status char) + `dirty_dirs` (dirs with a
    /// changed descendant) from the current `self.changes`. Pure CPU — no git.
    /// Port of old egui `git_status_map` in explorer.rs.
    fn rebuild_file_status(&mut self) {
        self.file_status.clear();
        self.dirty_dirs.clear();
        for c in &self.changes {
            let rel = c.path.trim_end_matches('/').to_string();
            let ch = c.status.chars().next().unwrap_or(' ');
            self.file_status.insert(rel.clone(), ch);
            let mut cur = std::path::Path::new(&rel);
            while let Some(parent) = cur.parent() {
                if parent.as_os_str().is_empty() {
                    break;
                }
                self.dirty_dirs.insert(parent.to_string_lossy().to_string());
                cur = parent;
            }
        }
    }

    /// Rebuild the Files-tab tree rows for `root` (mtime-cached FS walk, no git)
    /// and colour each row from the current `file_status` / `dirty_dirs`.
    fn rebuild_file_rows(&mut self, root: &std::path::Path) {
        let skip = self.nested_repo_skip_set(root);
        let mut rows = file_tree::build_rows_with_skip(root, &self.expanded_dirs, &skip);
        for r in &mut rows {
            let rel = r
                .path
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            if let Some(rel) = rel {
                if r.is_dir {
                    r.git_status = if self.dirty_dirs.contains(&rel) { Some('*') } else { None };
                } else {
                    r.git_status = self.file_status.get(&rel).copied();
                }
            }
        }
        self.file_rows = rows;
    }

    /// Directories under `root` that are surfaced as their own top-level
    /// Projects (nested git repos beneath a loose parent) — hidden from the
    /// Files tree so they don't appear twice. Port of `active_project_files_skip`.
    fn nested_repo_skip_set(&self, root: &std::path::Path) -> HashSet<PathBuf> {
        let mut skip = HashSet::new();
        for p in &self.projects {
            if p.path.is_empty() {
                continue;
            }
            let pp = PathBuf::from(&p.path);
            if pp != root && pp.starts_with(root) {
                skip.insert(pp);
            }
        }
        skip
    }

    /// A clickable project/worktree row — clicking respawns the terminal in
    /// `path` (empty path = non-clickable, e.g. a tab label).
    /// A phosphor icon glyph rendered as Text in the icon font.
    fn icon(&self, glyph: &str, size: f32, color: ColorU) -> Box<dyn Element> {
        Text::new(glyph.to_string(), self.icon_font, size)
            .with_color(color)
            .finish()
    }

    /// A 20×20 hover-lit icon button (12px glyph). `key` must be unique per
    /// on-screen instance — it keys the persistent hover state.
    fn icon_button(&self, key: &str, glyph: &'static str, action: CraneShellAction) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("ibtn:{key}"));
        let icon_font = self.icon_font;
        Hoverable::new(state, move |ms| {
            let (bg, fg) = if ms.is_hovered() {
                (theme::selection_wash(), theme::text_hover())
            } else {
                (ColorU::new(0, 0, 0, 0), theme::text_muted())
            };
            ConstrainedBox::new(
                Container::new(
                    Align::new(
                        Text::new(glyph.to_string(), icon_font, 12.0)
                            .with_color(fg)
                            .finish(),
                    )
                    .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish(),
            )
            .with_width(20.0)
            .with_height(20.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }


    #[allow(clippy::too_many_arguments)]
    /// A left-tree row. `caret` = Some(expanded) draws a disclosure chevron;
    /// None = no chevron (tabs/leaves). The TRANSPARENT hit Rect MUST be the
    /// topmost child (warpui hit-tests at the child's max z-index).
    #[allow(clippy::too_many_arguments)]
    fn nav_row(
        &self,
        caret: Option<bool>,
        icon_glyph: &str,
        icon_color: ColorU,
        text: &str,
        size: f32,
        color: ColorU,
        pad: f32,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        // Row chrome (background wash + selection highlight) is now supplied by
        // `row_shell`; the row itself only lays out its content over a 24px-tall
        // transparent hit surface.
        let row_h = 24.0;
        let bg_layer = ConstrainedBox::new(Rect::new().finish()).with_height(row_h).finish();

        let mut label_inner = Flex::row();
        if let Some(expanded) = caret {
            let glyph = if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            label_inner = label_inner.with_child(
                Container::new(self.icon(glyph, 9.0, theme::text_muted()))
                    .with_padding_right(3.0)
                    .finish(),
            );
        }
        label_inner = label_inner
            .with_child(
                Container::new(self.icon(icon_glyph, size, icon_color))
                    .with_padding_right(6.0)
                    .finish(),
            )
            .with_child(
                Text::new(text.to_string(), self.ui_font, size)
                    .with_color(color)
                    .finish(),
            );
        let label = Container::new(label_inner.finish())
            .with_padding_left(pad)
            .with_padding_top(4.0)
            .finish();
        let hit_layer = ConstrainedBox::new(Rect::new().finish())
            .with_height(row_h)
            .finish();
        let row = Stack::new()
            .with_child(bg_layer)
            .with_child(label)
            .with_child(hit_layer)
            .finish();

        EventHandler::new(row)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// A worktree row: caret + GIT_BRANCH icon + name, with an optional `+N -M` diff-stat
    /// badge pushed to the right side. `selected` drives the active background highlight.
    #[allow(clippy::too_many_arguments)]
    fn worktree_nav_row(
        &self,
        expanded: bool,
        name: &str,
        icon_color: ColorU,
        label_color: ColorU,
        diff_stat: (u32, u32),
        dirty: bool,
        indent: f32,
        rename_buf: Option<String>,
        action: CraneShellAction,
        plus_action: Option<CraneShellAction>,
    ) -> Box<dyn Element> {
        let size = 12.0_f32;
        // Row chrome (background wash + selection highlight) is supplied by
        // `row_shell`.
        let row_h = 24.0;
        let bg_layer = ConstrainedBox::new(Rect::new().finish()).with_height(row_h).finish();

        let caret_glyph = if expanded {
            icons::CARET_DOWN
        } else {
            icons::CARET_RIGHT
        };
        let mut row_inner = Flex::row()
            .with_child(
                Container::new(self.icon(caret_glyph, 9.0, theme::text_muted()))
                    .with_padding_right(3.0)
                    .finish(),
            )
            .with_child(
                Container::new(self.icon(icons::GIT_BRANCH, size, icon_color))
                    .with_padding_right(6.0)
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    // Inline rename: show an editable field (buffer + caret) on a
                    // highlighted bg in place of the branch name, mirroring the
                    // commit box's text rendering.
                    if let Some(buf) = &rename_buf {
                        Container::new(
                            Text::new(format!("{buf}|"), self.ui_font, size)
                                .with_color(theme::text())
                                .finish(),
                        )
                        .with_background_color(theme::row_active())
                        .with_padding_left(4.0)
                        .with_padding_right(4.0)
                        .with_padding_top(1.0)
                        .with_padding_bottom(1.0)
                        .finish()
                    } else {
                        Text::new(name.to_string(), self.ui_font, size)
                            .with_color(label_color)
                            .finish()
                    },
                )
                .finish(),
            );

        // +N / -M badges appended at right when there are line changes. Hidden
        // while renaming so the input has room.
        let (added, deleted) = if rename_buf.is_some() { (0, 0) } else { diff_stat };
        let dirty = dirty && rename_buf.is_none();
        // Dirty-dot fallback: the tree is dirty but `diff --numstat HEAD` shows
        // no line changes (e.g. only untracked files). Old egui paints a small
        // 3px filled add-colour dot so the branch still signals uncommitted
        // content. Rendered as a 6x6 fully-rounded (→ circular) success Rect.
        if added == 0 && deleted == 0 && dirty {
            row_inner = row_inner.with_child(
                Container::new(
                    ConstrainedBox::new(
                        Rect::new()
                            .with_background_color(theme::success())
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                            .finish(),
                    )
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
                )
                .with_padding_right(6.0)
                .finish(),
            );
        }
        if added > 0 {
            row_inner = row_inner.with_child(
                Container::new(
                    Text::new(format!("+{added}"), self.ui_font, size - 2.0)
                        .with_color(theme::success())
                        .finish(),
                )
                .with_padding_right(2.0)
                .finish(),
            );
        }
        if deleted > 0 {
            row_inner = row_inner.with_child(
                Container::new(
                    Text::new(format!("-{deleted}"), self.ui_font, size - 2.0)
                        .with_color(theme::error())
                        .finish(),
                )
                .with_padding_right(6.0)
                .finish(),
            );
        }

        let mut label = Container::new(row_inner.finish())
            .with_padding_left(indent)
            .with_padding_top(4.0);
        if plus_action.is_some() {
            // Reserve the right edge for the trailing "+" so the diff badge
            // never slides underneath it.
            label = label.with_padding_right(22.0);
        }
        let label = label.finish();
        let hit_layer = ConstrainedBox::new(Rect::new().finish())
            .with_height(row_h)
            .finish();
        let stack = Stack::new()
            .with_child(bg_layer)
            .with_child(label)
            .with_child(hit_layer)
            .finish();
        let base = EventHandler::new(stack)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish();
        match plus_action {
            None => base,
            Some(pa) => {
                // Hover-revealed trailing "+ new tab" on the row's right edge
                // (old Crane's hover affordance) — the overlay only joins the
                // stack while the pointer is over the row.
                let key = format!("wtplus:{pa:?}");
                let state = self.hover_handle(&key);
                let overlay = self.trailing_plus_overlay(row_h, pa, &key, "New Tab");
                Box::new(Hoverable::new(state, move |ms| {
                    if ms.is_hovered() {
                        Stack::new().with_child(base).with_child(overlay).finish()
                    } else {
                        base
                    }
                }))
            }
        }
    }

    /// A right-aligned "+" button layered over a tree row (topmost, so it wins
    /// the click against the row's hit layer). The row-item "New tab" / "New
    /// workspace" entries this replaces cost a full row of height each.
    fn trailing_plus_overlay(
        &self,
        row_h: f32,
        action: CraneShellAction,
        tooltip_key: &str,
        tooltip: &'static str,
    ) -> Box<dyn Element> {
        let plus = EventHandler::new(
            Container::new(self.icon(icons::PLUS, 10.0, theme::text_muted()))
                .with_padding_left(6.0)
                .with_padding_right(8.0)
                .with_padding_top((row_h - 10.0) / 2.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();
        let plus = self.with_tooltip(tooltip_key, tooltip, plus);
        Flex::row()
            .with_child(
                Expanded::new(
                    1.0,
                    ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish(),
                )
                .finish(),
            )
            .with_child(plus)
            .finish()
    }

    /// Wrap `inner` in an outer `Hoverable` that dispatches
    /// `ShowTooltip` / `HideTooltip` on hover-in / hover-out — purely for the
    /// tooltip side effect, so it doesn't alter `inner`'s own click/hover
    /// visuals (those stay owned by whatever `Hoverable`/`EventHandler`
    /// `inner` already wraps itself in). `key` must be stable and unique
    /// among tooltip-wrapped elements on screen at once (backs the
    /// persistent `MouseStateHandle`, keyed like every other hover row).
    fn with_tooltip(&self, key: &str, tooltip: &'static str, inner: Box<dyn Element>) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("tip:{key}"));
        Hoverable::new(state, move |_ms| inner)
            .on_hover(move |is_hovered, ctx, _app, pos| {
                if is_hovered {
                    ctx.dispatch_typed_action(CraneShellAction::ShowTooltip {
                        text: tooltip.to_string(),
                        x: pos.x(),
                        y: pos.y(),
                    });
                } else {
                    ctx.dispatch_typed_action(CraneShellAction::HideTooltip);
                }
            })
            .finish()
    }

    /// The hover-tooltip label itself: a small `surface()`-bg,
    /// `border()`-bordered, 4px-radius card, positioned near the cursor via
    /// the same `Popover` on-screen clamp used by the context-menu popovers.
    /// Purely decorative (no event handlers), so it never intercepts clicks.
    fn tooltip_overlay(&self, text: &str, x: f32, y: f32) -> Box<dyn Element> {
        let label = Container::new(
            Text::new(text.to_string(), self.ui_font, 10.0)
                .with_color(theme::text())
                .finish(),
        )
        .with_background_color(theme::surface())
        .with_border(Border::all(1.0).with_border_color(theme::border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish();
        Box::new(crate::warpui::rect_probe::Popover::new(label, x + 14.0, y + 18.0))
    }

    /// A tab row with a trailing close button. The close button's EventHandler returns
    /// `StopPropagation` so the outer select handler never fires when close is clicked.
    /// OSC-2 title of a terminal Tab, if one of its terminal panes set one.
    /// Prefers the focused pane (when it is a leaf of this tab), else the first
    /// leaf. Returns `None` for non-terminal tabs or when no title has arrived,
    /// so the caller falls back to the tab's own name ("Terminal N").
    fn terminal_tab_title(
        &self,
        key: (usize, usize, usize),
        app: &AppContext,
    ) -> Option<String> {
        let node = self.layouts.get(&key)?;
        let mut leaves = Vec::new();
        node.leaves(&mut leaves);
        let pid = self
            .focused
            .filter(|p| leaves.contains(p))
            .or_else(|| leaves.first().copied())?;
        if let Some(PaneContent::Terminal(h)) = self.panes.get(&pid) {
            let title = h.as_ref(app).title()?;
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
        None
    }

    /// The Tab row's terminal title after debouncing (see `TitleDebounce`).
    /// Returns `None` for non-terminal tabs / when no title has arrived, so the
    /// caller falls back to the tab's own "Terminal N" name — identical contract
    /// to `terminal_tab_title`, but the returned title only changes once the
    /// live one has held steady for `TITLE_STABLE_WINDOW`. Updates the per-tab
    /// debounce state as a side effect (hence the `RefCell`; `render` is `&self`).
    /// Debounce state is keyed by the tab id (`key.2`) alone: tab ids come from
    /// the single persisted `next_tab_id` counter so they are globally unique,
    /// and unlike the positional `(pi, wi)` prefix they never shift when
    /// projects are reordered or removed.
    fn stabilized_tab_title(&self, key: (usize, usize, usize), app: &AppContext) -> Option<String> {
        let live = self.terminal_tab_title(key, app)?;
        let mut map = self.title_debounce.borrow_mut();
        let entry = map.entry(key.2).or_default();
        Some(entry.observe(&live, std::time::Instant::now(), TITLE_STABLE_WINDOW))
    }

    fn tab_closeable_row(
        &self,
        icon_color: ColorU,
        name: &str,
        indent: f32,
        rename_buf: Option<String>,
        select_action: CraneShellAction,
        close_action: CraneShellAction,
    ) -> Box<dyn Element> {
        let size = 11.0_f32;
        // Row chrome (background wash + selection highlight) is supplied by
        // `row_shell`.
        let row_h = 24.0;
        let bg_layer = ConstrainedBox::new(Rect::new().finish()).with_height(row_h).finish();

        // Label: icon + text (no caret for tab leaves). While renaming, the text
        // becomes an editable field (buffer + caret) on a highlighted bg.
        let label_text: Box<dyn Element> = if let Some(buf) = &rename_buf {
            Container::new(
                Text::new(format!("{buf}|"), self.ui_font, size)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_background_color(theme::row_active())
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .finish()
        } else {
            Text::new(name.to_string(), self.ui_font, size)
                .with_color(icon_color)
                .finish()
        };
        let label_content = Flex::row()
            .with_child(
                Container::new(self.icon(icons::TERMINAL_WINDOW, size, icon_color))
                    .with_padding_right(6.0)
                    .finish(),
            )
            .with_child(label_text)
            .finish();
        let label = Container::new(label_content)
            .with_padding_left(indent)
            .with_padding_top(4.0)
            .finish();

        // Close button — inner EventHandler stops propagation so select doesn't fire.
        let hover_key = format!("tabx:{close_action:?}");
        let close_btn = EventHandler::new(
            Container::new(self.icon(icons::X, 9.0, theme::text_muted()))
                .with_padding_right(6.0)
                .with_padding_top(5.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(close_action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();

        // Compose inside a Hoverable: the × only joins the row while the
        // pointer is over it (old Crane's hover-revealed affordance).
        let state = self.hover_handle(&hover_key);
        Box::new(Hoverable::new(state, move |ms| {
            let mut row = Flex::row().with_child(Expanded::new(1.0, label).finish());
            if ms.is_hovered() {
                row = row.with_child(close_btn);
            }
            let stack = Stack::new()
                .with_child(bg_layer)
                .with_child(row.finish())
                .finish();
            EventHandler::new(stack)
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(select_action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish()
        }))
    }

    fn panel(&self, bg: warpui::color::ColorU, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(bg).finish())
            .with_child(content)
            .finish()
    }

    fn header(&self, text: &'static str) -> Box<dyn Element> {
        Container::new(
            Text::new(text, self.ui_font, 11.0)
                .with_color(theme::text_header())
                .finish(),
        )
        .with_uniform_padding(8.0)
        .finish()
    }

    fn row(&self, text: &'static str, color: warpui::color::ColorU) -> Box<dyn Element> {
        Container::new(Text::new(text, self.ui_font, 13.0).with_color(color).finish())
            .with_padding_left(12.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
    }

    /// A project-tree row at a given indent (owns its text — from session.json).
    fn tree_row(&self, text: &str, size: f32, color: warpui::color::ColorU, pad_left: f32) -> Box<dyn Element> {
        Container::new(
            Text::new(text.to_string(), self.ui_font, size)
                .with_color(color)
                .finish(),
        )
        .with_padding_left(pad_left)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .finish()
    }

    /// Shared Left-Panel row chrome: a rounded highlight with 4px side margins
    /// that washes on hover and stays lit for the selection tier. The pre-built
    /// row `inner` (with its own click / right-click handlers and hover-revealed
    /// overlays) is wrapped directly by a single `Hoverable` — the same outer
    /// idiom as `menu_item` / `trailing_plus_overlay`, so hover is detected
    /// reliably regardless of the enclosing Stack's event-dispatch mode. The
    /// closure runs once per frame-rebuild, reading the live hover state.
    fn row_shell(&self, key: &str, tier: RowTier, inner: Box<dyn Element>) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("lrow:{key}"));
        Hoverable::new(state, move |ms| {
            let bg = match (tier, ms.is_hovered()) {
                (RowTier::Selected, _) => theme::selection_wash(),
                (_, true) => theme::hover_wash(),
                (RowTier::Ancestor, false) => theme::context_wash(),
                (RowTier::Plain, false) => ColorU::new(0, 0, 0, 0),
            };
            Container::new(inner)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_margin_left(4.0)
                .with_margin_right(4.0)
                .finish()
        })
        .finish()
    }

    fn divider(&self) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
            .with_width(1.0)
            .finish()
    }

    fn left_sidebar(&self, app: &AppContext) -> Box<dyn Element> {
        // Header row: "PROJECTS" label with a trailing ＋ that opens Add Project
        // (mirrors the quiet footer row below). The Expanded spacer pushes the
        // button to the far right edge.
        let header_row = Container::new(
            Flex::row()
                .with_child(
                    Text::new("PROJECTS", self.ui_font, 11.0)
                        .with_color(theme::text_header())
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        Container::new(Text::new("", self.ui_font, 11.0).finish()).finish(),
                    )
                    .finish(),
                )
                .with_child(self.with_tooltip(
                    "projects-add",
                    "Add Project",
                    self.icon_button(
                        "projects-add",
                        icons::FOLDER_PLUS,
                        CraneShellAction::AddProject,
                    ),
                ))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(6.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish();

        // Real project tree loaded from ~/.crane/session.json: the user's
        // actual projects -> worktrees (branches) -> tabs.
        // Drop-zone rects repopulate as the rows paint this frame (visual
        // order). Snapshot last frame's list for render-time readers (the
        // drop-line overlay builds BEFORE this frame's paint refills it).
        *self.tree_zones_last.borrow_mut() =
            std::mem::take(&mut *self.tree_zones.borrow_mut());
        let mut col = Flex::column();
        if self.projects.is_empty() {
            col = col.with_child(self.tree_row(
                "No projects. Click + to add one.",
                12.0,
                theme::text_muted(),
                12.0,
            ));
        }
        // Tracks the group_path of the previous project so a FOLDER header is
        // emitted exactly once per contiguous run of same-group projects.
        let mut last_group: Option<String> = None;
        for (pi, p) in self.projects.iter().enumerate() {
            // Container folder groups: when the user opens a NON-git folder whose
            // immediate children are git repos, each child carries the container
            // folder's own path in `group_path`. Emit a collapsible FOLDER header
            // (label = container basename) once per contiguous run of children,
            // then nest the child projects one indent deeper. Projects the user
            // opened directly (git repo / loose folder) have `group_path == None`
            // and render flush-left exactly as before (group_offset == 0). We
            // NEVER group two separately-opened projects by a shared parent dir.
            let in_group = p.group_path.is_some();
            let group_collapsed = p
                .group_path
                .as_ref()
                .map(|g| self.collapsed_groups.contains(g))
                .unwrap_or(false);
            if in_group && p.group_path != last_group {
                let gp = p.group_path.clone().unwrap();
                let group_label = std::path::Path::new(&gp)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("group")
                    .to_string();
                let folder_glyph = if group_collapsed {
                    icons::FOLDER
                } else {
                    icons::FOLDER_OPEN
                };
                // Feature A: paint the folder icon + label in the group tint when
                // set (else muted default). Feature B: aggregate attention over
                // every member project while the group is collapsed.
                let gcolor = self.group_color_for(&gp);
                let group_since = if group_collapsed {
                    self.group_attention(&gp)
                } else {
                    None
                };
                let folder_base = self.nav_row(
                    Some(!group_collapsed),
                    folder_glyph,
                    gcolor,
                    &group_label,
                    13.0,
                    gcolor,
                    10.0,
                    CraneShellAction::ToggleGroup(gp.clone()),
                );
                // Container-folder headers are not part of the selection chain, so
                // they take the `Plain` tier (hover-only wash); their own group
                // tint still colours the folder glyph + label above.
                let folder_base = self.row_shell(&format!("group:{gp}"), RowTier::Plain, folder_base);
                // Right-click the header → folder-group context menu (tint /
                // remove whole group). Left-click still toggles collapse.
                let gp_menu = gp.clone();
                let folder_row = EventHandler::new(folder_base)
                    .on_right_mouse_down(move |ctx, _, pos| {
                        ctx.dispatch_typed_action(CraneShellAction::ShowFolderMenu {
                            group: gp_menu.clone(),
                            x: pos.x(),
                            y: pos.y(),
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                // Drag the header to reorder the whole group among root rows.
                col = col.with_child(self.tree_zone(
                    TreeScope::Root,
                    self.tree_draggable(
                        format!("g:{gp}"),
                        TreeDrag::Group { path: gp.clone() },
                        self.attention_wrap(folder_row, group_since, 21.0),
                    ),
                ));
            }
            last_group = p.group_path.clone();
            // Hide member projects (and their subtree) while the group is
            // collapsed — the header remains and can be re-expanded.
            if in_group && group_collapsed {
                continue;
            }
            // Group members indent one level in under the FOLDER header.
            let group_offset = if in_group { 14.0 } else { 0.0 };

            let p_expanded = self.expanded_projects.contains(&pi);
            // Two-tier selection: the project is `Selected` only when it is the
            // selection's chain root AND collapsed (so it is the deepest visible
            // row); when expanded it is an `Ancestor` (context) of the selected
            // tab below it. Everything else is `Plain`.
            let p_tier = match self.active_tab {
                Some((api, _, _)) if api == pi => {
                    if p_expanded { RowTier::Ancestor } else { RowTier::Selected }
                }
                _ => RowTier::Plain,
            };
            let tint = self.project_color_for(pi);
            // Label follows the tier; a `Selected` row also brightens its icon,
            // while `Ancestor`/`Plain` rows keep any explicit project tint on the
            // icon (the coloured CUBE stays recognisable in the chain).
            let pcol = match p_tier {
                RowTier::Selected => theme::text_hover(),
                RowTier::Ancestor => theme::text(),
                RowTier::Plain => theme::text_muted(),
            };
            let picon = if p_tier == RowTier::Selected {
                theme::text_hover()
            } else {
                tint
            };
            // Loose projects (non-git folders) use a FOLDER icon; git projects use CUBE.
            let project_icon = if p.is_loose { icons::FOLDER } else { icons::CUBE };
            let base_row = self.nav_row(
                Some(p_expanded),
                project_icon,
                picon,
                &p.name,
                13.0,
                pcol,
                10.0 + group_offset,
                CraneShellAction::ToggleProject(pi),
            );
            let base_row = self.row_shell(&format!("proj:{pi}"), p_tier, base_row);
            // Wrap in a second EventHandler to capture right-click for the
            // context menu without interfering with the left-click toggle.
            let project_row = EventHandler::new(base_row)
                .on_right_mouse_down(move |ctx, _, pos| {
                    ctx.dispatch_typed_action(CraneShellAction::ShowProjectMenu {
                        project_idx: pi,
                        x: pos.x(),
                        y: pos.y(),
                    });
                    DispatchEventResult::StopPropagation
                })
                .finish();
            // Feature B: a COLLAPSED project row aggregates attention over all its
            // tabs (expanded → its own tab rows carry the pulse instead).
            let project_since = if p_expanded { None } else { self.project_attention(pi) };
            // Trailing "+" on the project row's right edge: git projects open
            // the New Workspace modal; loose folders add a tab directly. This
            // replaces the old full-height "New workspace" / "New tab" row
            // items (user feedback: reduce tree height, plus lives on the row).
            let (plus_action, plus_tip) = if p.is_loose {
                (CraneShellAction::NewTabIn(pi, 0), "New Tab")
            } else {
                (CraneShellAction::OpenNewWorkspace { pi, branch: None }, "New Workspace")
            };
            let plus_key = format!("pplus:{}", p.path);
            let plus_state = self.hover_handle(&plus_key);
            let plus_overlay = self.trailing_plus_overlay(24.0, plus_action, &plus_key, plus_tip);
            let project_row = Box::new(Hoverable::new(plus_state, move |ms| {
                if ms.is_hovered() {
                    Stack::new()
                        .with_child(project_row)
                        .with_child(plus_overlay)
                        .finish()
                } else {
                    project_row
                }
            })) as Box<dyn Element>;
            // Drag to reorder: standalone projects move among root rows;
            // grouped members move only within their folder block.
            let p_scope = match &p.group_path {
                Some(g) => TreeScope::InBlock { group: g.clone() },
                None => TreeScope::Root,
            };
            col = col.with_child(self.tree_zone(
                p_scope,
                self.tree_draggable(
                    format!("p:{}", p.path),
                    TreeDrag::Project {
                        path: p.path.clone(),
                        group: p.group_path.clone(),
                    },
                    self.attention_wrap(project_row, project_since, 21.0),
                ),
            ));
            if !p_expanded {
                continue;
            }
            for (wi, w) in p.worktrees.iter().enumerate() {
                // FIX: loose (non-git) projects have NO branch/worktree row.
                // Render the worktree's tabs DIRECTLY under the project folder
                // at one indent, plus the "+ New tab" affordance — never the
                // bogus "(no git)" branch row. (Old egui flattens loose
                // projects; see ui/projects.rs is_loose handling.)
                if p.is_loose {
                    if let Some(tabs) = self.worktree_tabs.get(&(pi, wi)) {
                        for t in tabs {
                            let tkey = (pi, wi, t.id);
                            // A tab is a leaf: it is either the `Selected` row or
                            // `Plain`. Selection follows the active tab.
                            let t_tier = if self.active_tab == Some(tkey) {
                                RowTier::Selected
                            } else {
                                RowTier::Plain
                            };
                            let tab_tint = self.tab_tints.get(&(w.path.clone(), t.id)).copied();
                            let tcol = if t_tier == RowTier::Selected {
                                theme::text_hover()
                            } else if let Some([r, g, b]) = tab_tint {
                                ColorU::new(r, g, b, 255)
                            } else {
                                theme::text_muted()
                            };
                            let rbuf = self.tab_rename_buf(tkey);
                            let select = if rbuf.is_some() {
                                CraneShellAction::Noop
                            } else {
                                CraneShellAction::TabRowClick {
                                    key: tkey,
                                    path: PathBuf::from(&w.path),
                                }
                            };
                            let tab_base = self.tab_closeable_row(
                                tcol,
                                &t.name,
                                24.0 + group_offset,
                                rbuf,
                                select,
                                CraneShellAction::CloseTab(tkey),
                            );
                            let tab_base =
                                self.row_shell(&format!("term:{pi}:{wi}:{}", t.id), t_tier, tab_base);
                            let tab_row = self.tab_right_click(tab_base, tkey);
                            col = col.with_child(self.tree_zone(
                                TreeScope::Tab {
                                    project: p.path.clone(),
                                    worktree: w.path.clone(),
                                },
                                self.tree_draggable(
                                    format!("t:{}:{}", w.path, t.id),
                                    TreeDrag::Tab {
                                        project: p.path.clone(),
                                        worktree: w.path.clone(),
                                        id: t.id,
                                    },
                                    self.attention_wrap(tab_row, t.attention_since, 19.0),
                                ),
                            ));
                        }
                    }
                    continue;
                }
                let w_expanded = self.expanded_worktrees.contains(&(pi, wi));
                // Feature 2: the worktree row lights up as "active" when any of its tabs
                // is the current active tab, not only when the worktree row itself is
                // selected. This mirrors old egui Crane's `active_wt` flag.
                let w_active = self
                    .active_tab
                    .map(|(api, awi, _)| api == pi && awi == wi)
                    .unwrap_or(false);
                // Two-tier: the workspace is `Selected` only when it hosts the
                // active tab AND is collapsed (deepest visible row); expanded it
                // is an `Ancestor` of the selected tab below. Else `Plain`.
                let w_tier = if w_active {
                    if w_expanded { RowTier::Ancestor } else { RowTier::Selected }
                } else {
                    RowTier::Plain
                };
                // Tint priority: explicit per-worktree tint keeps colouring the
                // icon in the `Ancestor`/`Plain` tiers; the `Selected` row brightens
                // both icon and label to `text_hover`.
                let wt_tint = self.worktree_tints.get(&w.path).copied();
                let wcol = match w_tier {
                    RowTier::Selected => theme::text_hover(),
                    RowTier::Ancestor => theme::text(),
                    RowTier::Plain => theme::text_muted(),
                };
                let wicon = if w_tier == RowTier::Selected {
                    theme::text_hover()
                } else if let Some([r, g, b]) = wt_tint {
                    ColorU::new(r, g, b, 255)
                } else {
                    wcol
                };
                // Display-name override (per-path) wins over the branch name.
                let display = self
                    .worktree_names
                    .get(&w.path)
                    .cloned()
                    .unwrap_or_else(|| w.name.clone());
                let rbuf = self.worktree_rename_buf(pi, wi);
                // While renaming, the row click must not toggle expand.
                let wt_action = if rbuf.is_some() {
                    CraneShellAction::Noop
                } else {
                    CraneShellAction::WorktreeRowClick { pi, wi }
                };
                // Feature 1: pass the worktree's cached diff-stat to the row builder so
                // it renders the +N -M badge at the right side of the branch row.
                let wt_base = self.worktree_nav_row(
                    w_expanded,
                    &display,
                    wicon,
                    wcol,
                    w.diff_stat,
                    w.dirty,
                    24.0 + group_offset,
                    rbuf,
                    wt_action,
                    Some(CraneShellAction::NewTabIn(pi, wi)),
                );
                let wt_base = self.row_shell(&format!("wt:{pi}:{wi}"), w_tier, wt_base);
                // Right-click opens the worktree/branch context menu (mirrors the
                // project row) without disturbing the left-click toggle.
                let wt_row = EventHandler::new(wt_base)
                    .on_right_mouse_down(move |ctx, _, pos| {
                        ctx.dispatch_typed_action(CraneShellAction::ShowWorktreeMenu {
                            pi,
                            wi,
                            x: pos.x(),
                            y: pos.y(),
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                // Feature B: a COLLAPSED worktree row aggregates attention over its
                // tabs (expanded → its tab rows carry the pulse instead).
                let wt_since = if w_expanded { None } else { self.worktree_attention(pi, wi) };
                // Drag to reorder this Workspace among its project's siblings.
                col = col.with_child(self.tree_zone(
                    TreeScope::Worktree { project: p.path.clone() },
                    self.tree_draggable(
                        format!("w:{}", w.path),
                        TreeDrag::Worktree {
                            project: p.path.clone(),
                            path: w.path.clone(),
                        },
                        self.attention_wrap(wt_row, wt_since, 20.0),
                    ),
                ));
                if !w_expanded {
                    continue;
                }
                // Tabs from the LIVE model (worktree_tabs), keyed by stable id.
                if let Some(tabs) = self.worktree_tabs.get(&(pi, wi)) {
                    for t in tabs {
                        let tkey = (pi, wi, t.id);
                        // Leaf tab row: `Selected` when it is the active tab, else
                        // `Plain`.
                        let t_tier = if self.active_tab == Some(tkey) {
                            RowTier::Selected
                        } else {
                            RowTier::Plain
                        };
                        let tab_tint = self.tab_tints.get(&(w.path.clone(), t.id)).copied();
                        let tcol = if t_tier == RowTier::Selected {
                            theme::text_hover()
                        } else if let Some([r, g, b]) = tab_tint {
                            ColorU::new(r, g, b, 255)
                        } else {
                            theme::text_muted()
                        };
                        let rbuf = self.tab_rename_buf(tkey);
                        // Prefer the terminal's live OSC-2 title over the stored
                        // tab name — but never while this row is being renamed
                        // (the rename buffer owns the label then), and never once
                        // the user has explicitly renamed the tab (the pinned
                        // name wins, so the OSC title can't clobber it).
                        let display_name = if rbuf.is_some() || t.renamed {
                            t.name.clone()
                        } else {
                            self.stabilized_tab_title(tkey, app)
                                .unwrap_or_else(|| t.name.clone())
                        };
                        // Double-click → rename; single click → select. Noop while
                        // this row is the one being renamed.
                        let select = if rbuf.is_some() {
                            CraneShellAction::Noop
                        } else {
                            CraneShellAction::TabRowClick {
                                key: tkey,
                                path: PathBuf::from(&w.path),
                            }
                        };
                        // Feature 4: each tab row has a trailing close button.
                        // The close button's EventHandler returns StopPropagation so
                        // clicking it does not also trigger the row's select action.
                        let tab_base = self.tab_closeable_row(
                            tcol,
                            &display_name,
                            42.0 + group_offset,
                            rbuf,
                            select,
                            CraneShellAction::CloseTab(tkey),
                        );
                        let tab_base =
                            self.row_shell(&format!("term:{pi}:{wi}:{}", t.id), t_tier, tab_base);
                        // Feature B: the leaf Tab row carries its own attention
                        // pulse directly (cleared when the user opens the tab).
                        let tab_row = self.tab_right_click(tab_base, tkey);
                        col = col.with_child(self.tree_zone(
                            TreeScope::Tab {
                                project: p.path.clone(),
                                worktree: w.path.clone(),
                            },
                            self.tree_draggable(
                                format!("t:{}:{}", w.path, t.id),
                                TreeDrag::Tab {
                                    project: p.path.clone(),
                                    worktree: w.path.clone(),
                                    id: t.id,
                                },
                                self.attention_wrap(tab_row, t.attention_since, 19.0),
                            ),
                        ));
                    }
                }
            }
        }
        // Quiet "Add Project" footer, pinned at the bottom: a 1px divider on top
        // then a low-key PLUS + label that washes on hover — no boxed accent
        // pill. Same dispatch action as the header ＋.
        let icon_font = self.icon_font;
        let ui_font = self.ui_font;
        let add_state = self.hover_handle("footer-add-project");
        let add_body = Hoverable::new(add_state, move |ms| {
            let fg = if ms.is_hovered() {
                theme::text_hover()
            } else {
                theme::text_muted()
            };
            let bg = if ms.is_hovered() {
                theme::hover_wash()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(
                Flex::row()
                    .with_child(
                        Container::new(
                            Text::new(icons::PLUS.to_string(), icon_font, 11.0)
                                .with_color(fg)
                                .finish(),
                        )
                        .with_padding_right(6.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new("Add Project", ui_font, 11.0)
                            .with_color(fg)
                            .finish(),
                    )
                    // Expanded filler forces the row to consume the full
                    // incoming width (same idiom as `header_row`'s trailing
                    // spacer) so the background/hover wash spans the whole
                    // panel instead of shrink-wrapping to the label.
                    .with_child(
                        Expanded::new(
                            1.0,
                            Container::new(Text::new("", ui_font, 11.0).finish()).finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .with_background_color(bg)
            .with_padding_left(8.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(CraneShellAction::AddProject);
        })
        .finish();
        let add_footer = Flex::column()
            .with_child(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::divider()).finish(),
                )
                .with_height(1.0)
                .finish(),
            )
            .with_child(add_body)
            .finish();

        // Header, then the project list (fills), then the quiet Add Project footer.
        let outer = Flex::column()
            .with_child(header_row)
            .with_child(Expanded::new(1.0, col.finish()).finish())
            .with_child(add_footer)
            .finish();
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), outer)
    }

    fn tab_label(&self, text: &'static str, active: bool, action: CraneShellAction) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("rtab:{text}"));
        let ui_font = self.ui_font;
        let label_color = if active { theme::text_hover() } else { theme::text_muted() };
        let chip = Hoverable::new(state, move |ms| {
            let bg = if active {
                ColorU::new(0, 0, 0, 0)
            } else if ms.is_hovered() {
                theme::hover_wash()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let row = Text::new(text.to_string(), ui_font, 12.0)
                .with_color(label_color)
                .finish();
            // Underline: 2px accent for the active tab, transparent filler otherwise.
            let underline = ConstrainedBox::new(
                Rect::new()
                    .with_background_color(if active {
                        theme::accent()
                    } else {
                        ColorU::new(0, 0, 0, 0)
                    })
                    .finish(),
            )
            .with_height(2.0)
            .finish();
            Container::new(
                Flex::column()
                    .with_child(
                        Expanded::new(
                            1.0,
                            Container::new(row)
                                .with_padding_left(12.0)
                                .with_padding_right(12.0)
                                .with_padding_top(6.0)
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(underline)
                    .finish(),
            )
            .with_background_color(bg)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish();
        ConstrainedBox::new(chip).with_height(theme::TAB_H).finish()
    }

    /// Colour for a single-char git porcelain status. Mirrors old egui
    /// `status_color` (added/untracked = success, modified/renamed = warning,
    /// deleted/unmerged = error).
    fn status_color(c: char) -> ColorU {
        match c {
            'A' | '?' => theme::success(),
            'M' | 'R' | 'C' => theme::warning(),
            'D' | 'U' => theme::error(),
            _ => theme::text_muted(),
        }
    }

    fn file_row(&self, r: &file_tree::FileRow) -> Box<dyn Element> {
        let is_sel = self.selected_file.as_deref() == Some(r.path.as_path());
        let row_h = 21.0;
        let pad = 8.0 + r.depth as f32 * 14.0;
        let chevron: Box<dyn Element> = if r.is_dir {
            Container::new(self.icon(
                if r.expanded {
                    icons::CARET_DOWN
                } else {
                    icons::CARET_RIGHT
                },
                10.0,
                theme::text_muted(),
            ))
            .with_padding_right(4.0)
            .finish()
        } else {
            Self::spacer(13.0)
        };
        // Git-status colouring (old explorer.rs render_fs_dir): a changed file
        // shows its status LETTER as the leading glyph in the status colour and
        // tints the label; a directory with changed descendants keeps the folder
        // glyph but tints it. Clean rows use the default folder / file glyph.
        let (glyph, glyph_color, text_color): (String, ColorU, ColorU) = match r.git_status {
            Some('*') if r.is_dir => (icons::FOLDER.to_string(), theme::warning(), theme::text()),
            Some(c) if !r.is_dir => {
                let col = Self::status_color(c);
                let letter = if c == '?' { "?".to_string() } else { c.to_string() };
                (letter, col, col)
            }
            _ if r.is_dir => (icons::FOLDER.to_string(), theme::text_muted(), theme::text()),
            _ => (icons::FILE.to_string(), theme::text_muted(), theme::text_muted()),
        };
        let label_inner = Flex::row()
            .with_child(chevron)
            .with_child(
                ConstrainedBox::new(self.icon(&glyph, 12.0, glyph_color))
                    .with_width(16.0)
                    .finish(),
            )
            .with_child(
                Text::new(r.name.clone(), self.ui_font, 12.0)
                    .with_color(text_color)
                    .finish(),
            )
            .finish();
        let label = Container::new(label_inner)
            .with_padding_left(pad)
            .with_padding_top(3.0)
            .finish();
        // Drop-target highlight: while a Files-tree drag is in flight, tint
        // the dir row under the cursor (last frame's painted zone rect).
        let drop_hover = r.is_dir
            && self.fs_drag.is_some()
            && self
                .fs_drag_pos
                .get()
                .map(|c| {
                    self.fs_zones_last
                        .borrow()
                        .iter()
                        .any(|(rect, p)| p == &r.path && rect.contains_point(c))
                })
                .unwrap_or(false);
        let mut bg = Rect::new();
        if is_sel {
            bg = bg.with_background_color(theme::row_active());
        } else if drop_hover {
            bg = bg.with_background_color(theme::row_hover());
        }
        let bg_layer = ConstrainedBox::new(bg.finish()).with_height(row_h).finish();
        let hit_layer = ConstrainedBox::new(Rect::new().finish())
            .with_height(row_h)
            .finish();
        let row = Stack::new()
            .with_child(bg_layer)
            .with_child(label)
            .with_child(hit_layer)
            .finish();
        let action = if r.is_dir {
            CraneShellAction::ToggleDir(r.path.clone())
        } else {
            CraneShellAction::SelectFile(r.path.clone())
        };
        let menu_path = r.path.clone();
        let is_dir = r.is_dir;
        let clickable = EventHandler::new(
            EventHandler::new(row)
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish(),
        )
        .on_right_mouse_down(move |ctx, _app, pos| {
            ctx.dispatch_typed_action(CraneShellAction::ShowFileMenu {
                path: menu_path.clone(),
                is_dir,
                x: pos.x(),
                y: pos.y(),
            });
            DispatchEventResult::StopPropagation
        })
        .finish();
        // Every row is a drag source (move into another dir); dir rows are
        // also drop zones (rects recorded per paint).
        let dragged = self.fs_draggable(r.path.clone(), clickable);
        if r.is_dir {
            Box::new(crate::warpui::rect_probe::ZoneProbe::new(
                dragged,
                self.fs_zones.clone(),
                r.path.clone(),
            ))
        } else {
            dragged
        }
    }

    /// Wrap a Files-tree row in a `Draggable` carrying its source path —
    /// clicks pass through under the movement threshold; the release
    /// dispatches `FsDrop` with the cursor position.
    fn fs_draggable(&self, path: PathBuf, child: Box<dyn Element>) -> Box<dyn Element> {
        let key = format!("f:{}", path.display());
        let state = self
            .fs_drag_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .clone();
        let pos_cell = self.fs_drag_pos.clone();
        let pos_cell_drop = self.fs_drag_pos.clone();
        let state_drag = state.clone();
        let state_drop = state.clone();
        Box::new(
            Draggable::new(state, child)
                .on_drag_start(move |ctx, _app, _rect| {
                    ctx.dispatch_typed_action(CraneShellAction::FsDragStart(path.clone()));
                })
                .on_drag(move |_ctx, _app, rect, _data| {
                    let off = state_drag
                        .cursor_offset_within_element()
                        .unwrap_or_else(|| vec2f(0.0, 0.0));
                    pos_cell.set(Some(rect.origin() + off));
                })
                .on_drop(move |ctx, _app, rect, _data| {
                    let off = state_drop
                        .cursor_offset_within_element()
                        .unwrap_or_else(|| vec2f(0.0, 0.0));
                    let cursor = rect.origin() + off;
                    pos_cell_drop.set(None);
                    ctx.dispatch_typed_action(CraneShellAction::FsDrop {
                        x: cursor.x(),
                        y: cursor.y(),
                    });
                }),
        )
    }

    /// True when the active Project is a loose (non-git) folder — the Changes
    /// tab has nothing to show, so it is disabled and Files is forced.
    fn is_loose_active(&self) -> bool {
        self.projects
            .get(self.selected.0)
            .map(|p| p.is_loose)
            .unwrap_or(false)
    }

    fn right_sidebar(&self) -> Box<dyn Element> {
        let loose = self.is_loose_active();
        // Loose projects can't select Changes; its chip is greyed + inert and we
        // always render Files (mirrors old egui right_tab auto-switch).
        let show_changes = !self.files_tab && !loose;
        let tabs = Flex::row()
            .with_child(self.changes_tab_label(!self.files_tab && !loose, loose))
            .with_child(self.tab_label(
                "Files",
                self.files_tab || loose,
                CraneShellAction::SetTab { files: true },
            ))
            .finish();
        let tabs = Flex::column()
            .with_child(tabs)
            .with_child(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::divider()).finish(),
                )
                .with_height(1.0)
                .finish(),
            )
            .finish();

        let mut col = Flex::column().with_child(tabs);
        if show_changes {
            // Fixed header (branch + Push/Pull/Fetch) stays pinned above the
            // scroll region.
            col = col.with_child(self.changes_header());
            let mut list = Flex::column();
            if self.changes.is_empty() {
                list = list.with_child(self.tree_row(
                    "working tree clean",
                    12.0,
                    theme::text_muted(),
                    12.0,
                ));
            } else {
                let tree = self.build_change_tree();
                let mut rows: Vec<Box<dyn Element>> = Vec::new();
                self.change_node_rows(&tree, "", 0, &mut rows);
                for r in rows {
                    list = list.with_child(r);
                }
            }
            // Scroll the change rows so the commit box + Commit button stay
            // reachable no matter how many files changed.
            col = col.with_child(
                Expanded::new(1.0, self.scroll_list(list.finish())).finish(),
            );
            col = col.with_child(self.commit_box());
        } else {
            // FS drag-drop zones repopulate at paint; snapshot last frame's
            // list for the render-time drop-hover highlight.
            *self.fs_zones_last.borrow_mut() =
                std::mem::take(&mut *self.fs_zones.borrow_mut());
            let mut list = Flex::column();
            if let Some(p) = &self.pending_new_entry {
                list = list.with_child(self.pending_entry_row(p));
            }
            if self.file_rows.is_empty() {
                list = list.with_child(self.tree_row("(empty)", 12.0, theme::text_muted(), 12.0));
            }
            for r in &self.file_rows {
                list = list.with_child(self.file_row(r));
            }
            let mut body = self.scroll_list(list.finish());
            if let Some(root) = &self.active_cwd {
                // The whole list is (a) the root drop zone for internal drags
                // that miss every dir row and (b) the sink for OS file drops
                // (Finder → Crane), which copy into the dir under the cursor.
                body = Box::new(crate::warpui::rect_probe::ZoneProbe::new(
                    body,
                    self.fs_zones.clone(),
                    root.clone(),
                ));
                body = Box::new(crate::warpui::rect_probe::FileDropSink::new(
                    body,
                    Rc::new(|paths: &[String], loc, ectx| {
                        ectx.dispatch_typed_action(CraneShellAction::FsExternalDrop {
                            paths: paths.to_vec(),
                            x: loc.x(),
                            y: loc.y(),
                        });
                    }),
                ));
            }
            col = col.with_child(Expanded::new(1.0, body).finish());
        }
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), col.finish())
    }

    /// Wrap a Right-Panel row list in a vertical scroll container (theme-styled
    /// thumb, no track) keyed to `right_scroll`. Rule: every reusable scroll
    /// region carries stable scroll state so the list scrolls and pinned chrome
    /// (the commit box) stays reachable.
    fn scroll_list(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        ClippedScrollable::vertical(
            self.right_scroll.clone(),
            content,
            ScrollbarWidth::Auto,
            Fill::Solid(theme::border()),
            Fill::Solid(theme::text_muted()),
            Fill::None,
        )
        .finish()
    }

    /// The "Changes" tab chip. When the active Project is loose it renders greyed
    /// and inert (dispatches Noop) so it can't be selected.
    fn changes_tab_label(&self, active: bool, loose: bool) -> Box<dyn Element> {
        if loose {
            let label = Container::new(
                Text::new("Changes".to_string(), self.ui_font, 12.0)
                    .with_color(theme::pane_dim())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .finish();
            return ConstrainedBox::new(label).with_height(theme::TAB_H).finish();
        }
        self.tab_label("Changes", active, CraneShellAction::SetTab { files: false })
    }

    /// The shared git-op status, but ONLY when it belongs to the active repo.
    /// Push/Pull/Fetch/Commit run against `active_cwd` and flip a single
    /// app-wide `OpStatus` slot (now stamped with `repo` by `spawn_git_op` /
    /// `spawn_git_commit`). Without this gate, a Running spinner or a Failed
    /// pill from project A would bleed into project B's Changes tab the moment
    /// the user switches projects. Compare `op.repo` against the active cwd and
    /// hand back a fresh Idle default when they differ, so each project's
    /// Changes tab only reflects its OWN op.
    fn active_op_status(&self) -> crate::warpui::git::OpStatus {
        let op = self.git_op.lock().clone();
        match &self.active_cwd {
            Some(cwd) if op.repo == *cwd => op,
            _ => crate::warpui::git::OpStatus::default(),
        }
    }

    /// Branch + ahead/behind + Push/Pull/Fetch header at the top of the Changes
    /// area. Port of old egui `render_changes` top toolbar.
    fn changes_header(&self) -> Box<dyn Element> {
        let op = self.active_op_status();
        let running = op.is_running();
        let run_kind = if running { op.kind } else { None };

        let mut left = Flex::row()
            .with_child(
                Container::new(self.icon(icons::GIT_BRANCH, 12.0, theme::text()))
                    .with_padding_right(6.0)
                    .with_padding_top(4.0)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(
                        if self.branch.is_empty() { "—".to_string() } else { self.branch.clone() },
                        self.ui_font,
                        12.0,
                    )
                    .with_color(theme::text())
                    .finish(),
                )
                .with_padding_top(4.0)
                .finish(),
            );
        if let Some((ahead, behind)) = self.ahead_behind {
            if ahead > 0 {
                left = left.with_child(
                    Container::new(
                        Text::new(format!("{} {ahead}", icons::ARROW_UP), self.ui_font, 11.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(8.0)
                    .with_padding_top(5.0)
                    .finish(),
                );
            }
            if behind > 0 {
                left = left.with_child(
                    Container::new(
                        Text::new(format!("{} {behind}", icons::ARROW_DOWN), self.ui_font, 11.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(6.0)
                    .with_padding_top(5.0)
                    .finish(),
                );
            }
        }
        // Clicking the branch region opens the branch picker.
        let left = EventHandler::new(left.finish())
            .on_left_mouse_down(|ctx, _app, pos| {
                ctx.dispatch_typed_action(CraneShellAction::ShowBranchPicker {
                    x: pos.x(),
                    y: pos.y(),
                });
                DispatchEventResult::StopPropagation
            })
            .finish();

        // Push / Pull / Fetch buttons. The running op shows a spinner glyph; all
        // buttons disable while any op is in flight (guarded in the handler too).
        let buttons = Flex::row()
            .with_child(self.git_op_button(icons::ARROW_UP, crate::warpui::git::OpKind::Push, run_kind, running))
            .with_child(Self::spacer(4.0))
            .with_child(self.git_op_button(icons::ARROW_DOWN, crate::warpui::git::OpKind::Pull, run_kind, running))
            .with_child(Self::spacer(4.0))
            .with_child(self.git_op_button(icons::ARROW_COUNTER_CLOCKWISE, crate::warpui::git::OpKind::Fetch, run_kind, running))
            .finish();

        let row = Flex::row()
            .with_child(Expanded::new(1.0, left).finish())
            .with_child(buttons)
            .finish();
        Container::new(row)
            .with_padding_left(10.0)
            .with_padding_right(8.0)
            .with_padding_top(4.0)
            .with_padding_bottom(6.0)
            .finish()
    }

    /// A single Push/Pull/Fetch icon button. Shows a spinner glyph + accent when
    /// this op is the one running; renders muted while any op runs.
    fn git_op_button(
        &self,
        glyph: &str,
        kind: crate::warpui::git::OpKind,
        run_kind: Option<crate::warpui::git::OpKind>,
        any_running: bool,
    ) -> Box<dyn Element> {
        let this_running = run_kind == Some(kind);
        let (g, color) = if this_running {
            (icons::ARROW_COUNTER_CLOCKWISE, theme::accent())
        } else if any_running {
            (glyph, theme::pane_dim())
        } else {
            (glyph, theme::text_muted())
        };
        let action = match kind {
            crate::warpui::git::OpKind::Push => CraneShellAction::GitPush,
            crate::warpui::git::OpKind::Pull => CraneShellAction::GitPull,
            crate::warpui::git::OpKind::Fetch => CraneShellAction::GitFetch,
            crate::warpui::git::OpKind::Commit => CraneShellAction::Noop,
        };
        let content = ConstrainedBox::new(
            Container::new(self.icon(g, 13.0, color))
                .with_background_color(theme::surface())
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        )
        .with_width(28.0)
        .finish();
        EventHandler::new(content)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// Build the directory-grouped change tree from `self.changes` (port of old
    /// egui `build_tree`).
    fn build_change_tree(&self) -> ChangeDir {
        let mut root = ChangeDir::default();
        for c in &self.changes {
            let cleaned = c.path.trim_end_matches('/');
            if cleaned.is_empty() {
                continue;
            }
            let parts: Vec<&str> = cleaned.split('/').collect();
            let Some((file, dirs)) = parts.split_last() else { continue };
            if file.is_empty() {
                continue;
            }
            let mut node = &mut root;
            for d in dirs {
                node = node.dirs.entry((*d).to_string()).or_default();
            }
            let ch = c.status.chars().next().unwrap_or(' ');
            // Y column (worktree side). Set for a still-unstaged change; an
            // `MM` file has both X (`staged`) and Y set. Untracked ("??")
            // reports Y='?', which is a worktree change too.
            let y = c.xy.chars().nth(1).unwrap_or(' ');
            let has_unstaged = y != ' ';
            node.files.push(ChangeFile {
                name: (*file).to_string(),
                path: c.path.clone(),
                staged: c.staged,
                has_unstaged,
                status: ch,
            });
        }
        root
    }

    /// Recursively emit change-tree rows (dirs first, then files) into `out`.
    /// Port of old egui `render_change_node`.
    fn change_node_rows(
        &self,
        node: &ChangeDir,
        prefix: &str,
        depth: usize,
        out: &mut Vec<Box<dyn Element>>,
    ) {
        for (dir_name, child) in &node.dirs {
            let key = if prefix.is_empty() {
                dir_name.clone()
            } else {
                format!("{prefix}/{dir_name}")
            };
            let collapsed = self.collapsed_change_dirs.contains(&key);
            let (all_staged, any_staged) = child.staged_state();
            let mut paths = Vec::new();
            child.collect_paths(&mut paths);
            out.push(self.change_dir_row(dir_name, &key, depth, collapsed, all_staged, any_staged, paths));
            if !collapsed {
                self.change_node_rows(child, &key, depth + 1, out);
            }
        }
        for f in &node.files {
            out.push(self.change_file_row(f, depth));
        }
    }

    /// A collapsible directory row in the Changes tree. The leading +/- marker
    /// bulk-stages / unstages the whole subtree; the rest toggles collapse.
    #[allow(clippy::too_many_arguments)]
    fn change_dir_row(
        &self,
        name: &str,
        key: &str,
        depth: usize,
        collapsed: bool,
        all_staged: bool,
        _any_staged: bool,
        paths: Vec<String>,
    ) -> Box<dyn Element> {
        let indent = 8.0 + depth as f32 * 14.0;
        // Marker: '-' (unstage all) when the subtree is fully staged, else '+'.
        let (marker, marker_color) = if all_staged {
            (icons::MINUS, theme::success())
        } else {
            (icons::PLUS, theme::text_muted())
        };
        let stage_action = if all_staged {
            CraneShellAction::UnstagePaths(paths)
        } else {
            CraneShellAction::StagePaths(paths)
        };
        let marker_btn = EventHandler::new(
            Container::new(self.icon(marker, 11.0, marker_color))
                .with_background_color(theme::sidebar_bg())
                .with_padding_left(indent)
                .with_padding_right(4.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(stage_action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();

        let caret = if collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
        let label = Flex::row()
            .with_child(
                Container::new(self.icon(caret, 9.0, theme::text_muted()))
                    .with_padding_right(3.0)
                    .with_padding_top(4.0)
                    .finish(),
            )
            .with_child(
                Container::new(self.icon(icons::FOLDER, 12.0, theme::text_muted()))
                    .with_padding_right(5.0)
                    .with_padding_top(3.0)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(name.to_string(), self.ui_font, 12.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_padding_top(3.0)
                .finish(),
            )
            .finish();
        let toggle_action = CraneShellAction::ToggleChangeDir(key.to_string());
        let label_btn = EventHandler::new(
            Container::new(label).with_background_color(theme::sidebar_bg()).finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(toggle_action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();

        Flex::row()
            .with_child(marker_btn)
            .with_child(Expanded::new(1.0, label_btn).finish())
            .finish()
    }

    /// A file row in the Changes tree — +/- stage marker + status letter +
    /// name, opening the diff on click and a context menu on right-click.
    fn change_file_row(&self, f: &ChangeFile, depth: usize) -> Box<dyn Element> {
        let indent = 8.0 + depth as f32 * 14.0;
        let color = Self::status_color(f.status);
        // Marker click stages the WORKTREE change whenever one exists (so an
        // `MM` file's click stages its remaining worktree edit, matching old
        // Crane's tri-state). Only a file that is fully staged (index set,
        // worktree clean) offers Unstage. This inverts the naive `f.staged`
        // test, which wrongly unstaged an `MM` file on its first click.
        let (marker, marker_color) = if f.has_unstaged {
            (icons::PLUS, theme::text_muted())
        } else {
            (icons::MINUS, theme::success())
        };
        let stage_action = if f.has_unstaged {
            CraneShellAction::StagePaths(vec![f.path.clone()])
        } else {
            CraneShellAction::UnstagePaths(vec![f.path.clone()])
        };
        let marker_btn = EventHandler::new(
            Container::new(self.icon(marker, 11.0, marker_color))
                .with_background_color(theme::sidebar_bg())
                .with_padding_left(indent)
                .with_padding_right(4.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(stage_action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();

        let letter = if f.status == '?' { "?".to_string() } else { f.status.to_string() };
        let label_inner = Flex::row()
            .with_child(
                ConstrainedBox::new(
                    Text::new(letter, self.ui_font, 11.0).with_color(color).finish(),
                )
                .with_width(16.0)
                .finish(),
            )
            .with_child(
                Text::new(f.name.clone(), self.ui_font, 12.0)
                    .with_color(if f.staged { theme::text() } else { theme::text_muted() })
                    .finish(),
            )
            .finish();
        let abs = self
            .active_cwd
            .as_ref()
            .map(|c| c.join(&f.path))
            .unwrap_or_else(|| PathBuf::from(&f.path));
        let open_action = CraneShellAction::OpenDiff(abs);
        let label_btn = EventHandler::new(
            Container::new(label_inner)
                .with_background_color(theme::sidebar_bg())
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(open_action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish();

        let row = Flex::row()
            .with_child(marker_btn)
            .with_child(Expanded::new(1.0, label_btn).finish())
            .finish();
        // Right-click → Changes-row context menu.
        let menu_path = f.path.clone();
        let staged = f.staged;
        let has_unstaged = f.has_unstaged;
        EventHandler::new(row)
            .on_right_mouse_down(move |ctx, _app, pos| {
                ctx.dispatch_typed_action(CraneShellAction::ShowChangeMenu {
                    path: menu_path.clone(),
                    staged,
                    has_unstaged,
                    x: pos.x(),
                    y: pos.y(),
                });
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// Commit message field + Commit button at the bottom of the Changes tab.
    /// Button disables when nothing is staged OR the message is empty; label is
    /// "Commit to <branch> (<N staged>)". Commit / op errors show below.
    fn commit_box(&self) -> Box<dyn Element> {
        let staged = self.changes.iter().filter(|c| c.staged).count();
        let has_message = !self.commit_msg.trim().is_empty();
        let op = self.active_op_status();
        let any_running = op.is_running();
        let can_commit = staged > 0 && has_message && !any_running;

        let (text, color) = if self.commit_msg.is_empty() {
            (
                if staged > 0 { "Commit message".to_string() } else { "Stage files to commit".to_string() },
                theme::text_muted(),
            )
        } else {
            let caret = if self.commit_focused { "|" } else { "" };
            (format!("{}{}", self.commit_msg, caret), theme::text())
        };
        let field = EventHandler::new(
            Container::new(Text::new(text, self.ui_font, 12.0).with_color(color).finish())
                .with_background_color(if self.commit_focused {
                    theme::row_active()
                } else {
                    theme::surface()
                })
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::FocusCommit);
            DispatchEventResult::StopPropagation
        })
        .finish();

        // Primary Commit button — accent fill when enabled, dim when not. Label
        // shows the target branch + staged count so the user sees where it lands.
        let commit_running = any_running && op.kind == Some(crate::warpui::git::OpKind::Commit);
        let btn_label = if commit_running {
            format!("{}  Committing…", icons::ARROW_COUNTER_CLOCKWISE)
        } else {
            format!("{}  Commit to {} ({staged})", icons::CHECK, self.branch)
        };
        let (btn_bg, btn_fg) = if can_commit {
            (theme::accent(), ColorU::new(255, 255, 255, 255))
        } else {
            (theme::surface(), theme::pane_dim())
        };
        let commit_inner = Container::new(
            Text::new(btn_label, self.ui_font, 12.5).with_color(btn_fg).finish(),
        )
        .with_background_color(btn_bg)
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .finish();
        let commit_btn = EventHandler::new(commit_inner)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                if can_commit {
                    ctx.dispatch_typed_action(CraneShellAction::CommitStaged);
                }
                DispatchEventResult::StopPropagation
            })
            .finish();

        let mut column = Flex::column()
            .with_child(field)
            .with_child(Self::spacer(6.0))
            .with_child(commit_btn);

        // Status pill: in-flight op / last success / failure, else legacy error.
        let pill: Option<(String, ColorU)> = match &op.state {
            crate::warpui::git::OpState::Idle => self
                .commit_error
                .as_ref()
                .map(|e| (e.clone(), theme::error())),
            crate::warpui::git::OpState::Running => op
                .kind
                .map(|k| (format!("{}…", k.label()), theme::text_muted())),
            crate::warpui::git::OpState::Done(msg) => op
                .kind
                .map(|k| (format!("{}: {}", k.label(), msg), theme::success())),
            crate::warpui::git::OpState::Failed(err) => op.kind.map(|k| {
                let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or(err);
                (format!("{} failed: {}", k.label(), first.trim()), theme::error())
            }),
        };
        if let Some((msg, col)) = pill {
            column = column.with_child(Self::spacer(6.0)).with_child(
                Text::new(msg, self.ui_font, 11.0).with_color(col).finish(),
            );
        }

        Container::new(column.finish())
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
    }

    /// The inline new-file / new-folder editor row in the Files tree. Text is
    /// the live `pending_new_entry.name` with a caret; keys route here via
    /// `SendKeys`. Enter commits, Escape cancels (handled in `edit_new_entry`).
    fn pending_entry_row(&self, p: &PendingNewEntry) -> Box<dyn Element> {
        let glyph = if p.is_folder { icons::FOLDER } else { icons::FILE };
        let hint = if p.is_folder { "folder-name" } else { "filename.ext" };
        let shown = if p.name.is_empty() {
            hint.to_string()
        } else {
            format!("{}|", p.name)
        };
        let text_color = if p.name.is_empty() { theme::text_muted() } else { theme::text() };
        let row = Flex::row()
            .with_child(
                Container::new(self.icon(glyph, 12.0, theme::text_muted()))
                    .with_padding_left(22.0)
                    .with_padding_right(5.0)
                    .with_padding_top(3.0)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(shown, self.ui_font, 12.0).with_color(text_color).finish(),
                )
                .with_padding_top(3.0)
                .finish(),
            )
            .finish();
        let mut col = Flex::column().with_child(
            Container::new(row)
                .with_background_color(theme::row_active())
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .finish(),
        );
        if let Some(err) = &p.error {
            col = col.with_child(
                Container::new(
                    Text::new(err.clone(), self.ui_font, 10.5).with_color(theme::error()).finish(),
                )
                .with_padding_left(40.0)
                .finish(),
            );
        }
        col.finish()
    }

    /// Unified full-width top bar that doubles as the macOS titlebar: the
    /// left ~84px is left empty so the traffic-light buttons have room
    /// (this region is the draggable titlebar), the breadcrumb follows.
    /// A fixed-width horizontal gap. MUST bound height too — a width-only
    /// ConstrainedBox lets the inner Rect fill to infinite height in an
    /// unbounded-height row (warpui panics in validate_rect).
    fn spacer(w: f32) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().finish())
            .with_width(w)
            .with_height(1.0)
            .finish()
    }

    fn top_bar(&self) -> Box<dyn Element> {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Self::spacer(80.0)) // macOS traffic-light inset
            .with_child(self.icon_button("tb-left", icons::SIDEBAR, CraneShellAction::ToggleLeft))
            .with_child(Self::spacer(6.0))
            .with_child(self.breadcrumb_capsule());
        // Dirty diff-stat chip, immediately after the capsule (6px gap) — same
        // visibility rule and click target as before, just relocated from the
        // status bar into the top bar.
        if let Some(chip) = self.diff_chip() {
            row = row.with_child(Self::spacer(6.0));
            row = row.with_child(chip);
        }
        let row = row
            .with_child(Expanded::new(1.0, ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish()).finish())
            .with_child(self.new_pane_button())
            .with_child(Self::spacer(8.0))
            .with_child(self.icon_button("tb-gitlog", icons::GIT_BRANCH, CraneShellAction::OpenGitLog))
            .with_child(self.icon_button("tb-right", icons::SIDEBAR, CraneShellAction::ToggleRight))
            // ⚙ Settings — far right of the top bar (relocated from the removed
            // status bar). Opens the Settings modal directly (theme picker lives
            // in Settings > Appearance).
            .with_child(self.icon_button("tb-gear", icons::GEAR, CraneShellAction::OpenSettings))
            .with_child(Self::spacer(4.0))
            .finish();
        // Top-lit sheen: a 1px white-a10 rect pinned to the bar's top edge
        // (the scene graph has no gradient primitive), over the flat topbar_bg.
        let sheen = Flex::column()
            .with_child(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::topbar_sheen()).finish(),
                )
                .with_height(1.0)
                .finish(),
            )
            .finish();
        let stacked = Stack::new()
            .with_child(Rect::new().with_background_color(theme::topbar_bg()).finish())
            .with_child(sheen)
            .with_child(row)
            .finish();
        ConstrainedBox::new(stacked)
            .with_height(theme::TOPBAR_H)
            .finish()
    }

    /// The top-bar breadcrumb capsule: a rounded 24px pill showing the active
    /// project (CUBE + name) and, when the project is a git repo, its branch
    /// (accent GIT_BRANCH + name). Hover tints the border with `accent_soft()`
    /// and clicking opens the Switch Branch modal. A loose project (no git)
    /// shows its name only and is inert.
    ///
    /// Leading position carries the repo-pulse dot (7x7, `success()` clean /
    /// `warning()` dirty, sourced from `self.changes` — the same set the
    /// status bar used to read). `self.changes` is only ever populated by a
    /// git `status --porcelain` scan, so it has no meaning for a loose
    /// (non-git) project — the dot is shown only when the workspace has a
    /// branch (i.e. is a git repo), matching the existing `has_branch` gate.
    fn breadcrumb_capsule(&self) -> Box<dyn Element> {
        let (pi, wi, _ti) = self.selected;
        let project = self.projects.get(pi);
        let is_loose = project.map(|p| p.is_loose).unwrap_or(false);
        let proj_name = project.map(|p| p.name.clone()).unwrap_or_else(|| "Crane".to_string());
        let branch = if is_loose {
            None
        } else {
            project.and_then(|p| p.worktrees.get(wi)).map(|w| w.name.clone())
        };
        let has_branch = branch.is_some();
        let dot_color = if self.changes.is_empty() {
            theme::success()
        } else {
            theme::warning()
        };
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        let state = self.hover_handle("topbar:crumb");
        let capsule = Hoverable::new(state, move |ms| {
            let hovered = ms.is_hovered();
            let border_color = if has_branch && hovered {
                theme::accent_soft()
            } else {
                theme::border()
            };
            let mut inner = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            if has_branch {
                inner = inner.with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Rect::new()
                                .with_background_color(dot_color)
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.5)))
                                .finish(),
                        )
                        .with_width(7.0)
                        .with_height(7.0)
                        .finish(),
                    )
                    .with_padding_right(6.0)
                    .finish(),
                );
            }
            inner = inner
                .with_child(
                    Container::new(
                        Text::new(icons::CUBE.to_string(), icon_font, 11.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_right(5.0)
                    .finish(),
                )
                .with_child(
                    Text::new(proj_name.clone(), ui_font, 11.0)
                        .with_color(theme::text())
                        .finish(),
                );
            if let Some(b) = &branch {
                inner = inner
                    .with_child(
                        Container::new(
                            Text::new(icons::GIT_BRANCH.to_string(), icon_font, 11.0)
                                .with_color(theme::accent())
                                .finish(),
                        )
                        .with_padding_left(8.0)
                        .with_padding_right(5.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new(b.clone(), ui_font, 11.0)
                            .with_color(theme::text_hover())
                            .finish(),
                    );
            }
            ConstrainedBox::new(
                Container::new(inner.finish())
                    .with_background_color(theme::sidebar_bg())
                    .with_border(Border::all(1.0).with_border_color(border_color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .finish(),
            )
            .with_height(24.0)
            .finish()
        });
        // A loose project's capsule is inert — no click target, no pointer cursor.
        if has_branch {
            capsule
                .with_cursor(Cursor::PointingHand)
                .on_mouse_down(|ctx, _app, _pos| {
                    ctx.dispatch_typed_action(CraneShellAction::OpenSwitchBranch);
                })
                .finish()
        } else {
            capsule.finish()
        }
    }

    /// The ＋ New Pane button: a bordered 24px pill (PLUS + label + CARET_DOWN)
    /// that toggles the New Pane dropdown.
    fn new_pane_button(&self) -> Box<dyn Element> {
        let state = self.hover_handle("topbar:newpane");
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        Hoverable::new(state, move |ms| {
            let hovered = ms.is_hovered();
            let bg = if hovered { theme::selection_wash() } else { theme::sidebar_bg() };
            let inner = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Text::new(icons::PLUS.to_string(), icon_font, 11.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_right(5.0)
                    .finish(),
                )
                .with_child(
                    Text::new("New Pane".to_string(), ui_font, 11.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new(icons::CARET_DOWN.to_string(), icon_font, 9.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(6.0)
                    .finish(),
                )
                .finish();
            ConstrainedBox::new(
                Container::new(inner)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(theme::border()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .finish(),
            )
            .with_height(24.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::ToggleNewPaneMenu);
        })
        .finish()
    }

    /// The ＋ New Pane dropdown contents, rendered through the shared
    /// `menu_popover` overlay chrome so it dismisses like any context menu.
    fn new_pane_menu_overlay(&self, x: f32, y: f32) -> Box<dyn Element> {
        let items = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.menu_item_hint(
                icons::TERMINAL_WINDOW,
                "Terminal",
                Some("⌘T"),
                false,
                CraneShellAction::SplitFocused(Dir::Horizontal),
            ))
            .with_child(self.menu_item_hint(
                icons::GLOBE,
                "Browser",
                Some("⌘⇧U"),
                false,
                CraneShellAction::OpenBrowser,
            ))
            .with_child(self.menu_item_hint(
                icons::FILE,
                "File…",
                Some("⌘O"),
                false,
                CraneShellAction::OpenExternalFile,
            ))
            .with_child(self.menu_separator())
            .with_child(self.menu_label("SPLIT"))
            .with_child(self.menu_item_hint(
                icons::ARROW_RIGHT,
                "Split right",
                Some("⌘D"),
                false,
                CraneShellAction::SplitFocused(Dir::Horizontal),
            ))
            .with_child(self.menu_item_hint(
                icons::ARROW_DOWN,
                "Split down",
                Some("⌘⇧D"),
                false,
                CraneShellAction::SplitFocused(Dir::Vertical),
            ))
            .finish();
        self.menu_popover(items, x, y)
    }

    /// Dirty diff-stat chip: a rounded `surface()` pill with `+{added}`
    /// (success) / `-{deleted}` (error). Data is the active workspace's
    /// cached `git diff --numstat` totals — the SAME source the Left Panel
    /// branch badge uses. Shown only when the tree is dirty AND numstat
    /// reports line changes (untracked-only dirt has no counts; the
    /// breadcrumb dot already signals it). Clicking the chip opens Switch
    /// Branch, same as the capsule. Lives in the top bar, immediately after
    /// the breadcrumb capsule (moved out of the status bar).
    fn diff_chip(&self) -> Option<Box<dyn Element>> {
        let (added, deleted) = self
            .active_cwd
            .as_ref()
            .map(|cwd| cwd.to_string_lossy().to_string())
            .and_then(|s| {
                self.projects
                    .iter()
                    .flat_map(|p| p.worktrees.iter())
                    .find(|w| w.path == s)
                    .map(|w| w.diff_stat)
            })
            .unwrap_or((0, 0));
        if self.changes.is_empty() || (added == 0 && deleted == 0) {
            return None;
        }
        let ui_font = self.ui_font;
        let mut chip_row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        if added > 0 {
            chip_row = chip_row.with_child(
                Container::new(
                    Text::new(format!("+{added}"), ui_font, 10.0)
                        .with_color(theme::success())
                        .finish(),
                )
                .with_padding_right(if deleted > 0 { 4.0 } else { 0.0 })
                .finish(),
            );
        }
        if deleted > 0 {
            chip_row = chip_row.with_child(
                Text::new(format!("-{deleted}"), ui_font, 10.0)
                    .with_color(theme::error())
                    .finish(),
            );
        }
        let chip = ConstrainedBox::new(
            Container::new(Align::new(chip_row.finish()).finish())
                .with_background_color(theme::surface())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .finish(),
        )
        .with_height(18.0)
        .finish();
        let chip = EventHandler::new(chip)
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::OpenSwitchBranch);
                DispatchEventResult::StopPropagation
            })
            .finish();
        Some(chip)
    }

    fn center(&self, app: &AppContext) -> Box<dyn Element> {
        // Expand-to-full: render only the maximized pane.
        if let Some(id) = self.maximized {
            if self.panes.contains_key(&id) {
                return self.panel(theme::bg(), self.render_pane(id, app));
            }
        }
        // Otherwise render the active tab's split tree. Each leaf is a persistent
        // terminal pane (history retained); inactive tabs' panes stay alive.
        let body: Box<dyn Element> = match self.active_tab.and_then(|k| self.layouts.get(&k)) {
            Some(node) => self.render_node(node, app),
            None => Rect::new().with_background_color(theme::bg()).finish(),
        };
        self.panel(theme::bg(), body)
    }

    /// Recursively render a layout `Node` — leaves become terminal `ChildView`s,
    /// splits become draggable `SplitBox`es.
    fn render_node(&self, node: &Node, app: &AppContext) -> Box<dyn Element> {
        match node {
            Node::Leaf(id) => self.render_pane(*id, app),
            Node::Split {
                dir,
                ratio,
                dragging,
                first,
                second,
            } => SplitBox::new(
                *dir,
                self.render_node(first, app),
                self.render_node(second, app),
                ratio.clone(),
                dragging.clone(),
                theme::divider(),
            )
            .finish(),
        }
    }

    /// A leaf pane: header (drag handle) + terminal body, wrapped in a RectProbe
    /// that records the pane's window rect. Drag the header over another pane:
    /// the dock edge is computed 1:1 from the cursor position (`dock_zone`),
    /// shown as a half-pane preview, and applied on drop (edge=split, center=swap).
    fn render_pane(&self, id: PaneId, app: &AppContext) -> Box<dyn Element> {
        let inner: Box<dyn Element> = match self.panes.get(&id) {
            Some(PaneContent::Terminal(h)) => ChildView::new(h).finish(),
            Some(PaneContent::File(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Editor(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Welcome(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Browser(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Markdown(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Diff(h)) => ChildView::new(h).finish(),
            None => Rect::new().with_background_color(theme::bg()).finish(),
        };
        // Focus ring (canonical UI rule): with 2+ panes in the Layout, the
        // focused pane body gets a 2px accent border and the others a subtle
        // 1px border, so the active pane reads at a glance (paired with the
        // terminal-side dim of unfocused grids). A solo pane stays chromeless.
        let multi = self
            .active_tab
            .and_then(|t| self.layouts.get(&t))
            .map(|n| {
                let mut v = Vec::new();
                n.leaves(&mut v);
                v.len() > 1
            })
            .unwrap_or(false);
        let inner = if multi && self.focused == Some(id) {
            // Subtle: a 1px hairline in half-strength accent — the unfocused
            // panes' text dim does the heavy lifting; the ring just confirms.
            let mut ring = theme::accent();
            ring.a = 120;
            Container::new(inner)
                .with_border(Border::all(1.0).with_border_color(ring))
                .finish()
        } else {
            inner
        };
        // Click anywhere inside the pane body focuses it. `with_always_handle` so
        // it fires even when the child (e.g. the editor) consumes the click to
        // place its caret — otherwise clicking into the file wouldn't focus it.
        let is_editor = matches!(self.panes.get(&id), Some(PaneContent::Editor(_)));
        let drag_wake = self.ui_wake.clone();
        let body = EventHandler::new(inner)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FocusPane(id));
                DispatchEventResult::PropagateToParent
            })
            // A mouse-drag selection inside an editor updates the editor's own
            // view but not the shell — yet the shell renders the Ln/Col + "(N
            // chars)" status row. Ping the shell repaint waker so that row tracks
            // the drag live (as Shift+Arrow already does). Non-consuming, so the
            // editor still processes the drag to extend its selection.
            .on_mouse_dragged(move |_ctx, _app, _pos| {
                if is_editor {
                    drag_wake();
                }
                DispatchEventResult::PropagateToParent
            })
            .with_always_handle()
            .finish();
        let state = self.drag_states.get(&id).cloned().unwrap_or_default();

        // Leaves of the ACTIVE tab — restrict drop targets to these so a drag
        // can't hit a stale rect from an inactive tab (which would orphan the
        // dragged pane). Captured into on_drag.
        let active_leaves: Vec<PaneId> = self
            .active_tab
            .and_then(|t| self.layouts.get(&t))
            .map(|n| {
                let mut v = Vec::new();
                n.leaves(&mut v);
                v
            })
            .unwrap_or_default();

        // on_drag: cursor = dragged-rect origin + grab offset → dock zone.
        let drag_state = state.clone();
        let rects = self.pane_rects.clone();
        let preview_drag = self.drop_preview.clone();
        let preview_drop = self.drop_preview.clone();
        let header = Draggable::new(state, self.pane_header(id, app))
            .on_drag_start(move |ctx, _app, _rect| {
                ctx.dispatch_typed_action(CraneShellAction::FocusPane(id));
            })
            .on_drag(move |ctx, _app, rect, _data| {
                let off = drag_state
                    .cursor_offset_within_element()
                    .unwrap_or_else(|| vec2f(0.0, 0.0));
                let cursor = rect.origin() + off;
                let snapshot: Vec<(PaneId, RectF)> = rects
                    .borrow()
                    .iter()
                    .filter(|(k, _)| active_leaves.contains(k))
                    .map(|(k, v)| (*k, v.get()))
                    .collect();
                *preview_drag.borrow_mut() = pane_under(&snapshot, id, cursor);
                ctx.notify();
            })
            .on_drop(move |ctx, _app, _rect, _data| {
                if let Some((target, edge)) = preview_drop.borrow_mut().take() {
                    let act = if edge == DockEdge::Center {
                        CraneShellAction::SwapPanes { a: id, b: target }
                    } else {
                        CraneShellAction::DockPane {
                            src: id,
                            target,
                            edge,
                        }
                    };
                    ctx.dispatch_typed_action(act);
                }
            })
            .finish();

        // The File pane gets a second header row (the tab strip) between its
        // chrome header and body; Flex shrinks the body to make room.
        let mut col = Flex::column().with_child(header);
        if self.files_pane == Some(id) {
            col = col.with_child(self.file_tab_strip(app));
        }
        let content = col
            .with_child(Expanded::new(1.0, body).finish())
            .finish();
        let cell = self
            .pane_rects
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| Rc::new(Cell::new(RectF::new(vec2f(0.0, 0.0), vec2f(0.0, 0.0)))))
            .clone();
        let probed = RectProbe::new(content, cell).finish();

        let preview = *self.drop_preview.borrow();
        let is_preview = matches!(preview, Some((pid, _)) if pid == id);
        // Only one pane in the tab? Never dim (it's the active one).
        let single = self
            .active_tab
            .and_then(|t| self.layouts.get(&t))
            .map(|n| {
                let mut leaves = Vec::new();
                n.leaves(&mut leaves);
                leaves.len() <= 1
            })
            .unwrap_or(true);
        let _ = (single, is_preview);
        let mut stack = Stack::new().with_child(probed);
        // NOTE: no dim/overlay on inactive panes — a hit-recording Rect on top
        // would COVER and swallow clicks to the pane content (warpui's Rect
        // always records hits, with no opt-out), making file tabs / buttons in
        // a non-focused pane unclickable. Focus is still tracked for input
        // routing; a non-blocking indicator can live in the header later.
        // Drop preview painted last, above everything (only during a drag).
        if let Some((pid, edge)) = preview {
            if pid == id {
                stack = stack.with_child(self.zone_highlight(edge));
            }
        }
        stack.finish()
    }

    /// The half-pane (or full, for Center) highlight overlay for a dock edge —
    /// matches old Crane's `zone_rect`.
    fn zone_highlight(&self, edge: DockEdge) -> Box<dyn Element> {
        let hl = || -> Box<dyn Element> {
            Rect::new().with_background_color(theme::drop_zone()).finish()
        };
        let empty = || -> Box<dyn Element> { Rect::new().finish() };
        match edge {
            DockEdge::Center => hl(),
            DockEdge::Left => Flex::row()
                .with_child(Expanded::new(1.0, hl()).finish())
                .with_child(Expanded::new(1.0, empty()).finish())
                .finish(),
            DockEdge::Right => Flex::row()
                .with_child(Expanded::new(1.0, empty()).finish())
                .with_child(Expanded::new(1.0, hl()).finish())
                .finish(),
            DockEdge::Top => Flex::column()
                .with_child(Expanded::new(1.0, hl()).finish())
                .with_child(Expanded::new(1.0, empty()).finish())
                .finish(),
            DockEdge::Bottom => Flex::column()
                .with_child(Expanded::new(1.0, empty()).finish())
                .with_child(Expanded::new(1.0, hl()).finish())
                .finish(),
        }
    }

    /// The File pane's tab strip — second header row. Active tab: surface bg +
    /// 2px accent underline. Inactive: flat, hover wash. Per-tab ✕ closes the tab.
    fn file_tab_strip(&self, app: &AppContext) -> Box<dyn Element> {
        let mut strip = Flex::row();
        for (i, path) in self.file_pane_paths.iter().enumerate() {
            let active = i == self.file_pane_active;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let dirty = self
                .editor_views
                .get(path)
                .map(|h| h.as_ref(app).is_dirty(app))
                .unwrap_or(false);
            // Key hover state by the tab's path, not its index — otherwise a
            // close/reorder shifts indices and hover state migrates onto whatever
            // tab now sits at that index for a frame (same reason `lrow:` keys use
            // the row's path string).
            let path_key = path.to_string_lossy();
            let state = self.hover_handle(&format!("ftab:{path_key}"));
            let xstate = self.hover_handle(&format!("ftabx:{path_key}"));
            let ui_font = self.ui_font;
            let icon_font = self.icon_font;
            let label_color = if active { theme::text() } else { theme::text_muted() };
            let name_cl = name.clone();
            // The whole chip is one visual unit: label (+ dirty dot) and the ✕
            // share the chip's background, hover wash, and active underline. The
            // ✕ lives INSIDE the chip and keeps its own 16×16 selection_wash
            // hover box; its EventHandler returns StopPropagation so a ✕ click
            // closes the tab WITHOUT also triggering the chip's select handler
            // (same nested-handler idiom as `tab_closeable_row`).
            let chip = Hoverable::new(state, move |ms| {
                let bg = if active {
                    theme::surface()
                } else if ms.is_hovered() {
                    theme::hover_wash()
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let close = EventHandler::new(
                    Hoverable::new(xstate, move |xs| {
                        let (xbg, xfg) = if xs.is_hovered() {
                            (theme::selection_wash(), theme::text_hover())
                        } else {
                            (ColorU::new(0, 0, 0, 0), theme::text_muted())
                        };
                        ConstrainedBox::new(
                            Container::new(
                                Text::new(icons::X.to_string(), icon_font, 10.0)
                                    .with_color(xfg)
                                    .finish(),
                            )
                            .with_background_color(xbg)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                            .with_padding_left(3.0)
                            .with_padding_top(3.0)
                            .finish(),
                        )
                        .with_width(16.0)
                        .with_height(16.0)
                        .finish()
                    })
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(CraneShellAction::FileTabClose(i));
                    DispatchEventResult::StopPropagation
                })
                .finish();
                let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
                if dirty {
                    row = row.with_child(
                        Container::new(
                            Text::new(icons::CIRCLE.to_string(), icon_font, 8.0)
                                .with_color(theme::accent())
                                .finish(),
                        )
                        .with_padding_right(5.0)
                        .finish(),
                    );
                }
                row = row.with_child(
                    Text::new(name_cl.clone(), ui_font, 11.0)
                        .with_color(label_color)
                        .finish(),
                );
                // ✕ sits to the right of the label, inside the chip bounds.
                row = row.with_child(Container::new(close).with_padding_left(6.0).finish());
                // Underline: 2px accent for the active tab, transparent filler
                // otherwise. Spans the full chip width, ✕ region included.
                let underline = ConstrainedBox::new(
                    Rect::new()
                        .with_background_color(if active {
                            theme::accent()
                        } else {
                            ColorU::new(0, 0, 0, 0)
                        })
                        .finish(),
                )
                .with_height(2.0)
                .finish();
                EventHandler::new(
                    Container::new(
                        Flex::column()
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    Container::new(row.finish())
                                        .with_padding_left(12.0)
                                        .with_padding_right(6.0)
                                        .with_padding_top(6.0)
                                        .finish(),
                                )
                                .finish(),
                            )
                            .with_child(underline)
                            .finish(),
                    )
                    .with_background_color(bg)
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(CraneShellAction::FileTabSelect(i));
                    DispatchEventResult::StopPropagation
                })
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish();
            strip = strip.with_child(chip);
        }
        // Ln/Col + selection info for the File pane's ACTIVE editor, right-aligned
        // at the far end of the tab strip (relocated from the removed status bar).
        // Read the file pane's active tab specifically — not the globally focused
        // pane — since this strip only exists alongside the File pane. Markdown
        // tabs have no editor_views entry, so the readout simply hides for them.
        if let Some((ln, col, sel)) = self
            .file_pane_paths
            .get(self.file_pane_active)
            .and_then(|p| self.editor_views.get(p))
            .map(|h| {
                h.read(app, |v, a| {
                    let (l, c) = v.cursor_line_col(a);
                    (l, c, v.selection_info(a))
                })
            })
        {
            let mut text = format!("Ln {ln}, Col {col}");
            if let Some((chars, lines)) = sel {
                if lines > 1 {
                    text.push_str(&format!("   ({chars} chars, {lines} lines)"));
                } else {
                    text.push_str(&format!("   ({chars} chars)"));
                }
            }
            let ui_font = self.ui_font;
            // Flexible filler pushes the readout to the right edge of the strip.
            strip = strip.with_child(Expanded::new(1.0, Flex::row().finish()).finish());
            strip = strip.with_child(
                Container::new(
                    Align::new(
                        Text::new(text, ui_font, 10.5)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .finish(),
                )
                .with_padding_right(8.0)
                .finish(),
            );
        }
        ConstrainedBox::new(
            Flex::column()
                .with_child(
                    Expanded::new(
                        1.0,
                        Stack::new()
                            .with_child(
                                Rect::new()
                                    .with_background_color(theme::topbar_bg())
                                    .finish(),
                            )
                            .with_child(strip.finish())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(
                        Rect::new().with_background_color(theme::divider()).finish(),
                    )
                    .with_height(1.0)
                    .finish(),
                )
                .finish(),
        )
        .with_height(theme::TAB_H)
        .finish()
    }

    /// Pane header: title (click to focus) + expand-to-full + close.
    fn pane_header(&self, id: PaneId, app: &AppContext) -> Box<dyn Element> {
        let focused = self.focused == Some(id);
        let bg = if focused { theme::surface() } else { theme::topbar_bg() };
        // Selected pane's heading is painted in the accent — the same colour that
        // marks the active tab in the left panel — so the focused pane and its tab
        // read as one selection.
        let fg = if focused { theme::accent() } else { theme::text_muted() };
        let is_file_pane = self.files_pane == Some(id);

        // Row 1 is plain pane chrome (icon + title). The File pane's tab strip
        // now renders as a SECOND row beneath this header (see `file_tab_strip`),
        // so a File pane shows a static "Files" title here — pane close (the
        // row-1 ✕) is thus separate from per-tab closes.
        let title: Box<dyn Element> = {
            // Title + icon reflect the pane's content (Terminal is the default;
            // Welcome / Markdown / Diff panes name themselves; the File pane is
            // always "Files").
            let (glyph, label): (&'static str, String) = if is_file_pane {
                (icons::FILE, "Files".to_string())
            } else {
                match self.panes.get(&id) {
                    Some(PaneContent::Welcome(_)) => (icons::CUBE, "Welcome".to_string()),
                    Some(PaneContent::Markdown(h)) => {
                        (icons::FILE_TEXT, h.as_ref(app).title().to_string())
                    }
                    Some(PaneContent::Diff(h)) => {
                        (icons::GIT_DIFF, format!("Diff: {}", h.as_ref(app).title()))
                    }
                    Some(PaneContent::Browser(h)) => (icons::GLOBE, h.as_ref(app).title()),
                    _ => (icons::TERMINAL_WINDOW, "Terminal".to_string()),
                }
            };
            EventHandler::new(
                Container::new(
                    Flex::row()
                        .with_child(
                            Container::new(self.icon(glyph, 12.0, fg))
                                .with_padding_right(5.0)
                                .finish(),
                        )
                        .with_child(
                            Text::new(label, self.ui_font, 11.0)
                                .with_color(fg)
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_top(4.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FocusPane(id));
                DispatchEventResult::StopPropagation
            })
            .finish()
        };

        // The Expanded title fills the row, pushing these to the right edge.
        let buttons = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(self.icon_button(&format!("pane-max:{id}"), icons::ARROWS_OUT, CraneShellAction::ToggleMaximize(id)))
                .with_child(Self::spacer(2.0))
                .with_child(self.icon_button(&format!("pane-close:{id}"), icons::X, CraneShellAction::ClosePane(id)))
                .finish(),
        )
        .with_padding_right(4.0)
        .with_padding_top(2.0)
        .finish();

        let row = Flex::row()
            .with_child(Expanded::new(1.0, title).finish())
            .with_child(buttons)
            .finish();
        ConstrainedBox::new(
            Flex::column()
                .with_child(
                    Expanded::new(
                        1.0,
                        Stack::new()
                            .with_child(Rect::new().with_background_color(bg).finish())
                            .with_child(row)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                        .with_height(1.0)
                        .finish(),
                )
                .finish(),
        )
        .with_height(theme::HEADER_H)
        .finish()
    }

    /// Spawn a new persistent terminal pane rooted at `path`; returns its id.
    fn new_pane(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let handle = Self::spawn_terminal(ctx, path, self.ui_wake.clone());
        self.panes.insert(id, PaneContent::Terminal(handle));
        self.drag_states.insert(id, DraggableState::default());
        id
    }

    /// Insert `content` beside the focused pane (even split). Returns the id.
    fn split_with(&mut self, content: PaneContent) -> Option<PaneId> {
        self.split_with_at(content, false, 0.5)
    }

    /// Insert `content` beside the focused pane. `before` = new pane on the
    /// left/top; `ratio` = first-child width fraction.
    fn split_with_at(&mut self, content: PaneContent, before: bool, ratio: f32) -> Option<PaneId> {
        let tab = self.active_tab?;
        let target = self
            .focused
            .filter(|id| self.panes.contains_key(id))
            .or_else(|| self.layouts.get(&tab).map(|n| n.first_leaf()))?;
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        self.panes.insert(id, content);
        self.drag_states.insert(id, DraggableState::default());
        if let Some(node) = self.layouts.get_mut(&tab) {
            if node.split_leaf_at(target, id, Dir::Horizontal, before, ratio) {
                self.focused = Some(id);
                return Some(id);
            }
            self.panes.remove(&id);
        }
        None
    }

    /// Open `path` in the dedicated File pane (as a tab). Creates the pane the
    /// The file pane's id ONLY when it is a live document leaf in the ACTIVE
    /// tab's layout — the sole case where `open_file` may reuse it by swapping
    /// content. `self.panes` keeps panes from every tab alive, so a `files_pane`
    /// that was closed (orphaned in `panes`) or belongs to a background tab
    /// still matches a bare `panes.get()` check; reusing it would insert content
    /// into a pane that never renders, so the file pane "won't open". Returns
    /// None in those cases so the caller splits a fresh pane instead.
    fn reusable_files_pane(&self) -> Option<PaneId> {
        let fp = self.files_pane?;
        let is_doc = matches!(
            self.panes.get(&fp),
            Some(PaneContent::Editor(_))
                | Some(PaneContent::File(_))
                | Some(PaneContent::Markdown(_))
        );
        let in_active = self
            .active_tab
            .and_then(|t| self.layouts.get(&t))
            .map(|n| {
                let mut leaves = Vec::new();
                n.leaves(&mut leaves);
                leaves.contains(&fp)
            })
            .unwrap_or(false);
        (is_doc && in_active).then_some(fp)
    }

    /// first time; thereafter adds/switches a tab inside it (old Crane FilesPane).
    fn open_file(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        // Track the tab order + active index.
        if let Some(i) = self.file_pane_paths.iter().position(|p| p == &path) {
            self.file_pane_active = i;
        } else {
            self.file_pane_paths.push(path.clone());
            self.file_pane_active = self.file_pane_paths.len() - 1;
        }
        // Markdown files render read-only in a Markdown pane instead of the
        // editor (peer of the editor path below, same placement / reuse logic).
        let is_md = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
            .unwrap_or(false);
        if is_md {
            let handle = if let Some(h) = self.markdown_views.get(&path) {
                h.clone()
            } else {
                let p = path.clone();
                let h = ctx.add_typed_action_view(move |ctx| {
                    crate::warpui::markdown_view::WarpMarkdownView::new(ctx, p)
                });
                self.markdown_views.insert(path.clone(), h.clone());
                h
            };
            // Reuse the live files_pane (swap content) only when it's a document
            // leaf in the ACTIVE tab; else split a fresh pane on the RIGHT at
            // 0.35 (a closed / background-tab pane can't be reused).
            if let Some(fp) = self.reusable_files_pane() {
                self.panes.insert(fp, PaneContent::Markdown(handle));
                self.focused = Some(fp);
                return;
            }
            self.files_pane = None; // stale (closed or on another tab)
            self.files_pane = self.split_with_at(PaneContent::Markdown(handle), false, 0.35);
            ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
            return;
        }
        // Build the editor for this file once; reuse it on later opens/switches
        // so each tab keeps its own cursor / scroll / unsaved edits.
        let handle = if let Some(h) = self.editor_views.get(&path) {
            h.clone()
        } else {
            // Binary guard: `read_to_string().unwrap_or_default()` used to turn
            // an image/binary (or unreadable file) into an EMPTY editable
            // buffer — and a reflexive Cmd+S would then truncate the real file
            // to zero bytes. Refuse to open non-UTF-8 content as text: undo the
            // tab bookkeeping above and say why in a toast.
            let content = match std::fs::read(&path).map(String::from_utf8) {
                Ok(Ok(s)) => s,
                _ => {
                    self.file_pane_paths.retain(|p| p != &path);
                    if self.file_pane_active >= self.file_pane_paths.len() {
                        self.file_pane_active =
                            self.file_pane_paths.len().saturating_sub(1);
                    }
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let id = self.next_toast_id;
                    self.next_toast_id = self.next_toast_id.wrapping_add(1);
                    if self.toasts.len() >= TOAST_MAX {
                        self.toasts.pop_front();
                    }
                    self.toasts.push_back(Toast {
                        id,
                        body: format!("“{name}” is not a text file — use Reveal in Finder to open it externally."),
                        urgent: false,
                        source: "Files".to_string(),
                        tab_key: None,
                        at: std::time::Instant::now(),
                    });
                    ctx.notify();
                    return;
                }
            };
            let mono = warpui::fonts::Cache::handle(ctx)
                .update(ctx, |cache, _| crate::warpui::bundled_fonts::mono(cache));
            let p = path.clone();
            let goto = Self::lsp_goto_cb(path.clone());
            let h = ctx.add_typed_action_view(move |ctx| {
                crate::warpui::editor_view::WarpEditorView::new(ctx, content, mono, p)
                    .with_goto(goto)
            });
            // Apply the persisted editor prefs (Settings > Editor) to the fresh
            // buffer: word-wrap default + trim-on-save.
            let wrap = self.word_wrap_default;
            let trim = self.trim_on_save;
            h.update(ctx, |v, vctx| {
                if wrap {
                    v.set_word_wrap(true, vctx);
                }
                v.set_trim_on_save(trim);
            });
            self.editor_views.insert(path.clone(), h.clone());
            h
        };
        // Notify the LSP that this file is open (spawns the matching server on
        // first sight; a no-op when none is installed). Seed the sent-version so
        // the poll loop doesn't fire a redundant did_change on the first tick.
        // GATED: with the LSP opt-in OFF (default) we never did_open, so no
        // language server is spawned on file open.
        if self.lsp_enabled && !self.lsp.is_tracked(&path) {
            let content = handle.read(ctx, |v, app| v.buffer_text(app));
            self.lsp
                .did_open(&self.lsp_wake, &path, &content, &self.lsp_configs);
            let v0 = handle.read(ctx, |v, app| v.buffer_version(app));
            self.lsp_versions.insert(path.clone(), v0);
        }
        // Reuse the file pane only when it's a live document leaf in the ACTIVE
        // tab; a closed / background-tab pane can't be reused (swapping content
        // into it would render nothing — the "file pane won't open" bug).
        if let Some(fp) = self.reusable_files_pane() {
            self.panes.insert(fp, PaneContent::Editor(handle));
            self.focused = Some(fp);
            return;
        }
        if self.files_pane.is_some() {
            self.files_pane = None; // stale (closed or on another tab)
            self.file_pane_paths = vec![path.clone()];
            self.file_pane_active = 0;
        }
        // First open: File pane goes on the RIGHT and takes ~65% width (the
        // existing pane keeps 35% as the first child). Full height by default;
        // the user can drag the splitter to resize. Backed by Warp's REAL editor.
        self.files_pane = self.split_with_at(PaneContent::Editor(handle), false, 0.35);
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// The warp editor view handle for a pane, if it is an Editor pane.
    fn editor_at(&self, id: PaneId) -> Option<ViewHandle<crate::warpui::editor_view::WarpEditorView>> {
        match self.panes.get(&id) {
            Some(PaneContent::Editor(h)) => Some(h.clone()),
            _ => None,
        }
    }

    fn browser_at(
        &self,
        id: PaneId,
    ) -> Option<ViewHandle<crate::warpui::browser_view::WarpBrowserView>> {
        match self.panes.get(&id) {
            Some(PaneContent::Browser(h)) => Some(h.clone()),
            _ => None,
        }
    }

    /// Mark every open editor's gutter git-diff cache stale so it recomputes on
    /// the next paint. Called after a git op that can change the working-tree
    /// diff (stage / unstage / checkout / network op) — the "changes refresh"
    /// trigger, peer of the save-time invalidation the editor does itself.
    fn invalidate_editor_diffs(&self, app: &AppContext) {
        for h in self.editor_views.values() {
            h.read(app, |v, _| v.mark_diff_dirty());
        }
    }

    /// The pane content to show for an open file-tab `path`: a Markdown pane for
    /// `.md` docs (tracked in `markdown_views`), else the live Editor pane.
    fn file_tab_pane(&self, path: &PathBuf) -> Option<PaneContent> {
        if let Some(h) = self.markdown_views.get(path) {
            Some(PaneContent::Markdown(h.clone()))
        } else {
            self.editor_views
                .get(path)
                .map(|h| PaneContent::Editor(h.clone()))
        }
    }

    // ── LSP ──────────────────────────────────────────────────────────────────

    /// Build the goto-definition callback for an editor bound to `path`. On a
    /// Cmd+LeftClick the editor invokes this with the 0-based `(line, char)`
    /// under the cursor; we dispatch a shell action (deferred, so it runs after
    /// the editor's own update settles) that starts the LSP request.
    fn lsp_goto_cb(
        path: PathBuf,
    ) -> Rc<dyn Fn(u32, u32, &mut ViewContext<crate::warpui::editor_view::WarpEditorView>)> {
        Rc::new(move |line, character, ctx| {
            ctx.dispatch_typed_action_deferred(CraneShellAction::LspGoto {
                path: path.clone(),
                line,
                character,
            });
        })
    }

    /// The path of the editor currently shown in the File pane (the active file
    /// tab), if that tab is a real editor (not a Markdown preview).
    fn active_editor_path(&self) -> Option<PathBuf> {
        let path = self.file_pane_paths.get(self.file_pane_active)?.clone();
        self.editor_views.contains_key(&path).then_some(path)
    }

    /// 300ms poll: drain server state transitions, sync the active editor's
    /// content + diagnostics with the LSP, and pick up any resolved goto
    /// results. Runs off the `_lsp_tick` timer stream (and is cheap / silent
    /// when nothing changed).
    fn poll_lsp(&mut self, ctx: &mut ViewContext<Self>) {
        // LSP opt-in OFF (default): no-op the whole tick body. The timer stream
        // stays registered (it's cheap) but drives no server activity — no
        // tick, no did_change, no diagnostics fetch, no goto dispatch.
        if !self.lsp_enabled {
            return;
        }
        self.lsp.tick(&self.lsp_wake);
        if let Some(path) = self.active_editor_path() {
            if let Some(h) = self.editor_views.get(&path).cloned() {
                let (ver, text) =
                    h.read(ctx, |v, app| (v.buffer_version(app), v.buffer_text(app)));
                if !self.lsp.is_tracked(&path) {
                    self.lsp
                        .did_open(&self.lsp_wake, &path, &text, &self.lsp_configs);
                    self.lsp_versions.insert(path.clone(), ver);
                } else if self.lsp_versions.get(&path) != Some(&ver) {
                    self.lsp.did_change(&path, &text);
                    self.lsp_versions.insert(path.clone(), ver);
                }
                // Push diagnostics only when they changed — avoids re-rendering
                // (set_diagnostics notifies) on every idle tick.
                let diags = self.lsp.diagnostics(&path);
                let sig: Vec<(u32, u32, u32, u8)> = diags
                    .iter()
                    .map(|d| (d.line, d.col_start, d.col_end, d.severity))
                    .collect();
                if self.lsp_diag_sig.get(&path) != Some(&sig) {
                    self.lsp_diag_sig.insert(path.clone(), sig);
                    h.update(ctx, |v, c| v.set_diagnostics(diags, c));
                }
            }
        }
        self.drain_gotos(ctx);
    }

    /// Fire a goto-definition request for `path` at the 0-based `(line, char)`.
    /// Non-blocking: results are polled in `drain_gotos`. Ensures the file is
    /// opened on the server first (goto_dispatch only routes to tracked files).
    fn lsp_start_goto(
        &mut self,
        path: PathBuf,
        line: u32,
        character: u32,
        ctx: &mut ViewContext<Self>,
    ) {
        // LSP opt-in OFF (default): Cmd+click / F12 do NOT dispatch an LSP goto
        // (and never spawn a server). This is the single choke point for both
        // `LspGoto` and `LspGotoAtCursor`.
        if !self.lsp_enabled {
            return;
        }
        if !self.lsp.is_tracked(&path) {
            if let Some(h) = self.editor_views.get(&path).cloned() {
                let (ver, text) =
                    h.read(ctx, |v, app| (v.buffer_version(app), v.buffer_text(app)));
                self.lsp
                    .did_open(&self.lsp_wake, &path, &text, &self.lsp_configs);
                self.lsp_versions.insert(path.clone(), ver);
            }
        }
        let now = std::time::Instant::now();
        for (server, request_id) in self.lsp.goto_dispatch(&path, line, character) {
            self.pending_gotos.push(PendingGoto {
                server,
                request_id,
                dispatched_at: now,
            });
        }
        // Try once immediately in case the server already had the answer.
        self.drain_gotos(ctx);
    }

    /// Poll every in-flight goto request. Jump to the first location that
    /// resolves (dropping its siblings — multiple servers per file); a 5s
    /// watchdog prunes requests that never answer. Port of the egui app's
    /// goto-result drain.
    fn drain_gotos(&mut self, ctx: &mut ViewContext<Self>) {
        if self.pending_gotos.is_empty() {
            return;
        }
        let mut landed = false;
        let mut target: Option<crate::lsp::Location> = None;
        let mut pending = std::mem::take(&mut self.pending_gotos);
        pending.retain(|p| {
            if landed {
                return false;
            }
            if p.dispatched_at.elapsed() > std::time::Duration::from_secs(5) {
                return false;
            }
            match self.lsp.take_goto_result(p.server, p.request_id) {
                Some(Some(loc)) => {
                    target = Some(loc);
                    landed = true;
                    false
                }
                Some(None) => false,
                None => true,
            }
        });
        self.pending_gotos = pending;
        if let Some(loc) = target {
            self.goto_location(loc, ctx);
        }
    }

    /// Open the goto-definition target file at its line. `Location::line` is
    /// 0-based; `goto_line` takes a 1-based line.
    fn goto_location(&mut self, loc: crate::lsp::Location, ctx: &mut ViewContext<Self>) {
        let path = loc.path.clone();
        self.open_file(path.clone(), ctx);
        if let Some(h) = self.editor_views.get(&path).cloned() {
            h.update(ctx, |v, c| v.goto_line((loc.line as usize) + 1, c));
        }
        ctx.notify();
    }

    /// Toggle the Git Log bottom dock for the active worktree. On open, kicks
    /// an off-thread graph load (the fast tick also reloads on Workspace switch
    /// / ref change while the dock stays open).
    fn toggle_gitlog(&mut self, ctx: &mut ViewContext<Self>) {
        self.show_git_log = !self.show_git_log;
        if self.show_git_log {
            self.reload_git_log(ctx);
        }
    }

    /// Load the lane graph for the active repo OFF the UI thread (`git log` +
    /// `for-each-ref` + lane layout all run on the background executor via
    /// `ctx.spawn`; the callback swaps in the frame on the main thread). A
    /// generation guard drops stale results from a superseded reload. Switching
    /// repos clears the selection + detail + scroll so the pane never shows a
    /// commit from the previous Workspace.
    fn reload_git_log(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(repo) = self.active_cwd.clone() else {
            self.git_log_frame = None;
            self.git_log_loading = false;
            self.git_log_repo = None;
            return;
        };
        if self.git_log_repo.as_deref() != Some(repo.as_path()) {
            self.git_log_selected = None;
            self.git_log_detail = None;
            self.git_log_detail_loading = false;
            self.git_log_scroll.set(0.0);
            self.git_log_detail_scroll = 0;
        }
        self.git_log_repo = Some(repo.clone());
        self.git_log_loading = true;
        self.git_log_gen = self.git_log_gen.wrapping_add(1);
        self.git_log_last_reload = std::time::Instant::now();
        let load_gen = self.git_log_gen;
        let ref_filter = self.git_log_ref_filter.clone();
        let fut = async move {
            crate::warpui::git_log::load_graph_for(&repo, ref_filter.as_deref())
        };
        ctx.spawn(fut, move |this, frame, vctx| {
            // Ignore a result from a superseded reload (newer gen already ran).
            if this.git_log_gen != load_gen {
                return;
            }
            this.git_log_frame = Some(Rc::new(frame));
            this.git_log_loading = false;
            vctx.notify();
        });
    }

    /// Fast-tick backstop: reload the graph when the dock is open but the cached
    /// frame belongs to a different Workspace than the active one. Cheap no-op
    /// when the repo matches (the common case) since `reload_git_log` writes
    /// `git_log_repo` up front.
    fn git_log_tick(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.show_git_log {
            return;
        }
        if self.git_log_repo.as_deref() != self.active_cwd.as_deref() {
            self.reload_git_log(ctx);
        }
    }

    /// Typing routed to the git-log text-filter field. Chars append, Backspace
    /// pops, Enter/Escape drop focus (Escape also clears). Same simplified
    /// field model as the find bar. Arrow keys step the selection even while
    /// the field is focused (old log.rs let you filter-then-arrow).
    fn edit_git_log_filter(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        if ks.cmd || ks.ctrl {
            return;
        }
        match ks.key.as_str() {
            "up" | "down" => {
                let a = CraneShellAction::GitLogStepSelection(ks.key == "down");
                self.handle_action(&a, ctx);
            }
            "enter" => self.git_log_filter_active = false,
            "escape" => {
                self.git_log_filter.clear();
                self.git_log_filter_active = false;
            }
            "backspace" => {
                self.git_log_filter.pop();
            }
            "space" => self.git_log_filter.push(' '),
            k if k.chars().count() == 1 => self.git_log_filter.push_str(k),
            _ => {}
        }
    }

    /// Typing routed to the "create branch from commit" prompt. Enter runs
    /// `git branch <name> <sha>` off-thread; Escape cancels.
    fn edit_git_log_branch_prompt(
        &mut self,
        ks: &warpui::keymap::Keystroke,
        ctx: &mut ViewContext<Self>,
    ) {
        if ks.cmd || ks.ctrl {
            return;
        }
        let Some((sha, buf)) = self.git_log_branch_prompt.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "enter" => {
                let name = buf.trim().to_string();
                let sha = sha.clone();
                self.git_log_branch_prompt = None;
                self.git_log_menu = None;
                if name.is_empty() {
                    return;
                }
                let Some(repo) = self.active_cwd.clone() else {
                    return;
                };
                let fut =
                    async move { crate::warpui::git::branch_from(&repo, &name, &sha) };
                ctx.spawn(fut, move |this, res, vctx| {
                    if let Err(e) = res {
                        this.commit_error = Some(e);
                    }
                    this.reload_git_log(vctx);
                    vctx.notify();
                });
            }
            "escape" => {
                self.git_log_branch_prompt = None;
                self.git_log_menu = None;
            }
            "backspace" => {
                buf.pop();
            }
            k if k.chars().count() == 1 => buf.push_str(k),
            _ => {}
        }
    }

    /// The frame the commit list is currently displaying: the raw loaded frame,
    /// or — when the text filter is non-empty — the cached filtered frame
    /// (recomputed only when the needle or the load generation changes; the
    /// lane relayout over up to 10k commits must not run per paint).
    fn git_log_shown_frame(&self) -> Option<Rc<crate::warpui::git_log::GraphFrame>> {
        let frame = self.git_log_frame.clone()?;
        let needle = self.git_log_filter.trim().to_string();
        if needle.is_empty() {
            return Some(frame);
        }
        let mut cache = self.git_log_filtered.borrow_mut();
        if let Some((n, g, cached)) = cache.as_ref() {
            if *n == needle && *g == self.git_log_gen {
                return Some(cached.clone());
            }
        }
        let filtered = Rc::new(crate::warpui::git_log::filtered_frame(&frame, &needle));
        *cache = Some((needle, self.git_log_gen, filtered.clone()));
        Some(filtered)
    }

    /// Run a mutating commit op (checkout / cherry-pick / revert) off-thread,
    /// surface failure in the error banner, then refresh everything a repo
    /// mutation invalidates (graph, Changes, editor gutters).
    fn run_git_log_op(&mut self, ctx: &mut ViewContext<Self>, sha: String, op: GitLogOp) {
        let Some(repo) = self.active_cwd.clone() else {
            return;
        };
        let fut = async move {
            match op {
                GitLogOp::Checkout => crate::warpui::git::checkout_commit(&repo, &sha),
                GitLogOp::CherryPick => crate::warpui::git::cherry_pick(&repo, &sha),
                GitLogOp::Revert => crate::warpui::git::revert(&repo, &sha),
            }
        };
        ctx.spawn(fut, move |this, res, vctx| {
            if let Err(e) = res {
                this.commit_error = Some(e);
            }
            this.reload_git_log(vctx);
            this.refresh_panel(vctx);
            this.invalidate_editor_diffs(&*vctx);
            vctx.notify();
        });
    }

    /// A commit row was clicked: select it and load its `git show` detail
    /// off-thread. The callback only lands the detail if that commit is still
    /// the selected one (guards against a rapid re-click on another row).
    fn select_git_log_commit(&mut self, sha: String, ctx: &mut ViewContext<Self>) {
        self.git_log_selected = Some(sha.clone());
        self.git_log_detail = None;
        self.git_log_detail_loading = true;
        self.git_log_detail_scroll = 0;
        self.git_log_detail_file = 0;
        let Some(repo) = self.active_cwd.clone() else {
            self.git_log_detail_loading = false;
            return;
        };
        let sha_for_fut = sha.clone();
        let fut = async move { crate::warpui::git_log::load_detail(&repo, &sha_for_fut) };
        ctx.spawn(fut, move |this, detail, vctx| {
            if this.git_log_selected.as_deref() != Some(sha.as_str()) {
                return;
            }
            this.git_log_detail = Some(detail);
            this.git_log_detail_loading = false;
            vctx.notify();
        });
    }

    /// The Git Log dock body — a header strip (branch icon + title + commit
    /// count / loading state + close) over a horizontal split: the railroad
    /// commit list (custom lane element) on the left, the selected commit's
    /// message + diff on the right.
    fn git_log_dock(&self) -> Box<dyn Element> {
        // The list renders the FILTERED frame when the text filter is active
        // (cached per needle+generation); counts show "N of M" then.
        let shown = self.git_log_shown_frame();
        // ── Header strip ──────────────────────────────────────────────────
        let status = if self.git_log_loading && self.git_log_frame.is_none() {
            "loading…".to_string()
        } else if self.git_log_fetching {
            "fetching…".to_string()
        } else {
            match (&shown, &self.git_log_frame) {
                (Some(s), Some(f)) if s.commits.len() != f.commits.len() => {
                    format!("{} of {} commits", s.commits.len(), f.commits.len())
                }
                (_, Some(f)) => format!("{} commits", f.commits.len()),
                _ => "no commits".to_string(),
            }
        };
        let header = ConstrainedBox::new(
            Stack::new()
                .with_child(Rect::new().with_background_color(theme::topbar_bg()).finish())
                .with_child(
                    Flex::row()
                        .with_child(
                            Container::new(self.icon(icons::GIT_BRANCH, 12.0, theme::text_muted()))
                                .with_padding_left(10.0)
                                .with_padding_right(6.0)
                                .with_padding_top(6.0)
                                .finish(),
                        )
                        .with_child(
                            Container::new(
                                Text::new("Git Log".to_string(), self.ui_font, 11.0)
                                    .with_color(theme::text())
                                    .finish(),
                            )
                            .with_padding_top(6.0)
                            .with_padding_right(8.0)
                            .finish(),
                        )
                        .with_child(
                            Container::new(
                                Text::new(status, self.ui_font, 10.5)
                                    .with_color(theme::text_muted())
                                    .finish(),
                            )
                            .with_padding_top(7.0)
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.0,
                                ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish(),
                            )
                            .finish(),
                        )
                        .with_child(
                            self.icon_button(
                                "gitlog-refresh",
                                icons::ARROW_COUNTER_CLOCKWISE,
                                CraneShellAction::GitLogFetchAll,
                            ),
                        )
                        .with_child(self.icon_button("gitlog-close", icons::X, CraneShellAction::OpenGitLog))
                        .finish(),
                )
                .finish(),
        )
        .with_height(26.0)
        .finish();

        // ── Filter bar: text field + active-ref pill ──────────────────────
        let filter_bar = self.git_log_filter_bar();

        // ── Commit list (lane graph) ──────────────────────────────────────
        let list: Box<dyn Element> = match &shown {
            Some(frame) if !frame.commits.is_empty() => {
                Box::new(
                    crate::warpui::git_log_element::GitLogListElement::new(
                        frame.clone(),
                        self.mono_font,
                        12.0,
                        self.git_log_scroll.clone(),
                        self.git_log_selected.clone(),
                        self.git_log_hover.clone(),
                    )
                    .with_context_menu(Rc::new(|sha: &str, x, y, ectx| {
                        ectx.dispatch_typed_action(CraneShellAction::GitLogShowMenu {
                            sha: sha.to_string(),
                            x,
                            y,
                        });
                    })),
                ) as Box<dyn Element>
            }
            _ => {
                let msg = if self.git_log_loading {
                    "Loading commit graph…"
                } else {
                    "No commits to display"
                };
                self.panel(
                    theme::bg(),
                    Container::new(
                        Text::new(msg.to_string(), self.ui_font, 12.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_top(10.0)
                    .finish(),
                )
            }
        };

        // ── Detail / diff panel ───────────────────────────────────────────
        let detail = self.git_log_detail_panel();

        let body = Flex::row()
            .with_child(self.git_log_refs_column())
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_width(1.0)
                    .finish(),
            )
            .with_child(Expanded::new(1.4, list).finish())
            .with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                    .with_width(1.0)
                    .finish(),
            )
            .with_child(Expanded::new(1.0, detail).finish())
            .finish();

        Flex::column()
            .with_child(header)
            .with_child(filter_bar)
            .with_child(Expanded::new(1.0, body).finish())
            .finish()
    }

    /// The git-log text-filter row: a magnifier + simplified editable field
    /// (click to focus; typing routes via `SendKeys`) and, when a ref scope is
    /// active, a dismissible `ref ×` pill.
    fn git_log_filter_bar(&self) -> Box<dyn Element> {
        let mut row = Flex::row();
        row = row.with_child(
            Container::new(self.icon(icons::MAGNIFYING_GLASS, 11.0, theme::text_muted()))
                .with_padding_left(10.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .finish(),
        );
        let shown = if self.git_log_filter.is_empty() && !self.git_log_filter_active {
            "Filter commits (subject / hash / author)".to_string()
        } else {
            self.git_log_filter.clone()
        };
        let mut field = Flex::row().with_child(
            Text::new(shown, self.ui_font, 11.0)
                .with_color(
                    if self.git_log_filter.is_empty() && !self.git_log_filter_active {
                        theme::text_muted()
                    } else {
                        theme::text()
                    },
                )
                .finish(),
        );
        if self.git_log_filter_active {
            field = field.with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
                    .with_width(2.0)
                    .with_height(12.0)
                    .finish(),
            );
        }
        let field = EventHandler::new(
            Container::new(field.finish())
                .with_background_color(theme::bg())
                .with_border(Border::all(1.0).with_border_color(
                    if self.git_log_filter_active {
                        theme::accent()
                    } else {
                        theme::border()
                    },
                ))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(CraneShellAction::GitLogFocusFilter);
            DispatchEventResult::StopPropagation
        })
        .finish();
        row = row.with_child(Expanded::new(1.0, field).finish());
        if let Some(r) = &self.git_log_ref_filter {
            let label = r.clone();
            let pill = EventHandler::new(
                Container::new(
                    Flex::row()
                        .with_child(
                            Text::new(label, self.ui_font, 10.5)
                                .with_color(theme::accent())
                                .finish(),
                        )
                        .with_child(
                            Container::new(self.icon(icons::X, 9.0, theme::text_muted()))
                                .with_padding_left(5.0)
                                .finish(),
                        )
                        .finish(),
                )
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::accent()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_left(8.0)
                .with_padding_right(6.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .finish(),
            )
            .on_left_mouse_down(|ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::GitLogSetRefFilter(None));
                DispatchEventResult::StopPropagation
            })
            .finish();
            row = row.with_child(Container::new(pill).with_padding_left(6.0).finish());
        }
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_background_color(theme::topbar_bg())
                .with_border(Border::bottom(1.0).with_border_color(theme::divider()))
                .with_padding_right(10.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .with_height(24.0)
        .finish()
    }

    /// The refs column (old view/refs.rs): LOCAL / REMOTE / TAGS groups, each
    /// ref a click-to-scope row; the active scope row is accent-highlighted
    /// and clicking it again clears the scope.
    fn git_log_refs_column(&self) -> Box<dyn Element> {
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if let Some(frame) = &self.git_log_frame {
            for group in crate::warpui::git_log::ref_groups(&frame.refs) {
                // Section header: caps label over a hairline divider.
                col = col.with_child(
                    Container::new(
                        Text::new(group.title.to_string(), self.ui_font, 9.5)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_top(10.0)
                    .with_padding_bottom(3.0)
                    .finish(),
                );
                col = col.with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Rect::new().with_background_color(theme::divider()).finish(),
                        )
                        .with_height(1.0)
                        .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .finish(),
                );
                let glyph = match group.title {
                    "REMOTE" => icons::CLOUD,
                    "TAGS" => icons::TAG,
                    _ => icons::GIT_BRANCH,
                };
                for item in group.items {
                    let active =
                        self.git_log_ref_filter.as_deref() == Some(item.display.as_str());
                    // Truncate long ref names so the 170px column never
                    // clips mid-glyph (full name still shows in the pill).
                    let shown = if item.display.chars().count() > 22 {
                        let cut: String = item.display.chars().take(21).collect();
                        format!("{cut}…")
                    } else {
                        item.display.clone()
                    };
                    let name_color = if active {
                        theme::accent()
                    } else if item.is_head {
                        theme::text()
                    } else {
                        theme::text_muted()
                    };
                    let mut inner = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Container::new(self.icon(glyph, 10.0, name_color))
                                .with_padding_right(6.0)
                                .finish(),
                        )
                        .with_child(
                            Text::new(shown, self.ui_font, 11.0)
                                .with_color(name_color)
                                .finish(),
                        );
                    if item.is_head {
                        inner = inner
                            .with_child(Expanded::new(
                                1.0,
                                ConstrainedBox::new(Rect::new().finish())
                                    .with_height(1.0)
                                    .finish(),
                            )
                            .finish())
                            .with_child(
                                Container::new(self.icon(icons::CHECK, 9.0, theme::accent()))
                                    .with_padding_right(8.0)
                                    .finish(),
                            );
                    }
                    let name = item.display.clone();
                    let state =
                        self.hover_handle(&format!("glref:{}:{}", group.title, item.display));
                    let ui_row = Container::new(inner.finish())
                        .with_padding_left(12.0)
                        .with_padding_top(3.0)
                        .with_padding_bottom(3.0);
                    let active_bg = active;
                    let row = Hoverable::new(state, move |ms| {
                        ui_row
                            .with_background_color(if active_bg {
                                theme::row_active()
                            } else if ms.is_hovered() {
                                theme::row_hover()
                            } else {
                                ColorU::new(0, 0, 0, 0)
                            })
                            .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_mouse_down(move |ctx, _app, _pos| {
                        // `display` is what `git log <ref>` resolves (`main`,
                        // `origin/main`, tag name) — exactly the scope string.
                        ctx.dispatch_typed_action(CraneShellAction::GitLogSetRefFilter(
                            Some(name.clone()),
                        ));
                    })
                    .finish();
                    col = col.with_child(row);
                }
            }
        }
        ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.git_log_refs_scroll.clone(),
                Container::new(col.finish())
                    .with_background_color(theme::bg())
                    .finish(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish(),
        )
        .with_width(170.0)
        .finish()
    }

    /// The commit context menu (right-click on a lane row): checkout /
    /// branch-from / cherry-pick / revert / copy hash. Old log.rs menu, minus
    /// the worktree verb (worktrees are a Left Panel concept here).
    fn git_log_menu_overlay(&self, sha: &str, x: f32, y: f32) -> Box<dyn Element> {
        let short: String = sha.chars().take(7).collect();
        let mut items = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        items = items.with_child(self.menu_item(
            icons::GIT_BRANCH,
            &format!("Checkout {short} (detached)"),
            CraneShellAction::GitLogCheckout(sha.to_string()),
        ));
        items = items.with_child(self.menu_item(
            icons::PLUS,
            "Create branch from here…",
            CraneShellAction::GitLogBranchPrompt(sha.to_string()),
        ));
        items = items.with_child(self.menu_separator());
        items = items.with_child(self.menu_item(
            icons::CHECK,
            "Cherry-pick onto current",
            CraneShellAction::GitLogCherryPick(sha.to_string()),
        ));
        items = items.with_child(self.menu_item(
            icons::ARROW_COUNTER_CLOCKWISE,
            "Revert this commit",
            CraneShellAction::GitLogRevert(sha.to_string()),
        ));
        items = items.with_child(self.menu_separator());
        items = items.with_child(self.menu_item(
            icons::COPY,
            "Copy hash",
            CraneShellAction::CopyPathStr(sha.to_string()),
        ));
        self.menu_popover(items.finish(), x, y)
    }

    /// The inline "create branch from commit" prompt, anchored like a menu.
    /// Enter creates the branch (`git branch <name> <sha>`), Escape cancels.
    fn git_log_branch_prompt_overlay(&self, sha: &str, buf: &str) -> Box<dyn Element> {
        let short: String = sha.chars().take(7).collect();
        let col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new(format!("New branch at {short}"), self.ui_font, 11.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_padding_bottom(6.0)
                .finish(),
            )
            .with_child(
                Container::new(
                    Flex::row()
                        .with_child(
                            Text::new(buf.to_string(), self.ui_font, 12.0)
                                .with_color(theme::text())
                                .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Rect::new().with_background_color(theme::accent()).finish(),
                            )
                            .with_width(2.0)
                            .with_height(13.0)
                            .finish(),
                        )
                        .finish(),
                )
                .with_background_color(theme::bg())
                .with_border(Border::all(1.0).with_border_color(theme::accent()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new("Enter creates · Esc cancels".to_string(), self.ui_font, 10.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_padding_top(6.0)
                .finish(),
            )
            .finish();
        // Center-ish placement: reuse the menu popover chrome at a fixed spot.
        let (x, y) = self
            .git_log_menu
            .as_ref()
            .map(|(_, x, y)| (*x, *y))
            .unwrap_or((300.0, 300.0));
        self.menu_popover(
            ConstrainedBox::new(Container::new(col).with_padding_left(4.0).finish())
                .with_width(260.0)
                .finish(),
            x,
            y,
        )
    }

    /// The right-hand detail panel: the selected commit's message header then
    /// its patch, add/delete-tinted, manually scrolled via `GitLogDetailScroll`.
    fn git_log_detail_panel(&self) -> Box<dyn Element> {
        let tint = |c: warpui::color::ColorU, a: u8| warpui::color::ColorU {
            r: c.r,
            g: c.g,
            b: c.b,
            a,
        };
        let mut col = Flex::column();

        match &self.git_log_detail {
            _ if self.git_log_selected.is_none() => {
                col = col.with_child(
                    Container::new(
                        Text::new(
                            "Select a commit to view its changes".to_string(),
                            self.ui_font,
                            12.0,
                        )
                        .with_color(theme::text_muted())
                        .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_top(10.0)
                    .finish(),
                );
            }
            None => {
                let msg = if self.git_log_detail_loading {
                    "Loading commit…"
                } else {
                    "(no changes)"
                };
                col = col.with_child(
                    Container::new(
                        Text::new(msg.to_string(), self.ui_font, 12.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_top(10.0)
                    .finish(),
                );
            }
            Some(detail) => {
                // Commit message header (mono, first line emphasized).
                for (i, line) in detail.header.iter().enumerate() {
                    if line.is_empty() {
                        continue;
                    }
                    let color = if i == 0 { theme::text() } else { theme::text_muted() };
                    col = col.with_child(
                        Container::new(
                            Text::new(line.clone(), self.mono_font, 11.5)
                                .with_color(color)
                                .finish(),
                        )
                        .with_padding_left(12.0)
                        .with_padding_right(8.0)
                        .with_padding_top(1.0)
                        .finish(),
                    );
                }
                if !detail.diff.is_empty() {
                    col = col.with_child(
                        ConstrainedBox::new(
                            Rect::new().with_background_color(theme::divider()).finish(),
                        )
                        .with_height(1.0)
                        .finish(),
                    );
                }
                // Changed-files list (JetBrains style): one clickable row per
                // file with +/- counts; the selected file's patch renders
                // below instead of one monolithic dump.
                let sel_file = self.git_log_detail_file.min(detail.files.len().saturating_sub(1));
                for (fi, f) in detail.files.iter().enumerate() {
                    let active = fi == sel_file;
                    let mut frow = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Container::new(self.icon(icons::FILE, 10.0, theme::text_muted()))
                                .with_padding_right(6.0)
                                .finish(),
                        )
                        .with_child(
                            Text::new(f.path.clone(), self.ui_font, 11.5)
                                .with_color(if active { theme::text() } else { theme::text_muted() })
                                .finish(),
                        )
                        .with_child(Expanded::new(
                            1.0,
                            ConstrainedBox::new(Rect::new().finish()).with_height(1.0).finish(),
                        )
                        .finish());
                    if f.added > 0 {
                        frow = frow.with_child(
                            Container::new(
                                Text::new(format!("+{}", f.added), self.ui_font, 10.5)
                                    .with_color(theme::success())
                                    .finish(),
                            )
                            .with_padding_right(4.0)
                            .finish(),
                        );
                    }
                    if f.deleted > 0 {
                        frow = frow.with_child(
                            Container::new(
                                Text::new(format!("-{}", f.deleted), self.ui_font, 10.5)
                                    .with_color(theme::error())
                                    .finish(),
                            )
                            .with_padding_right(4.0)
                            .finish(),
                        );
                    }
                    let row = EventHandler::new(
                        Container::new(frow.finish())
                            .with_background_color(if active {
                                theme::row_active()
                            } else {
                                ColorU::new(0, 0, 0, 0)
                            })
                            .with_padding_left(12.0)
                            .with_padding_right(8.0)
                            .with_padding_top(2.0)
                            .with_padding_bottom(2.0)
                            .finish(),
                    )
                    .on_left_mouse_down(move |ctx, _app, _pos| {
                        ctx.dispatch_typed_action(CraneShellAction::GitLogDetailFile(fi));
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                    col = col.with_child(row);
                }
                if !detail.files.is_empty() {
                    col = col.with_child(
                        ConstrainedBox::new(
                            Rect::new().with_background_color(theme::divider()).finish(),
                        )
                        .with_height(1.0)
                        .finish(),
                    );
                }
                // Selected file's patch, windowed by the detail scroll offset.
                // (Falls back to the whole diff when the split found no files —
                // e.g. a merge commit rendered without -m.)
                use crate::warpui::git_log::DiffLineKind;
                let lines: &[crate::warpui::git_log::DiffLine] =
                    match detail.files.get(sel_file) {
                        Some(f) => &f.lines,
                        None => &detail.diff,
                    };
                let start = self
                    .git_log_detail_scroll
                    .min(lines.len().saturating_sub(1));
                for dl in lines.iter().skip(start).take(1500) {
                    let (fg, bg) = match dl.kind {
                        DiffLineKind::Add => (theme::success(), tint(theme::success(), 38)),
                        DiffLineKind::Del => (theme::error(), tint(theme::error(), 38)),
                        DiffLineKind::Hunk => (theme::accent(), tint(theme::accent(), 22)),
                        DiffLineKind::FileHeader => (theme::text_muted(), tint(theme::bg(), 0)),
                        DiffLineKind::Context => (theme::text(), tint(theme::bg(), 0)),
                    };
                    col = col.with_child(
                        Container::new(
                            Text::new(dl.text.clone(), self.mono_font, 11.0)
                                .with_color(fg)
                                .finish(),
                        )
                        .with_background_color(bg)
                        .with_padding_left(12.0)
                        .with_padding_right(8.0)
                        .finish(),
                    );
                }
            }
        }

        // Scroll wheel adjusts the detail row window (same manual-scroll feel as
        // WarpDiffView).
        let scroll_body = EventHandler::new(Expanded::new(1.0, col.finish()).finish())
            .on_scroll_wheel(move |ctx, _app, delta, _mods| {
                let lines = (-delta.y() / 8.0).round() as i32;
                if lines != 0 {
                    ctx.dispatch_typed_action(CraneShellAction::GitLogDetailScroll(lines));
                }
                DispatchEventResult::StopPropagation
            })
            .finish();
        self.panel(
            theme::bg(),
            Flex::column()
                .with_child(Expanded::new(1.0, scroll_body).finish())
                .finish(),
        )
    }

    /// Open a placeholder Browser pane (WKWebView embed pending).
    fn open_browser(&mut self, ctx: &mut ViewContext<Self>) {
        self.ensure_active_tab(ctx);
        self.open_browser_with(Vec::new(), 0, ctx);
    }

    /// Create a Browser Pane beside the focused pane, optionally seeded with
    /// restored `(url, title)` tabs. The view is constructed with the PaneId
    /// that `split_with` is about to assign (peeked from `next_pane_id`) —
    /// it keys the native webview slots by `(pane, tab)`.
    fn open_browser_with(
        &mut self,
        tabs: Vec<(String, String)>,
        active: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane_id = self.next_pane_id;
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        let handle = ctx.add_typed_action_view(move |_ctx| {
            crate::warpui::browser_view::WarpBrowserView::new(
                pane_id, ui_font, icon_font, tabs, active,
            )
        });
        self.split_with(PaneContent::Browser(handle));
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// Open a read-only unified Diff pane (HEAD vs working copy) for `path` in a
    /// fresh pane beside the focused one (same placement as `open_browser`).
    fn open_diff(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        self.ensure_active_tab(ctx);
        let repo_root = self.active_cwd.clone();
        let handle = ctx.add_typed_action_view(move |ctx| {
            crate::warpui::diff_view::WarpDiffView::new(ctx, repo_root, path)
        });
        self.split_with(PaneContent::Diff(handle));
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// Open the Welcome / landing pane beside the focused pane. Its action cards
    /// dispatch a `WelcomeAction` that this closure maps to the matching shell
    /// action (mirrors the top-bar pills). Created with `add_typed_action_view`
    /// so the shell is recorded as the pane's responder-chain parent — without
    /// that, the card's `CraneShellAction` would never bubble up to the shell.
    fn open_welcome(&mut self, ctx: &mut ViewContext<Self>) {
        self.ensure_active_tab(ctx);
        let on_action: WelcomeCallback = Rc::new(|action, ectx| match action {
            WelcomeAction::Terminal => {
                ectx.dispatch_typed_action(CraneShellAction::SplitFocused(Dir::Horizontal))
            }
            WelcomeAction::Files => ectx.dispatch_typed_action(CraneShellAction::ToggleRight),
            WelcomeAction::Browser => ectx.dispatch_typed_action(CraneShellAction::OpenBrowser),
        });
        let (ui_font, icon_font) = (self.ui_font, self.icon_font);
        let handle = ctx.add_typed_action_view(move |vc| {
            WarpWelcomeView::new(vc, ui_font, icon_font, Some(on_action))
        });
        self.split_with(PaneContent::Welcome(handle));
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// The live rename buffer for worktree (pi, wi), or None when that row is
    /// not the active rename target.
    fn worktree_rename_buf(&self, pi: usize, wi: usize) -> Option<String> {
        self.renaming.as_ref().and_then(|r| match &r.target {
            RenameTarget::Worktree { pi: rp, wi: rw } if *rp == pi && *rw == wi => {
                Some(r.buffer.clone())
            }
            _ => None,
        })
    }

    /// The live rename buffer for tab `key`, or None when that row is not the
    /// active rename target.
    fn tab_rename_buf(&self, key: (usize, usize, usize)) -> Option<String> {
        self.renaming.as_ref().and_then(|r| match &r.target {
            RenameTarget::Tab { key: k } if *k == key => Some(r.buffer.clone()),
            _ => None,
        })
    }

    /// Apply a keystroke to the active inline rename buffer. Enter commits,
    /// Escape cancels, Backspace deletes, printable chars append — mirrors
    /// `edit_commit` / `edit_new_entry`.
    fn edit_rename(&mut self, ks: &warpui::keymap::Keystroke) {
        match ks.key.as_str() {
            "enter" | "return" | "numpadenter" => self.commit_rename(),
            "escape" => self.renaming = None,
            "backspace" => {
                if let Some(r) = self.renaming.as_mut() {
                    r.buffer.pop();
                }
            }
            k if k.chars().count() == 1 => {
                if let Some(r) = self.renaming.as_mut() {
                    r.buffer.push_str(k);
                }
            }
            _ => {}
        }
    }

    /// Commit the active inline rename: a worktree rename stores a per-path
    /// display-name override; a tab rename updates `TabMeta.name`. Empty names
    /// cancel. Persistence happens via the global save at the end of the action.
    fn commit_rename(&mut self) {
        let Some(r) = self.renaming.take() else { return };
        let name = r.buffer.trim().to_string();
        if name.is_empty() {
            return;
        }
        match r.target {
            RenameTarget::Worktree { pi, wi } => {
                if let Some(w) = self.projects.get(pi).and_then(|p| p.worktrees.get(wi)) {
                    self.worktree_names.insert(w.path.clone(), name);
                }
            }
            RenameTarget::Tab { key } => {
                let (pi, wi, tid) = key;
                if let Some(tabs) = self.worktree_tabs.get_mut(&(pi, wi)) {
                    if let Some(t) = tabs.iter_mut().find(|t| t.id == tid) {
                        t.name = name;
                        // Pin the chosen name: stop following the live OSC title.
                        t.renamed = true;
                    }
                }
            }
        }
    }

    /// Edit the commit message buffer from a keystroke (commit box focused).
    fn edit_commit(&mut self, ks: &warpui::keymap::Keystroke) {
        match ks.key.as_str() {
            "enter" | "return" | "numpadenter" => self.commit_now(),
            "backspace" => {
                self.commit_msg.pop();
            }
            k if k.chars().count() == 1 => self.commit_msg.push_str(k),
            _ => {}
        }
    }

    /// Commit staged changes with the current message on a background thread
    /// (so the op pill animates like Push/Pull), then clear the message.
    fn commit_now(&mut self) {
        let msg = self.commit_msg.trim().to_string();
        let staged = self.changes.iter().filter(|c| c.staged).count();
        if msg.is_empty() || staged == 0 {
            return;
        }
        if self.git_op.lock().is_running() {
            return;
        }
        if let Some(root) = self.active_cwd.clone() {
            self.commit_error = None;
            let status = self.git_op.clone();
            let wake = self.git_wake.clone();
            crate::warpui::git::spawn_git_commit(root, msg, status, move || wake());
            self.commit_msg.clear();
            self.commit_focused = false;
        }
    }

    /// Dispatch an async network git op (Push / Pull / Fetch) on the active repo.
    fn spawn_op(&mut self, kind: crate::warpui::git::OpKind) {
        if let Some(root) = self.active_cwd.clone() {
            let status = self.git_op.clone();
            let wake = self.git_wake.clone();
            crate::warpui::git::spawn_git_op(kind, root, status, move || wake());
        }
    }

    /// Apply a keystroke to the pending new-file/new-folder editor. Enter
    /// commits, Escape cancels, Backspace deletes, printable chars append.
    fn edit_new_entry(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        match ks.key.as_str() {
            "enter" | "return" | "numpadenter" => self.commit_pending_entry(ctx),
            "escape" => self.pending_new_entry = None,
            "backspace" => {
                if let Some(p) = self.pending_new_entry.as_mut() {
                    p.name.pop();
                }
            }
            k if k.chars().count() == 1 => {
                if let Some(p) = self.pending_new_entry.as_mut() {
                    p.name.push_str(k);
                }
            }
            _ => {}
        }
    }

    /// Create the pending new file/folder on disk; on success refresh + clear,
    /// on failure keep the row open with an inline error (port of old
    /// `try_commit_pending`).
    fn commit_pending_entry(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(p) = self.pending_new_entry.as_ref() else { return };
        let name = p.name.trim().to_string();
        if name.is_empty() {
            self.pending_new_entry = None;
            return;
        }
        if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
            if let Some(p) = self.pending_new_entry.as_mut() {
                p.error = Some("Name can't contain / \\ . or ..".into());
            }
            return;
        }
        let target = p.parent.join(&name);
        let parent = p.parent.clone();
        let is_folder = p.is_folder;
        if target.exists() {
            if let Some(p) = self.pending_new_entry.as_mut() {
                p.error = Some(format!("`{name}` already exists"));
            }
            return;
        }
        let result = if is_folder {
            std::fs::create_dir(&target)
        } else {
            std::fs::File::create(&target).map(|_| ())
        };
        match result {
            Ok(()) => {
                self.expanded_dirs.insert(parent);
                self.pending_new_entry = None;
                self.refresh_panel(ctx);
            }
            Err(e) => {
                if let Some(p) = self.pending_new_entry.as_mut() {
                    p.error = Some(format!("Couldn't create: {e}"));
                }
            }
        }
    }

    /// The pane that should receive keyboard input: the focused pane IF it
    /// belongs to the active tab, else the active tab's first pane. Guarantees
    /// typing goes to the visible tab even if `focused` is stale.
    fn active_input_pane(&self) -> Option<PaneId> {
        let tab = self.active_tab?;
        let node = self.layouts.get(&tab)?;
        let mut leaves = Vec::new();
        node.leaves(&mut leaves);
        match self.focused {
            Some(f) if leaves.contains(&f) => Some(f),
            _ => leaves.first().copied(),
        }
    }

    /// Whether a real text-input widget currently holds keyboard focus — the
    /// commit box, or a focused Editor / Files pane (warp's text editors).
    /// Mirrors old egui's `!any_focus` guard (`ctx.memory(|m| m.focused())`):
    /// the terminal grid never registers as an egui-focused widget, so it is
    /// intentionally EXCLUDED here — otherwise panel-toggle shortcuts could
    /// never fire (the shell always has a focused pane).
    fn any_text_input_focused(&self, app: &AppContext) -> bool {
        // Only block panel toggles when we are GENUINELY capturing text: the
        // commit box, the inline new-entry editor, or an editor pane whose Find
        // bar is open (its keys route to the bar). A file/editor pane merely
        // being focused must NOT block Cmd+B / Cmd+/ — mirrors old egui's
        // real-keyboard-focus guard.
        if self.commit_focused || self.pending_new_entry.is_some() || self.renaming.is_some() {
            return true;
        }
        self.active_input_pane()
            .and_then(|id| match self.panes.get(&id) {
                Some(PaneContent::Editor(h)) => Some(h.read(app, |v, _| v.find_open())),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Add a new tab to worktree (pi, wi) and make it active.
    fn add_tab(&mut self, pi: usize, wi: usize, ctx: &mut ViewContext<Self>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let name = format!("Terminal {}", id + 1);
        self.worktree_tabs
            .entry((pi, wi))
            .or_default()
            .push(TabMeta { id, name, renamed: false, attention_since: None });
        let path = self
            .projects
            .get(pi)
            .and_then(|p| p.worktrees.get(wi))
            .map(|w| PathBuf::from(&w.path))
            .unwrap_or_else(|| PathBuf::from("/"));
        let pane = self.new_pane(path.clone(), ctx);
        let key = (pi, wi, id);
        self.layouts.insert(key, Node::Leaf(pane));
        self.active_tab = Some(key);
        self.selected = key;
        self.focused = Some(pane);
        self.active_cwd = Some(path);
        self.expanded_projects.insert(pi);
        self.expanded_worktrees.insert((pi, wi));
        self.refresh_panel(ctx);
    }

    /// Open (expand + activate) project `pi`: focus its first worktree's first
    /// tab, or create one if it has none. Used after Add Project so the picked
    /// folder becomes the active project with a live terminal.
    fn open_project(&mut self, pi: usize, ctx: &mut ViewContext<Self>) {
        self.expanded_projects.insert(pi);
        self.expanded_worktrees.insert((pi, 0));
        let first_tab = self
            .worktree_tabs
            .get(&(pi, 0))
            .and_then(|tabs| tabs.first())
            .map(|t| t.id);
        if let Some(tid) = first_tab {
            let key = (pi, 0, tid);
            let path = self
                .projects
                .get(pi)
                .and_then(|p| p.worktrees.get(0))
                .map(|w| PathBuf::from(&w.path))
                .unwrap_or_else(|| PathBuf::from("/"));
            self.selected = key;
            self.active_cwd = Some(path.clone());
            if !self.layouts.contains_key(&key) {
                let pane = self.new_pane(path, ctx);
                self.layouts.insert(key, Node::Leaf(pane));
                self.focused = Some(pane);
            } else if let Some(node) = self.layouts.get(&key) {
                self.focused = Some(node.first_leaf());
            }
            self.active_tab = Some(key);
            self.refresh_panel(ctx);
        } else {
            // No tabs yet → create one (also activates + spawns a terminal).
            self.add_tab(pi, 0, ctx);
        }
    }

    /// The terminal view handle for a pane, if it is a terminal.
    fn terminal_at(&self, id: PaneId) -> Option<ViewHandle<TerminalView>> {
        match self.panes.get(&id) {
            Some(PaneContent::Terminal(h)) => Some(h.clone()),
            _ => None,
        }
    }

    /// The file view handle for a pane, if it is a File pane.
    fn file_at(&self, id: PaneId) -> Option<ViewHandle<FileView>> {
        match self.panes.get(&id) {
            Some(PaneContent::File(h)) => Some(h.clone()),
            _ => None,
        }
    }

    /// Split the focused pane in `dir` with a new terminal in the same cwd.
    fn split_focused(&mut self, dir: Dir, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self.active_tab else { return };
        // Fall back to the tab's first pane if focus went stale — so Cmd+D/T
        // always splits SOMETHING rather than silently no-opping.
        let target = self
            .focused
            .filter(|id| self.panes.contains_key(id))
            .or_else(|| self.layouts.get(&tab).map(|n| n.first_leaf()));
        let Some(target) = target else { return };
        let path = self.active_cwd.clone().unwrap_or_else(|| PathBuf::from("/"));
        let new_id = self.new_pane(path, ctx);
        if let Some(node) = self.layouts.get_mut(&tab) {
            if node.split_leaf(target, new_id, dir) {
                self.focused = Some(new_id);
            } else {
                self.panes.remove(&new_id);
            }
        }
        // The sibling that just gave up half its space must reflow now, not on
        // the next stray event.
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// Force every leaf in the active tab to re-render at its CURRENT size.
    /// Terminal/file/editor/browser ChildViews cache their laid-out grid and
    /// only re-measure when their own view is notified — a plain shell repaint
    /// isn't enough. Called after any layout change (split add, pane close,
    /// splitter drag, window resize) so a shrunk/grown terminal reflows (and
    /// SIGWINCHes its PTY) immediately instead of on the next stray event.
    fn relayout_panes(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(tab) = self.active_tab {
            if let Some(node) = self.layouts.get(&tab) {
                let mut leaves = Vec::new();
                node.leaves(&mut leaves);
                for id in leaves {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    } else if let Some(h) = self.file_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    } else if let Some(h) = self.editor_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    } else if let Some(h) = self.browser_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    }
                }
            }
        }
        ctx.notify();
    }

    /// Close the focused pane (and its terminal). Collapses the split tree.
    fn close_focused(&mut self, ctx: &mut ViewContext<Self>) {
        let (Some(tab), Some(focused)) = (self.active_tab, self.focused) else {
            return;
        };
        if let Some(node) = self.layouts.remove(&tab) {
            match node.close_leaf(focused) {
                Some(remaining) => {
                    self.focused = Some(remaining.first_leaf());
                    self.layouts.insert(tab, remaining);
                }
                None => {
                    // Last pane of the layout closed. The TabMeta itself stays
                    // in `worktree_tabs` (its row falls back to the stored
                    // name), so drop the debounce entry now: if a new layout
                    // is later seeded under this tab id, its first title
                    // should adopt immediately rather than inherit a stale
                    // 3s hold from the old terminal.
                    self.title_debounce.borrow_mut().remove(&tab.2);
                    self.active_tab = None;
                    self.focused = None;
                }
            }
        }
        self.panes.remove(&focused);
        self.drag_states.remove(&focused);
        self.pane_rects.borrow_mut().remove(&focused);
        // Closing the File Edit pane via Cmd+W / its × button (as opposed to
        // the last-File-Tab-closed path, which already resets this) left
        // `files_pane` pointing at a removed pane id. `open_file` self-heals
        // that on the next open, but other readers (`is_file_pane`, the tab
        // strip's close-shortcut gate) compare against it directly — reset it
        // here so it never dangles.
        if self.files_pane == Some(focused) {
            self.files_pane = None;
        }
        // The sibling that just reclaimed the closed pane's space must reflow
        // now (SIGWINCH the terminal) rather than on the next stray event.
        ctx.dispatch_typed_action(&CraneShellAction::RelayoutPanes);
    }

    /// Fully tear down the layout at `key`: drop the tab's split tree and every
    /// pane it owns (dropping a Terminal pane's ViewHandle kills its PTY), plus
    /// each pane's drag state and cached rect. Same teardown the CloseTab path
    /// uses — call when a project/tab is removed so nothing keeps rendering or
    /// leaks a PTY.
    fn tear_down_layout(&mut self, key: (usize, usize, usize)) {
        self.title_debounce.borrow_mut().remove(&key.2);
        if let Some(node) = self.layouts.remove(&key) {
            let mut leaves = Vec::new();
            node.leaves(&mut leaves);
            for l in leaves {
                self.panes.remove(&l);
                self.drag_states.remove(&l);
                self.pane_rects.borrow_mut().remove(&l);
            }
        }
    }

    /// Seed a default tab/layout (project 0 / worktree 0) when nothing is open
    /// so split-based openers (Welcome / Diff / Browser / new-tab) work from the
    /// empty state instead of silently no-opping. Mirrors the startup seed.
    fn ensure_active_tab(&mut self, ctx: &mut ViewContext<Self>) {
        if self.active_tab.is_some() {
            return;
        }
        self.add_tab(0, 0, ctx);
    }

    /// The current window's (width, height) in points, or (0, 0) if unavailable.
    fn window_size(&self, app: &AppContext) -> (f32, f32) {
        app.window_ids()
            .into_iter()
            .next()
            .and_then(|id| app.window_bounds(&id))
            .map(|r| (r.size().x(), r.size().y()))
            .unwrap_or((0.0, 0.0))
    }

    /// Drag-rearrange: detach `dragged` from the active tab's tree and re-dock
    /// it beside `target` in `dir`. Pane views stay alive (history retained).
    fn dock_pane(&mut self, src: PaneId, target: PaneId, edge: DockEdge) {
        if src == target {
            return;
        }
        let Some((dir, before)) = edge_dir_before(edge) else {
            return; // Center is a swap, not a dock.
        };
        let Some(tab) = self.active_tab else { return };
        let Some(node) = self.layouts.remove(&tab) else {
            return;
        };
        match node.close_leaf(src) {
            Some(mut tree) => {
                // Re-insert `src` at the chosen edge of `target`.
                tree.split_leaf_ordered(target, src, dir, before);
                self.layouts.insert(tab, tree);
                self.focused = Some(src);
            }
            None => {
                // `src` was the whole tree — nothing to re-dock onto.
                self.layouts.insert(tab, Node::Leaf(src));
            }
        }
    }
}

impl Entity for CraneShellView {
    type Event = ();
}

impl View for CraneShellView {
    fn ui_name() -> &'static str {
        "CraneShellView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        // Center region = just the active tab's panes. Tabs are managed in the
        // Left Panel (1:1 with old Crane — no mid-pane horizontal tab strip).
        // Center = panes, with the Git Log docked at the BOTTOM (outside the
        // pane tree) when toggled — height-resizable via the vertical splitter.
        let center_region = if self.show_git_log {
            SplitBox::new(
                Dir::Vertical,
                self.center(app),
                self.git_log_dock(),
                self.git_log_ratio.clone(),
                self.git_log_drag.clone(),
                theme::divider(),
            )
            .finish()
        } else {
            self.center(app)
        };

        // Resizable left | center | right via nested draggable SplitBoxes.
        let body: Box<dyn Element> = match (self.show_left, self.show_right) {
            (true, true) => {
                let inner = SplitBox::new(
                    Dir::Horizontal,
                    center_region,
                    self.right_sidebar(),
                    self.right_ratio.clone(),
                    self.right_drag.clone(),
                    theme::divider(),
                )
                .finish();
                SplitBox::new(
                    Dir::Horizontal,
                    self.left_sidebar(app),
                    inner,
                    self.left_ratio.clone(),
                    self.left_drag.clone(),
                    theme::divider(),
                )
                .finish()
            }
            (true, false) => SplitBox::new(
                Dir::Horizontal,
                self.left_sidebar(app),
                center_region,
                self.left_ratio.clone(),
                self.left_drag.clone(),
                theme::divider(),
            )
            .finish(),
            (false, true) => SplitBox::new(
                Dir::Horizontal,
                center_region,
                self.right_sidebar(),
                self.right_ratio.clone(),
                self.right_drag.clone(),
                theme::divider(),
            )
            .finish(),
            (false, false) => center_region,
        };

        let column = Flex::column()
            .with_child(self.top_bar())
            .with_child(Expanded::new(1.0, body).finish())
            .finish();

        let mut root_stack = Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(column);

        // Overlay the context menu on top of everything when it is open.
        if let Some(pm) = &self.context_menu {
            root_stack = root_stack.with_child(self.project_context_menu(pm));
        }
        if let Some(rm) = &self.row_menu {
            root_stack = root_stack.with_child(self.row_menu_overlay(rm));
        }
        if let Some((pi, wi, x, y)) = self.worktree_menu {
            root_stack = root_stack.with_child(self.worktree_menu_overlay(pi, wi, x, y));
        }
        if let Some((key, x, y)) = self.tab_menu {
            root_stack = root_stack.with_child(self.tab_menu_overlay(key, x, y));
        }
        if let Some((group, x, y)) = &self.folder_menu {
            root_stack = root_stack.with_child(self.folder_menu_overlay(group, *x, *y));
        }
        // The ＋ New Pane dropdown, anchored under the top-bar button. The button
        // rect isn't exposed here, so anchor a fixed offset in from the right edge
        // (menu is 220px wide); `menu_popover`'s Popover clamps it on-screen.
        if self.new_pane_menu_open {
            let (win_w, _win_h) = self.window_size(app);
            let x = if win_w > 0.0 { (win_w - 220.0 - 8.0).max(8.0) } else { 8.0 };
            root_stack = root_stack.with_child(self.new_pane_menu_overlay(x, theme::TOPBAR_H));
        }
        // Sidebar drag: 2px accent drop-line at the gap nearest the cursor.
        if let Some(line) = self.tree_drop_line_overlay() {
            root_stack = root_stack.with_child(line);
        }
        if let Some((sha, x, y)) = &self.git_log_menu {
            if self.git_log_branch_prompt.is_none() {
                root_stack = root_stack.with_child(self.git_log_menu_overlay(sha, *x, *y));
            }
        }
        if let Some((sha, buf)) = &self.git_log_branch_prompt {
            root_stack = root_stack.with_child(self.git_log_branch_prompt_overlay(sha, buf));
        }
        if let Some((x, y)) = self.branch_picker {
            // Clamp the popover origin so a long list opened near the window edge
            // stays on-screen (menu is 220px wide; height is estimated).
            let (win_w, win_h) = self.window_size(app);
            let cx = if win_w > 0.0 {
                x.min((win_w - 220.0 - 8.0).max(8.0))
            } else {
                x
            };
            let cy = if win_h > 0.0 {
                y.min((win_h - self.branch_picker_height() - 8.0).max(8.0))
            } else {
                y
            };
            root_stack = root_stack.with_child(self.branch_picker_overlay(cx, cy));
        }
        if let Some(p) = &self.pending_delete {
            root_stack = root_stack.with_child(self.delete_confirm_overlay(p));
        }
        // Notification toasts: above the panes / menus, BELOW the blocking modal.
        // Only paint still-live toasts (the fast tick sweeps expired ones); the
        // overlay itself is click-through except on each card.
        if self.toasts.iter().any(|t| t.at.elapsed() < TOAST_TTL) {
            root_stack = root_stack.with_child(self.toast_overlay());
        }
        // Persistent update banner (old update_toast.rs) — bottom-right, above
        // the transient toasts, below the blocking modal.
        if self.update_banner_should_show() {
            let card = self.update_banner();
            // Empty, not Rect, for the same reason as toast_overlay's leading
            // spacers: a bare Rect always registers a hit-test region even
            // with no background, which would swallow every click across the
            // window (not just the banner's own corner) for as long as the
            // banner is showing — including clicks on its own × close button
            // if the banner's layer ended up beneath this spacer's.
            let row = Flex::row()
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(card)
                .with_child(Self::spacer(20.0))
                .finish();
            root_stack = root_stack.with_child(
                Flex::column()
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(row)
                    .with_child(Self::spacer(24.0))
                    .finish(),
            );
        }
        // Hover tooltip: above menus/toasts, below the blocking modal. Purely
        // decorative and click-through (see `tooltip_overlay`), so it never
        // steals events from whatever's underneath. Paint-gated on the Left
        // Panel being visible (every tooltip owner lives there) as a second
        // line of defence behind the explicit `hover_tip = None` clears in the
        // structural handlers (ToggleLeft / RemoveProject / RemoveGroup) —
        // widen the gate if a tooltip owner ever lands outside the Left Panel.
        if let Some((text, x, y)) = &self.hover_tip {
            if self.show_left {
                root_stack = root_stack.with_child(self.tooltip_overlay(text, *x, *y));
            }
        }
        // The blocking modal renders LAST — topmost, over every other overlay.
        if let Some(m) = &self.modal {
            root_stack = root_stack.with_child(self.modal_overlay(m, app));
        }

        let root = root_stack.finish();

        // Press-to-focus (old Crane's `pressed_inside`): on any left press, focus
        // the pane whose rect contains the cursor. Reliable across the terminal's
        // view boundary because the shell owns the pane rects (via RectProbe).
        let focus_rects = self.pane_rects.clone();
        // Live leaves of the ACTIVE tab — restrict click hit-testing to these so
        // a CLOSED pane's stale rect (pane_rects is never pruned) can't capture a
        // click in the area its surviving sibling expanded into.
        let live_leaves: Vec<PaneId> = self
            .active_tab
            .and_then(|t| self.layouts.get(&t))
            .map(|n| {
                let mut v = Vec::new();
                n.leaves(&mut v);
                v
            })
            .unwrap_or_default();
        // Whether a blocking modal is open — captured into the keydown closure so
        // Escape closes it and other keys are swallowed.
        let modal_open = self.modal.is_some();
        // Which typing-capable modal is open (if any): its keys route to the
        // query field / switcher instead of being swallowed.
        let modal_is_fif = matches!(self.modal, Some(Modal::FindInFiles));
        let modal_is_switcher = matches!(self.modal, Some(Modal::TabSwitcher));
        let modal_is_switch_branch = matches!(self.modal, Some(Modal::SwitchBranch));
        let modal_is_new_workspace = matches!(self.modal, Some(Modal::NewWorkspace));
        // Any dropdown / popover menu open (mirrors exactly what CloseContextMenu
        // clears): a non-modal Escape should dismiss these before falling through
        // to the terminal. Captured now like `modal_open`; the view rebuilds every
        // frame so this reflects live state when a key arrives.
        let any_menu_open = self.context_menu.is_some()
            || self.row_menu.is_some()
            || self.branch_picker.is_some()
            || self.worktree_menu.is_some()
            || self.tab_menu.is_some()
            || self.folder_menu.is_some()
            || self.git_log_menu.is_some()
            || self.git_log_branch_prompt.is_some()
            || self.new_pane_menu_open;
        // App-level keyboard shortcuts. The terminal pane propagates Cmd combos
        // up to here (its own on_keydown returns PropagateToParent for cmd).
        EventHandler::new(root)
            .on_left_mouse_down(move |ctx, _app, pos| {
                let snapshot: Vec<(PaneId, RectF)> = focus_rects
                    .borrow()
                    .iter()
                    .filter(|(k, _)| live_leaves.contains(k))
                    .map(|(k, v)| (*k, v.get()))
                    .collect();
                // Pick the SMALLEST containing rect (the leaf), not the first in
                // nondeterministic HashMap order — avoids stale/overlapping rects.
                let hit = snapshot
                    .iter()
                    .filter(|(_, r)| r.contains_point(pos))
                    .min_by(|(_, a), (_, b)| {
                        (a.width() * a.height())
                            .partial_cmp(&(b.width() * b.height()))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(pid, _)| *pid);
                if let Some(pid) = hit {
                    ctx.dispatch_typed_action(CraneShellAction::FocusPane(pid));
                }
                DispatchEventResult::PropagateToParent
            })
            .on_keydown(move |ctx, _app, ks| {
                // A modal is blocking: Escape closes it (routed FIRST); every
                // other key is swallowed so nothing leaks to the panes behind
                // the dim backdrop.
                if modal_open {
                    if ks.key.to_ascii_lowercase() == "escape"
                        && !ks.cmd
                        && !ks.ctrl
                        && !ks.alt
                    {
                        ctx.dispatch_typed_action(CraneShellAction::CloseModal);
                        return DispatchEventResult::StopPropagation;
                    }
                    // Cmd+` while the switcher is open advances the highlight
                    // (next / prev on Shift) instead of typing into it.
                    if modal_is_switcher
                        && ks.cmd
                        && !ks.ctrl
                        && !ks.alt
                        && (ks.key == "`" || ks.key == "~")
                    {
                        ctx.dispatch_typed_action(CraneShellAction::AdvanceTabSwitcher(
                            ks.shift || ks.key == "~",
                        ));
                        return DispatchEventResult::StopPropagation;
                    }
                    // Global font-zoom chords (Cmd+= / Cmd+- / Cmd+0) stay live
                    // even while a modal is open — Settings > Appearance advertises
                    // them, and the live % readout only refreshes if they actually
                    // dispatch. Carve them out before the blanket swallow below.
                    if ks.cmd && !ks.ctrl && !ks.alt {
                        let zoom = match ks.key.to_ascii_lowercase().as_str() {
                            "=" | "+" => Some(CraneShellAction::FontZoomIn),
                            "-" => Some(CraneShellAction::FontZoomOut),
                            "0" => Some(CraneShellAction::FontZoomReset),
                            _ => None,
                        };
                        if let Some(act) = zoom {
                            ctx.dispatch_typed_action(act);
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                    // Route typing / nav into the Find-in-Files query field or
                    // the tab switcher; every other modal swallows all keys.
                    if modal_is_fif {
                        ctx.dispatch_typed_action(CraneShellAction::FindInFilesKey(ks.clone()));
                    } else if modal_is_switcher {
                        ctx.dispatch_typed_action(CraneShellAction::TabSwitcherKey(ks.clone()));
                    } else if modal_is_switch_branch {
                        ctx.dispatch_typed_action(CraneShellAction::SwitchBranchKey(ks.clone()));
                    } else if modal_is_new_workspace {
                        ctx.dispatch_typed_action(CraneShellAction::NewWorkspaceKey(ks.clone()));
                    }
                    return DispatchEventResult::StopPropagation;
                }
                // No modal, but a dropdown / popover menu is open: Escape closes it
                // (same clearing as clicking away) and is consumed so it does not
                // also reach the terminal. Falls through to the normal Escape path
                // (restore maximized pane / SendKeys) when no menu is open.
                if any_menu_open
                    && ks.key.to_ascii_lowercase() == "escape"
                    && !ks.cmd
                    && !ks.ctrl
                    && !ks.alt
                {
                    ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                    return DispatchEventResult::StopPropagation;
                }
                if ks.cmd && !ks.ctrl && !ks.alt {
                    // Shift uppercases the key ("D"), so normalize the case.
                    let key = ks.key.to_ascii_lowercase();
                    let act = match key.as_str() {
                        // Cmd+Shift+B opens the Switch Branch modal; Cmd+B toggles
                        // the Left Panel (the shift arm MUST precede the plain one).
                        "b" if ks.shift => Some(CraneShellAction::OpenSwitchBranch),
                        "b" => Some(CraneShellAction::ToggleLeft),
                        // Cmd+/ toggles the line comment when an editor pane is
                        // focused, else toggles the Right Panel (its old behavior).
                        "/" => Some(CraneShellAction::CommentOrToggleRight),
                        // Cmd+D split side-by-side, Cmd+Shift+D stacked.
                        "d" if ks.shift => Some(CraneShellAction::SplitFocused(Dir::Vertical)),
                        "d" => Some(CraneShellAction::SplitFocused(Dir::Horizontal)),
                        // Canonical: Cmd+T splits a pane, Cmd+Shift+T adds a tab.
                        "t" if ks.shift => Some(CraneShellAction::NewTab),
                        "t" => Some(CraneShellAction::SplitFocused(Dir::Horizontal)),
                        // Cmd+Shift+W closes the whole active tab; Cmd+W closes the focused pane.
                        "w" if ks.shift => Some(CraneShellAction::CloseActiveTab),
                        "w" => Some(CraneShellAction::CloseFocused),
                        "v" => Some(CraneShellAction::PasteFocused),
                        "k" => Some(CraneShellAction::ClearFocused),
                        "s" => Some(CraneShellAction::SaveFocusedFile),
                        "a" => Some(CraneShellAction::SelectAllFocused),
                        "z" if ks.shift => Some(CraneShellAction::RedoFocused),
                        "z" => Some(CraneShellAction::UndoFocused),
                        "c" => Some(CraneShellAction::CopyFocused),
                        "x" => Some(CraneShellAction::CutFocused),
                        // Editor find / replace / goto-line (open the bar; keys then
                        // route through the editor's own input_key). Cmd+Shift+F
                        // opens the project-wide Find-in-Files modal instead — it
                        // MUST precede the plain "f" arm.
                        "f" if ks.shift => Some(CraneShellAction::OpenFindInFiles),
                        "f" => Some(CraneShellAction::FindFocused),
                        "h" => Some(CraneShellAction::ReplaceFocused),
                        "g" => Some(CraneShellAction::GotoLineFocused),
                        // Cmd+Shift+O adds a project (folder picker); Cmd+O opens
                        // an external file (file picker). Matches old shortcuts.rs.
                        "o" if ks.shift => Some(CraneShellAction::AddProject),
                        "o" => Some(CraneShellAction::OpenExternalFile),
                        // Cmd+Shift+U opens a Browser pane. Cmd+Shift+B is already
                        // Switch Branch, so U is the free chord (mirrors the T/D
                        // shift-checked-first pattern; "u" has no plain-Cmd arm).
                        "u" if ks.shift => Some(CraneShellAction::OpenBrowser),
                        // Cmd+[ / Cmd+] cycle focus across panes in the active tab.
                        "[" => Some(CraneShellAction::FocusPrevPane),
                        "]" => Some(CraneShellAction::FocusNextPane),
                        // Cmd+` opens / advances the tab switcher (Cmd+Shift+` =
                        // "~" on macOS → previous). Committed via Enter / click.
                        "`" => Some(CraneShellAction::AdvanceTabSwitcher(false)),
                        "~" => Some(CraneShellAction::AdvanceTabSwitcher(true)),
                        // Cmd+9 toggles the Git log panel (bare Cmd+9 only —
                        // Cmd+Shift+9 must NOT open it, matching old shortcuts.rs).
                        "9" if !ks.shift => Some(CraneShellAction::OpenGitLog),
                        // Cmd+Shift+N opens the Welcome / landing pane beside the
                        // focused pane (default new-tab stays a terminal).
                        "n" if ks.shift => Some(CraneShellAction::OpenWelcome),
                        // Font zoom (Cmd+= / Cmd+- / Cmd+0) — +1 (max 40) / -1
                        // (min 8) / reset 14, matching old shortcuts.rs.
                        "=" | "+" => Some(CraneShellAction::FontZoomIn),
                        "-" => Some(CraneShellAction::FontZoomOut),
                        "0" => Some(CraneShellAction::FontZoomReset),
                        _ => None,
                    };
                    if let Some(act) = act {
                        ctx.dispatch_typed_action(act);
                        return DispatchEventResult::StopPropagation;
                    }
                    return DispatchEventResult::PropagateToParent;
                }
                // Cmd+Opt+W toggles soft word-wrap in the focused editor pane.
                // NOTE(choice): Cmd+Opt+W was picked as a free chord (Cmd+W closes
                // the pane, Cmd+Shift+W the tab); the old egui build had a wrap
                // pref rather than a shortcut.
                if ks.cmd && ks.alt && !ks.ctrl && ks.key.to_ascii_lowercase() == "w" {
                    ctx.dispatch_typed_action(CraneShellAction::ToggleWordWrap);
                    return DispatchEventResult::StopPropagation;
                }
                // Cmd+Opt+T — new tab in the focused Browser pane, or open a
                // Browser pane when none is focused (old shortcuts.rs chord).
                if ks.cmd && ks.alt && !ks.ctrl && ks.key.to_ascii_lowercase() == "t" {
                    ctx.dispatch_typed_action(CraneShellAction::BrowserNewTab);
                    return DispatchEventResult::StopPropagation;
                }
                // F12: LSP goto-definition at the caret in the focused editor.
                if !ks.cmd && !ks.ctrl && !ks.alt && ks.key.to_ascii_lowercase() == "f12" {
                    ctx.dispatch_typed_action(CraneShellAction::LspGotoAtCursor);
                    return DispatchEventResult::StopPropagation;
                }
                // Regular keys: route to the FOCUSED pane's terminal. Shell-driven
                // input avoids warpui per-view focus being out of sync.
                // TODO(parity): IME / composed multi-codepoint text (CJK, emoji,
                // dead keys) should arrive via a Text/insert-text route and be
                // written verbatim to the PTY — deferred until warpui surfaces a
                // composed-text event (only the single Keystroke.key is sent now).
                ctx.dispatch_typed_action(CraneShellAction::SendKeys(ks.clone()));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum CraneShellAction {
    Select {
        sel: (usize, usize, usize),
        path: PathBuf,
    },
    ToggleLeft,
    ToggleRight,
    /// Cmd+/: toggle the line comment in the focused editor pane, or fall back to
    /// toggling the Right Panel when no editor pane is focused.
    CommentOrToggleRight,
    /// Cmd+Opt+W: toggle soft word-wrap in the focused editor pane.
    ToggleWordWrap,
    SetTab { files: bool },
    ToggleDir(PathBuf),
    SelectFile(PathBuf),
    ToggleProject(usize),
    ToggleWorktree(usize, usize),
    /// Toggle collapse/expand of a folder group, keyed by its shared parent
    /// directory path (`ProjectNode::group_path`).
    ToggleGroup(String),
    /// Open the folder-group header context menu at (x, y), keyed by group path.
    ShowFolderMenu { group: String, x: f32, y: f32 },
    /// Set (Some) or reset (None) the folder-group tint keyed by group path.
    SetGroupTint { group: String, tint: Option<[u8; 3]> },
    /// Remove EVERY member project of the folder group atomically (the group is
    /// removed whole via the header, never member-by-member).
    RemoveGroup(String),
    /// Split the focused pane (Horizontal = side by side, Vertical = stacked).
    SplitFocused(Dir),
    /// Close the focused pane.
    CloseFocused,
    /// Focus a specific pane (click its header).
    FocusPane(PaneId),
    /// Close a specific pane (header ✕).
    ClosePane(PaneId),
    /// Toggle expand-to-full for a pane (header maximize button).
    ToggleMaximize(PaneId),
    /// Split a specific pane (header split buttons).
    SplitPane(PaneId, Dir),
    /// Drag-rearrange: dock `src` at `edge` of `target` (split).
    DockPane {
        src: PaneId,
        target: PaneId,
        edge: DockEdge,
    },
    /// Drag onto the center zone: swap the two panes' positions.
    SwapPanes { a: PaneId, b: PaneId },
    /// Route a keystroke to the FOCUSED pane's terminal (shell-driven input).
    SendKeys(warpui::keymap::Keystroke),
    /// Cmd+V into the focused pane.
    PasteFocused,
    /// Cmd+K clear the focused pane.
    ClearFocused,
    /// Cmd+S save the focused File pane.
    SaveFocusedFile,
    /// Open the editor's Find bar / Replace bar / Goto-line input.
    FindFocused,
    ReplaceFocused,
    GotoLineFocused,
    /// Cmd+Z undo / Cmd+Shift+Z redo in the focused File pane.
    UndoFocused,
    RedoFocused,
    /// Cmd+C copy / Cmd+X cut (whole line) in the focused File pane.
    CopyFocused,
    CutFocused,
    /// File pane tab strip: switch to / close file tab `i`.
    FileTabSelect(usize),
    FileTabClose(usize),
    /// Affirmative from the ConfirmCloseFileTab modal (or a direct close when
    /// the buffer is clean) — actually removes the File Tab.
    FileTabCloseConfirmed(usize),
    /// Cmd+A select-all in the focused editor.
    SelectAllFocused,
    /// Bulk-stage the given paths (folder-row / context-menu Stage).
    StagePaths(Vec<String>),
    /// Bulk-unstage the given paths (folder-row / context-menu Unstage).
    UnstagePaths(Vec<String>),
    /// Toggle collapse of a directory group in the Changes tree.
    ToggleChangeDir(String),
    /// Async network git ops on the active repo.
    GitPush,
    GitPull,
    GitFetch,
    /// Open the Changes-row right-click menu.
    ShowChangeMenu { path: String, staged: bool, has_unstaged: bool, x: f32, y: f32 },
    /// Open the Files-row right-click menu.
    ShowFileMenu { path: PathBuf, is_dir: bool, x: f32, y: f32 },
    /// Open an absolute path in the editor/Files pane (context-menu Open).
    OpenFileAt(PathBuf),
    /// Open a file in the editor/Files pane at an optional `:LINE[:COL]`,
    /// dispatched when a clickable path in a Terminal pane is clicked. `path` is
    /// already resolved (absolute) against the terminal's cwd. `line`/`col` are
    /// 1-based when present. (`col` is recorded for future column-precise jumps;
    /// the editor currently only supports goto-line.)
    OpenFileAtPath {
        path: PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
    /// A desktop notification (OSC 9 / OSC 777) drained from a Terminal pane and
    /// forwarded by its `TerminalView`. `urgent` is true for OSC 777. The shell
    /// owns rendering the toast — the terminal view only forwards the payload.
    TermNotification {
        body: String,
        urgent: bool,
        /// Source tab (project_idx, worktree_idx, tab_id) the emitting terminal
        /// lives in, as synced onto its `TerminalView::owner_key`. Drives the
        /// Left-Panel attention pulse; `None` before the shell has synced owners.
        source: Option<(usize, usize, usize)>,
    },
    /// A background terminal rang BEL — flag attention on `source` (if it isn't
    /// the active tab). No toast; pulse only. `source` as in `TermNotification`.
    TermBell {
        source: Option<(usize, usize, usize)>,
    },
    /// Dismiss the notification toast with this id (the toast's X button).
    DismissToast(u64),
    /// Clicking a toast body: activate its originating tab, then dismiss it.
    FocusToastSource(u64),
    /// Copy an arbitrary path string to the clipboard.
    CopyPathStr(String),
    /// Reveal an absolute path in the system file manager (`open -R`).
    RevealPathInFinder(PathBuf),
    /// Start the inline new-file / new-folder editor under `parent`.
    NewEntry { parent: PathBuf, is_folder: bool },
    /// Request delete of a path — opens the confirm overlay.
    RequestDelete(PathBuf),
    /// Confirm / cancel the pending delete.
    ConfirmDelete,
    CancelDelete,
    /// Open the branch picker overlay at (x, y).
    ShowBranchPicker { x: f32, y: f32 },
    /// Check out a branch, then refresh.
    CheckoutBranch(String),
    /// Give the commit message box keyboard focus.
    FocusCommit,
    /// Commit staged changes with the current message.
    CommitStaged,
    /// Cmd+[ focus the previous leaf pane (in-order traversal, wrapping).
    FocusPrevPane,
    /// Cmd+] focus the next leaf pane (in-order traversal, wrapping).
    FocusNextPane,
    /// Cmd+Shift+W close the active tab (all panes in it).
    CloseActiveTab,
    /// Open a Git log pane.
    OpenGitLog,
    /// Select a commit in the Git Log list (dispatched by the lane element on a
    /// row click) — highlights it and loads its `git show` detail off-thread.
    GitLogSelect(String),
    /// Right-click on a commit row — open the commit context menu at (x, y).
    GitLogShowMenu { sha: String, x: f32, y: f32 },
    /// Detach-checkout the commit (`git checkout <sha>`).
    GitLogCheckout(String),
    /// Cherry-pick the commit onto the current branch.
    GitLogCherryPick(String),
    /// Revert the commit (`git revert --no-edit`).
    GitLogRevert(String),
    /// Open the inline "create branch from commit" prompt.
    GitLogBranchPrompt(String),
    /// Select a file in the commit detail's changed-files list.
    GitLogDetailFile(usize),
    /// Scope the log to one ref (`git log <ref>`), or None for `--all`.
    GitLogSetRefFilter(Option<String>),
    /// Click into the git-log text-filter field — take typing focus.
    GitLogFocusFilter,
    /// Arrow-key selection step through the (possibly filtered) commit list;
    /// `true` = down (older).
    GitLogStepSelection(bool),
    /// `git fetch --all --prune --tags` off-thread, then reload the graph.
    GitLogFetchAll,
    /// Scroll the Git Log detail/diff panel by N rows (positive = down).
    GitLogDetailScroll(i32),
    /// Open a Browser pane (placeholder).
    OpenBrowser,
    /// Cmd+Opt+T — new tab in the focused Browser pane; opens a Browser pane
    /// when none is focused.
    BrowserNewTab,
    /// Re-layout the active tab's pane children (terminal grids get their
    /// SIGWINCH) after a geometry change that happens WITHOUT an action —
    /// splitter drags (ratio Cells mutate silently) and window resizes.
    /// Deliberately skips the handle_action tail (no save_state per drag tick).
    RelayoutPanes,
    /// A sidebar row drag crossed the movement threshold.
    TreeDragStart(TreeDrag),
    /// A sidebar row drag released at window position (x, y).
    TreeDrop { x: f32, y: f32 },
    /// A Files-tree row drag crossed the movement threshold.
    FsDragStart(PathBuf),
    /// A Files-tree row drag released at window position (x, y).
    FsDrop { x: f32, y: f32 },
    /// OS files dropped (Finder → Crane) onto the Files tree at (x, y).
    FsExternalDrop { paths: Vec<String>, x: f32, y: f32 },
    /// Open a read-only unified Diff pane (HEAD vs working copy) for the file at
    /// the given absolute path (dispatched by a Changes-row click).
    OpenDiff(PathBuf),
    /// Open the Welcome / landing pane beside the focused pane.
    OpenWelcome,
    /// App-wide font zoom (Cmd+= / Cmd+- / Cmd+0).
    FontZoomIn,
    FontZoomOut,
    FontZoomReset,
    /// Add a new tab to the active workspace.
    NewTab,
    /// Add a new tab to a specific worktree (left-panel + button).
    NewTabIn(usize, usize),
    /// Close a tab (project, worktree, tab_id) from the strip. Guarded: if the
    /// tab holds a running terminal or an unsaved editor it opens the
    /// `ConfirmCloseTab` modal instead of tearing down immediately.
    CloseTab((usize, usize, usize)),
    /// Actually tear down a tab's layout + PTYs — the post-confirm companion to
    /// `CloseTab` (and the bypass used by bulk / automatic teardown paths that
    /// have their own confirmation or none is warranted).
    CloseTabConfirmed((usize, usize, usize)),
    /// Switch to a named theme (cycles through all installed themes).
    SetTheme(String),
    /// Open a native folder picker and add the chosen directory as a new project.
    AddProject,
    /// Show a hover tooltip label anchored near the cursor at (x, y).
    ShowTooltip { text: String, x: f32, y: f32 },
    /// Hide the hover tooltip (mouse left the anchoring element).
    HideTooltip,
    /// Open a native file picker and open the chosen file into the Files pane.
    OpenExternalFile,
    /// Remove the project at index `i` from the project list and persist.
    RemoveProject(usize),
    /// Show the project context menu anchored at the given window position.
    ShowProjectMenu { project_idx: usize, x: f32, y: f32 },
    /// Show the worktree/branch-row context menu anchored at (x, y).
    ShowWorktreeMenu { pi: usize, wi: usize, x: f32, y: f32 },
    /// Show the Tab-row context menu anchored at (x, y).
    ShowTabMenu { key: (usize, usize, usize), x: f32, y: f32 },
    /// Close every tab in `key`'s worktree except `key` itself.
    CloseOtherTabs((usize, usize, usize)),
    /// A left-click on a worktree/branch row: double-click starts an inline
    /// rename, a single click toggles expand.
    WorktreeRowClick { pi: usize, wi: usize },
    /// A left-click on a Tab row: double-click starts an inline rename, a single
    /// click selects the tab (routes to Select).
    TabRowClick {
        key: (usize, usize, usize),
        path: PathBuf,
    },
    /// Start an inline rename of the worktree/branch row (from its menu).
    StartRenameWorktree { pi: usize, wi: usize },
    /// Start an inline rename of the Tab row (from its menu).
    StartRenameTab { key: (usize, usize, usize) },
    /// Open the `ConfirmRemoveWorktree` modal (computes the dirty/unpushed
    /// warning first). The destructive `git worktree remove` only runs on
    /// explicit confirm via `RemoveWorktreeConfirmed`.
    RemoveWorktree { pi: usize, wi: usize },
    /// `git worktree remove --force` the worktree, then tear down its panes —
    /// the post-confirm companion to `RemoveWorktree`. Also the entry point for
    /// automatic cleanup of a worktree whose checkout dir vanished on disk.
    RemoveWorktreeConfirmed { pi: usize, wi: usize },
    /// Set / clear a per-worktree tint (keyed by the worktree path).
    SetWorktreeTint {
        pi: usize,
        wi: usize,
        tint: Option<[u8; 3]>,
    },
    /// Set / clear a per-tab tint (keyed by (worktree_path, tab_id)).
    SetTabTint {
        key: (usize, usize, usize),
        tint: Option<[u8; 3]>,
    },
    /// Dismiss the active project context menu.
    CloseContextMenu,
    /// Toggle the top-bar ＋ New Pane dropdown open/closed.
    ToggleNewPaneMenu,
    /// Reveal the project folder in the system file manager.
    RevealProjectInFinder(usize),
    /// Copy the project path to the clipboard.
    CopyProjectPath(usize),
    /// Set or clear a per-project tint. `None` resets to the palette default.
    SetProjectTint(usize, Option<[u8; 3]>),
    /// Run `git init` in the project folder and reload the project list so it
    /// flips from loose (FOLDER icon) to a real git project (CUBE icon + branches).
    InitGitProject(usize),
    /// Dismiss the active blocking modal.
    CloseModal,
    /// Open the keyboard shortcuts (Help) modal.
    OpenHelp,
    /// Open the Settings modal (Appearance + About).
    OpenSettings,
    /// User confirmed Quit in the ConfirmQuit modal — actually terminate the app.
    QuitConfirmed,
    /// User confirmed closing a terminal pane that had a running process.
    ConfirmClosePane(PaneId),
    /// Cmd+Shift+F: open the project-wide Find-in-Files modal.
    OpenFindInFiles,
    /// A keystroke routed to the open Find-in-Files query field.
    FindInFilesKey(warpui::keymap::Keystroke),
    /// Open a Find-in-Files match: open its file at the given 1-based line.
    OpenFifMatch { path: PathBuf, line: u32 },
    /// Cmd+` / Cmd+Shift+`: open or advance the tab switcher (`true` = backward).
    AdvanceTabSwitcher(bool),
    /// A keystroke routed to the open tab switcher.
    TabSwitcherKey(warpui::keymap::Keystroke),
    /// Activate the given tab from the switcher (click / Enter) and close it.
    ActivateSwitcherTab {
        key: (usize, usize, usize),
        path: PathBuf,
    },
    /// LSP goto-definition at a 0-based `(line, character)` in `path` — raised by
    /// the editor's Cmd+LeftClick callback.
    LspGoto {
        path: PathBuf,
        line: u32,
        character: u32,
    },
    /// F12: LSP goto-definition at the caret in the focused editor pane.
    LspGotoAtCursor,
    /// Settings toggle: flip the editor Language Server opt-in. ON opens every
    /// live editor file so servers spawn + diagnostics start; OFF tears down all
    /// running servers and clears the squiggles from every open editor.
    ToggleLsp,
    /// Switch the active Settings sidebar section.
    SettingsGoto(SettingsSection),
    /// Step a base font size by ±1pt; `editor` picks editor vs terminal.
    FontBaseStep { editor: bool, delta: f32 },
    /// Toggle the editor word-wrap default (applies to every open editor).
    ToggleWordWrapDefault,
    /// Toggle trim-trailing-whitespace-on-save (applies to every open editor).
    ToggleTrimOnSave,
    /// Set (or clear, None = auto) the syntect theme override.
    SetSyntaxOverride(Option<String>),
    /// Create + reveal `~/.crane/themes` in Finder.
    OpenThemesFolder,
    /// Open a URL in the system browser (About links).
    OpenUrl(String),
    /// Manual "Check for updates" (About) — re-runs the release check.
    UpdateCheckNow,
    /// Settings toggle: flip the editor format-on-save opt-in, then persist.
    ToggleFormatOnSave,
    /// Settings > About: begin the background download + stage of the latest
    /// release DMG (idempotent — a no-op if one is already in flight or staged).
    StartUpdateDownload,
    /// Update banner: "Remind in 7 days" — persists a RemindAt for the version.
    UpdateRemindLater,
    /// Update banner: "Skip this version" — never prompt for it again.
    UpdateSkipVersion,
    /// Update banner: × / "Later" — hide it for the rest of this session.
    UpdateDismissSession,
    /// Settings > About: swap the running install for the staged `Crane.app` and
    /// relaunch. Carries the staged bundle path from `UpdateState::Ready`.
    ApplyUpdate(PathBuf),
    /// Open the "Switch Branch" modal (searchable local + remote branch list).
    /// Trigger: click the status-bar branch label, or Cmd+Shift+B.
    OpenSwitchBranch,
    /// A keystroke routed to the open Switch-Branch search field.
    SwitchBranchKey(warpui::keymap::Keystroke),
    /// Create a NEW branch (checkout -b) named after the current search query in
    /// the Switch-Branch modal, then refresh. Raised by the "Create new branch…"
    /// row when the typed query names a branch that doesn't exist yet.
    CreateBranchCheckout(String),
    /// Open the "New Workspace" modal for project `pi`, optionally pre-filling the
    /// branch field (e.g. from a Switch-Branch row's "+ worktree" affordance).
    OpenNewWorkspace { pi: usize, branch: Option<String> },
    /// A keystroke routed to the open New-Workspace branch field.
    NewWorkspaceKey(warpui::keymap::Keystroke),
    /// Toggle the New-Workspace "create new branch" checkbox.
    NewWorkspaceToggleNewBranch,
    /// Pick the New-Workspace location mode (Global / Project-local / Custom).
    NewWorkspaceSetMode(LocationMode),
    /// Move typing focus between the branch field and the custom-path field.
    NewWorkspaceFocusPath(bool),
    /// Open the OS folder picker for the Custom location's parent.
    NewWorkspaceBrowse,
    /// Confirm the New-Workspace modal: `git worktree add` + insert + open.
    NewWorkspaceConfirm,
    Noop,
}

/// Run a periodic-tick body under `catch_unwind`, logging (via the panic hook
/// in main.rs, which writes to `~/.crane/crash.log`) and skipping that tick on
/// panic instead of letting it unwind into the libdispatch/CFRunLoop callback
/// that's driving warpui's timer stream — crossing that FFI boundary aborts
/// the whole process (see `handle_action`'s doc comment for the confirmed
/// production crash this mirrors). Every `ctx.spawn_stream_local` tick body in
/// `CraneShellView::new` should route through this.
fn guarded_tick(
    this: &mut CraneShellView,
    ctx: &mut ViewContext<CraneShellView>,
    label: &'static str,
    f: impl FnOnce(&mut CraneShellView, &mut ViewContext<CraneShellView>),
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(this, ctx)));
    if result.is_err() {
        log::error!("crane: recovered from a panic in the {label} tick — see ~/.crane/crash.log");
    }
}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    /// Every user action (click, keystroke, menu item, tick) dispatches
    /// through here. Wrapped in `catch_unwind`: a panic anywhere in the match
    /// below used to unwind straight through warpui's executor into the
    /// libdispatch/AppKit callback that invoked it — crossing that FFI
    /// boundary aborts the WHOLE process (confirmed from a real production
    /// crash: `_dispatch_client_callout` sits directly above the crashed
    /// frames). Catching it here means one bad action logs to
    /// `~/.crane/crash.log` (via the panic hook in main.rs) and gets skipped
    /// — the rest of the app keeps running instead of taking the user's
    /// whole session down.
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        let action = action.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.handle_action_impl(&action, ctx);
        }));
        if result.is_err() {
            log::error!("crane: recovered from a panic in handle_action({action:?}) — see ~/.crane/crash.log");
            // The action may have partially mutated state before panicking;
            // repaint so the UI reflects whatever did land instead of
            // silently freezing on stale content.
            ctx.notify();
        }
    }
}

impl CraneShellView {
    fn handle_action_impl(&mut self, action: &CraneShellAction, ctx: &mut ViewContext<Self>) {
        match action {
            CraneShellAction::Select { sel, path } => {
                self.selected = *sel;
                self.active_cwd = Some(path.clone());
                // Loose (non-git) projects have no Changes tab — force Files so
                // the user never lands on a permanently empty Changes pane.
                if self.is_loose_active() {
                    self.files_tab = true;
                }
                // Open this tab's layout once (single terminal leaf); thereafter
                // just show it (its PTY + scrollback ran the whole time).
                if !self.layouts.contains_key(sel) {
                    let id = self.new_pane(path.clone(), ctx);
                    self.layouts.insert(*sel, Node::Leaf(id));
                    self.focused = Some(id);
                } else if let Some(node) = self.layouts.get(sel) {
                    // Re-focus this tab's first pane.
                    self.focused = Some(node.first_leaf());
                }
                self.active_tab = Some(*sel);
                self.refresh_panel(ctx);
            }
            CraneShellAction::SplitFocused(dir) => self.split_focused(*dir, ctx),
            // TODO(parity): Cmd+W should close the active File Tab first when a
            // Files/Editor pane has >1 tabs, and stage a running-process confirm
            // modal for terminals with a live foreground process. Both need the
            // (unported) confirm-modal framework; for now it tears the pane down.
            CraneShellAction::CloseFocused => {
                // Files pane with >1 open file tabs: close only the ACTIVE file
                // tab (route to FileTabClose), which tears the pane down only when
                // the last tab closes. Otherwise close the whole pane.
                let close_file_tab = self.files_pane.is_some()
                    && self.focused == self.files_pane
                    && self.file_pane_paths.len() > 1;
                if close_file_tab {
                    let a = CraneShellAction::FileTabClose(self.file_pane_active);
                    self.handle_action(&a, ctx);
                } else {
                    // Guard: if the focused pane is a terminal running a foreground
                    // program, confirm before tearing down its PTY. Idle panes
                    // close immediately (as before).
                    let running = self.focused.and_then(|id| self.terminal_at(id)).map_or(
                        false,
                        |h| h.read(&*ctx, |v, _| v.has_foreground_process()),
                    );
                    if running {
                        self.modal = Some(Modal::ConfirmClosePane(self.focused.unwrap()));
                    } else {
                        self.close_focused(ctx);
                    }
                }
            }
            CraneShellAction::FocusPane(id) => {
                self.focused = Some(*id);
                self.commit_focused = false;
            }
            CraneShellAction::ClosePane(id) => {
                self.focused = Some(*id);
                if self.maximized == Some(*id) {
                    self.maximized = None;
                }
                self.close_focused(ctx);
            }
            CraneShellAction::ToggleMaximize(id) => {
                self.maximized = if self.maximized == Some(*id) {
                    None
                } else {
                    Some(*id)
                };
            }
            CraneShellAction::SplitPane(id, dir) => {
                self.focused = Some(*id);
                self.split_focused(*dir, ctx);
            }
            CraneShellAction::DockPane { src, target, edge } => {
                self.dock_pane(*src, *target, *edge);
            }
            CraneShellAction::SwapPanes { a, b } => {
                if let Some(tab) = self.active_tab {
                    if let Some(node) = self.layouts.get_mut(&tab) {
                        node.swap_leaves(*a, *b);
                    }
                }
            }
            CraneShellAction::SendKeys(ks) => {
                if self.renaming.is_some() {
                    self.edit_rename(ks);
                } else if self.pending_new_entry.is_some() {
                    self.edit_new_entry(ks, ctx);
                } else if self.git_log_branch_prompt.is_some() {
                    self.edit_git_log_branch_prompt(ks, ctx);
                } else if self.git_log_filter_active {
                    self.edit_git_log_filter(ks, ctx);
                } else if self.commit_focused {
                    self.edit_commit(ks);
                } else if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.write_keystroke(ks));
                    } else if let Some(h) = self.editor_at(id) {
                        // Warp editor pane: translate the keystroke and apply it.
                        h.update(ctx, |view, vctx| view.input_key(ks, vctx));
                    } else if let Some(h) = self.browser_at(id) {
                        // Browser pane: typing routes to the URL field while it
                        // owns focus; inert otherwise (the WKWebView receives
                        // its own keys natively as AppKit first responder).
                        h.update(ctx, |view, vctx| view.input_key(ks, vctx));
                    } else if let Some(h) = self.file_at(id) {
                        // Editable File pane: route keys to its buffer.
                        h.update(ctx, |view, vctx| {
                            view.edit(ks);
                            vctx.notify();
                        });
                    }
                }
            }
            CraneShellAction::SaveFocusedFile => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    h.update(ctx, |view, _| {
                        view.save();
                    });
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    // Route through `save_on_cmd_s`: when `format_on_save` is on
                    // (and a formatter for this file's language is on PATH), it
                    // formats the buffer off-thread and writes the formatted text;
                    // otherwise it falls through to the plain synchronous `save`.
                    // A formatter error never mutates the file — the original
                    // buffer bytes are written unchanged.
                    let fmt = self.format_on_save;
                    h.update(ctx, |view, vctx| {
                        view.save_on_cmd_s(vctx, fmt);
                    });
                    // Notify the LSP of the on-disk save (rust-analyzer runs
                    // cargo check on didSave for full error coverage). Reads the
                    // just-saved buffer back so the server sees the same bytes.
                    // When formatting runs async the server re-syncs on the next
                    // `poll_lsp` tick (it watches `buffer_version`), so no extra
                    // wiring is needed here.
                    let (path, text) =
                        h.read(ctx, |v, app| (v.file_path().to_path_buf(), v.buffer_text(app)));
                    if !path.as_os_str().is_empty() {
                        self.lsp.did_save(&path, &text, &self.lsp_configs);
                    }
                }
            }
            CraneShellAction::FindFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.open_find(vctx));
                }
            }
            CraneShellAction::ReplaceFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.open_replace(vctx));
                }
            }
            CraneShellAction::GotoLineFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.open_goto_line(vctx));
                }
            }
            CraneShellAction::UndoFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    h.update(ctx, |view, vctx| {
                        view.undo();
                        vctx.notify();
                    });
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.undo(vctx));
                } else {
                    // No text buffer owns focus → undo the last Files-tree op
                    // (old `undo_last_file_op`): a move renames back, a copy
                    // goes to the Trash.
                    let _ = self.undo_file_op(ctx);
                }
            }
            CraneShellAction::RedoFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    h.update(ctx, |view, vctx| {
                        view.redo();
                        vctx.notify();
                    });
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.redo(vctx));
                }
            }
            CraneShellAction::CopyFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.terminal_at(id)) {
                    if let Some(text) = h.update(ctx, |view, _| view.copy_selection()) {
                        ctx.clipboard()
                            .write(warpui::clipboard::ClipboardContent::plain_text(text));
                    }
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.copy(vctx));
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    if let Some(text) = h.update(ctx, |view, _| view.copy_line()) {
                        ctx.clipboard()
                            .write(warpui::clipboard::ClipboardContent::plain_text(text));
                    }
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.browser_at(id)) {
                    h.update(ctx, |view, vctx| view.url_copy(vctx));
                }
            }
            CraneShellAction::CutFocused => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.cut(vctx));
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    if let Some(text) = h.update(ctx, |view, vctx| {
                        let t = view.cut_line();
                        vctx.notify();
                        t
                    }) {
                        ctx.clipboard()
                            .write(warpui::clipboard::ClipboardContent::plain_text(text));
                    }
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.browser_at(id)) {
                    h.update(ctx, |view, vctx| view.url_cut(vctx));
                }
            }
            CraneShellAction::FileTabSelect(i) => {
                if let Some(fp) = self.files_pane {
                    self.focused = Some(fp);
                    if let Some(path) = self.file_pane_paths.get(*i).cloned() {
                        self.file_pane_active = *i;
                        // Swap the document pane to show this file (Markdown or Editor).
                        if let Some(pc) = self.file_tab_pane(&path) {
                            self.panes.insert(fp, pc);
                        }
                    }
                }
            }
            CraneShellAction::FileTabClose(i) => {
                // Guard: closing a File Tab whose editor buffer has unsaved
                // edits confirms first (the top-level Tab confirm doesn't cover
                // this per-file path). Clean buffers close immediately.
                let dirty = self
                    .file_pane_paths
                    .get(*i)
                    .and_then(|p| self.editor_views.get(p))
                    .map(|h| h.as_ref(&*ctx).is_dirty(&*ctx))
                    .unwrap_or(false);
                if dirty {
                    self.modal = Some(Modal::ConfirmCloseFileTab { index: *i });
                } else {
                    let a = CraneShellAction::FileTabCloseConfirmed(*i);
                    self.handle_action(&a, ctx);
                }
            }
            CraneShellAction::FileTabCloseConfirmed(i) => {
                self.modal = None;
                if let Some(fp) = self.files_pane {
                    if *i < self.file_pane_paths.len() {
                        let removed = self.file_pane_paths.remove(*i);
                        self.editor_views.remove(&removed);
                        self.markdown_views.remove(&removed);
                        if self.file_pane_paths.is_empty() {
                            // Last tab closed — close the whole editor pane.
                            self.files_pane = None;
                            self.file_pane_active = 0;
                            self.focused = Some(fp);
                            self.close_focused(ctx);
                        } else {
                            if self.file_pane_active >= self.file_pane_paths.len() {
                                self.file_pane_active = self.file_pane_paths.len() - 1;
                            } else if self.file_pane_active > *i {
                                self.file_pane_active -= 1;
                            }
                            let path = self.file_pane_paths[self.file_pane_active].clone();
                            if let Some(pc) = self.file_tab_pane(&path) {
                                self.panes.insert(fp, pc);
                            }
                        }
                    }
                }
            }
            CraneShellAction::FocusCommit => self.commit_focused = true,
            CraneShellAction::CommitStaged => self.commit_now(),
            CraneShellAction::PasteFocused => {
                // Full clipboard content (not just plain_text): a terminal
                // pane needs the image branch too, matching Cmd+V's old-Crane
                // behavior of pasting an image-clipboard entry by file path.
                let content = ctx.clipboard().read();
                if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.paste_clipboard(&content));
                    } else if let Some(h) = self.file_at(id) {
                        h.update(ctx, |view, vctx| {
                            view.paste_at_cursor(&content.plain_text);
                            vctx.notify();
                        });
                    } else if let Some(h) = self.editor_at(id) {
                        h.update(ctx, |view, vctx| view.paste(&content.plain_text, vctx));
                    } else if let Some(h) = self.browser_at(id) {
                        // Reads the clipboard itself (strips newlines, honors a
                        // select-all'd buffer) — only acts when the URL field is
                        // focused, else the WKWebView handles paste natively.
                        h.update(ctx, |view, vctx| view.url_paste(vctx));
                    }
                }
            }
            CraneShellAction::SelectAllFocused => {
                // Terminal panes select the whole grid (old terminal/view.rs
                // Cmd+A); editor panes select all buffer text; a focused Browser
                // URL field selects its whole buffer.
                if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.select_all());
                    } else if let Some(h) = self.editor_at(id) {
                        h.update(ctx, |view, vctx| view.select_all(vctx));
                    } else if let Some(h) = self.browser_at(id) {
                        h.update(ctx, |view, vctx| view.url_select_all(vctx));
                    }
                }
            }
            CraneShellAction::ClearFocused => {
                if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.clear_screen());
                        // Allow this terminal's persisted history to shrink to the
                        // cleared state (overrides the never-shrink guard).
                        self.term_cleared.borrow_mut().insert(id);
                    }
                }
            }
            CraneShellAction::StagePaths(paths) => {
                if let Some(root) = self.active_cwd.clone() {
                    self.commit_error = None;
                    for p in paths {
                        if let Err(e) = crate::warpui::git::stage(&root, p) {
                            self.commit_error = Some(e);
                            break;
                        }
                    }
                    self.refresh_panel(ctx);
                    self.invalidate_editor_diffs(&*ctx);
                }
            }
            CraneShellAction::UnstagePaths(paths) => {
                if let Some(root) = self.active_cwd.clone() {
                    self.commit_error = None;
                    for p in paths {
                        if let Err(e) = crate::warpui::git::unstage(&root, p) {
                            self.commit_error = Some(e);
                            break;
                        }
                    }
                    self.refresh_panel(ctx);
                    self.invalidate_editor_diffs(&*ctx);
                }
            }
            CraneShellAction::ToggleChangeDir(key) => {
                if !self.collapsed_change_dirs.remove(key) {
                    self.collapsed_change_dirs.insert(key.clone());
                }
            }
            CraneShellAction::GitPush => self.spawn_op(crate::warpui::git::OpKind::Push),
            CraneShellAction::GitPull => self.spawn_op(crate::warpui::git::OpKind::Pull),
            CraneShellAction::GitFetch => self.spawn_op(crate::warpui::git::OpKind::Fetch),
            CraneShellAction::ShowChangeMenu { path, staged, has_unstaged, x, y } => {
                self.row_menu = Some(RowMenu::Change {
                    path: path.clone(),
                    staged: *staged,
                    has_unstaged: *has_unstaged,
                    x: *x,
                    y: *y,
                });
            }
            CraneShellAction::ShowFileMenu { path, is_dir, x, y } => {
                self.row_menu = Some(RowMenu::File {
                    path: path.clone(),
                    is_dir: *is_dir,
                    x: *x,
                    y: *y,
                });
            }
            CraneShellAction::OpenFileAt(path) => {
                self.row_menu = None;
                self.selected_file = Some(path.clone());
                self.open_file(path.clone(), ctx);
            }
            CraneShellAction::OpenFileAtPath { path, line, col: _ } => {
                // Clicked path in a Terminal pane: open in the editor and, when a
                // `:LINE` suffix was present, scroll to it. Column-precise jumps
                // are recorded on the action but not yet applied (editor exposes
                // goto-line only).
                self.row_menu = None;
                // The grid scanner already resolves against the terminal's cwd, so
                // `path` is normally absolute. Guard the relative case: resolve
                // against the focused terminal's cwd, then the active repo root.
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    let base = self
                        .focused
                        .and_then(|id| self.terminal_at(id))
                        .map(|h| h.read(&*ctx, |v, _| v.cwd()))
                        .or_else(|| self.active_cwd.clone());
                    match base {
                        Some(dir) => dir.join(path),
                        None => path.clone(),
                    }
                };
                self.selected_file = Some(resolved.clone());
                self.open_file(resolved.clone(), ctx);
                if let Some(l) = line {
                    if let Some(h) = self.editor_views.get(&resolved).cloned() {
                        h.update(ctx, |view, vctx| view.goto_line(*l as usize, vctx));
                    }
                }
            }
            CraneShellAction::TermNotification { body, urgent, source } => {
                // A terminal emitted an OSC 9 / OSC 777 desktop notification.
                // `source` is the emitting terminal's owner tab (synced from
                // `layouts`); fall back to the active tab if it hasn't synced yet.
                let source_key = source.or(self.active_tab);
                // Pulse the source tab in the Left Panel unless it's already the
                // active tab (no point nagging about what you're looking at). Only
                // latch the first ping so the dot reflects the earliest one.
                self.flag_attention(source_key);
                let label = source_key
                    .map(|k| self.notif_source_label(k))
                    .unwrap_or_else(|| "Terminal".to_string());
                let id = self.next_toast_id;
                self.next_toast_id = self.next_toast_id.wrapping_add(1);
                if self.toasts.len() >= TOAST_MAX {
                    self.toasts.pop_front();
                }
                self.toasts.push_back(Toast {
                    id,
                    body: body.clone(),
                    urgent: *urgent,
                    source: label,
                    tab_key: source_key,
                    at: std::time::Instant::now(),
                });
                ctx.notify();
            }
            CraneShellAction::TermBell { source } => {
                // Background bell → pulse only (no toast). `flag_attention`
                // ignores the active tab, so a bell in the focused terminal is a
                // no-op here (its audible beep already fired in the paint path).
                // Early-return so frequent bells never spam the `save_state` tail.
                self.flag_attention(source.or(self.active_tab));
                ctx.notify();
                return;
            }
            CraneShellAction::DismissToast(id) => {
                self.toasts.retain(|t| t.id != *id);
            }
            CraneShellAction::FocusToastSource(id) => {
                // Activate the tab the notification came from (best-effort), then
                // dismiss the toast. Reuses the same activation path as a left-panel
                // tab click.
                if let Some(t) = self.toasts.iter().find(|t| t.id == *id) {
                    if let Some(key) = t.tab_key {
                        let (pi, wi, tid) = key;
                        let path = self
                            .projects
                            .get(pi)
                            .and_then(|p| p.worktrees.get(wi))
                            .map(|w| PathBuf::from(&w.path));
                        if let Some(path) = path {
                            self.handle_action(
                                &CraneShellAction::Select { sel: (pi, wi, tid), path },
                                ctx,
                            );
                        }
                    }
                }
                self.toasts.retain(|t| t.id != *id);
            }
            CraneShellAction::CopyPathStr(s) => {
                self.row_menu = None;
                ctx.clipboard()
                    .write(warpui::clipboard::ClipboardContent::plain_text(s.clone()));
            }
            CraneShellAction::RevealPathInFinder(path) => {
                self.row_menu = None;
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
                #[cfg(target_os = "linux")]
                {
                    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("/"));
                    let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
                }
            }
            CraneShellAction::NewEntry { parent, is_folder } => {
                self.row_menu = None;
                self.expanded_dirs.insert(parent.clone());
                self.pending_new_entry = Some(PendingNewEntry {
                    parent: parent.clone(),
                    is_folder: *is_folder,
                    name: String::new(),
                    error: None,
                });
                self.refresh_panel(ctx);
            }
            CraneShellAction::RequestDelete(path) => {
                self.row_menu = None;
                self.pending_delete = Some(path.clone());
            }
            CraneShellAction::ConfirmDelete => {
                if let Some(path) = self.pending_delete.take() {
                    // Recoverable delete: move to the system Trash rather than
                    // permanently unlinking (matches old Crane's
                    // `confirm_delete_file` modal). Works for both files and
                    // directories. Surface any failure instead of silently
                    // dropping the request.
                    if let Err(e) = trash::delete(&path) {
                        self.commit_error = Some(format!("Trash: {e}"));
                    } else {
                        if self.selected_file.as_deref() == Some(path.as_path()) {
                            self.selected_file = None;
                        }
                        // Close any File Tab holding the deleted file (or a file
                        // under a deleted directory) — a surviving dirty buffer
                        // could otherwise re-write the trashed path on Cmd+S.
                        // FileTabCloseConfirmed handles active-index fixup and
                        // tears the pane down when the last tab goes.
                        while let Some(idx) = self
                            .file_pane_paths
                            .iter()
                            .position(|p| p.starts_with(&path))
                        {
                            let a = CraneShellAction::FileTabCloseConfirmed(idx);
                            self.handle_action(&a, ctx);
                        }
                    }
                    self.refresh_panel(ctx);
                }
            }
            CraneShellAction::CancelDelete => {
                self.pending_delete = None;
            }
            CraneShellAction::ShowBranchPicker { x, y } => {
                if let Some(root) = self.active_cwd.clone() {
                    // Open the popover immediately and fill the branch list OFF the
                    // UI thread (`git branch` + `git branch -r` dedup runs in
                    // `branch_candidates`). The overlay renders "(no branches)"
                    // until the async list lands, then repaints. Read-only, so the
                    // only guard needed is dropping a stale scan when a newer
                    // picker opened (generation) or the picker closed.
                    self.branch_list.clear();
                    self.branch_picker = Some((*x, *y));
                    let generation = self.bump_scan_gen("branchpicker");
                    let fut = async move { Self::branch_candidates(&root).0 };
                    ctx.spawn(fut, move |this, list, vctx| {
                        if this.git_scan_gen.get("branchpicker").copied() == Some(generation)
                            && this.branch_picker.is_some()
                        {
                            this.branch_list = list;
                            vctx.notify();
                        }
                    });
                }
            }
            CraneShellAction::CheckoutBranch(branch) => {
                self.branch_picker = None;
                if let Some(root) = self.active_cwd.clone() {
                    match crate::warpui::git::checkout_branch(&root, branch) {
                        Ok(()) => {
                            self.sync_worktree_branch_label(&root);
                            self.refresh_panel(ctx);
                            self.invalidate_editor_diffs(&*ctx);
                        }
                        Err(e) => self.commit_error = Some(e),
                    }
                }
            }
            CraneShellAction::FocusPrevPane | CraneShellAction::FocusNextPane => {
                if let Some(tab) = self.active_tab {
                    if let Some(node) = self.layouts.get(&tab) {
                        let mut leaves = Vec::new();
                        node.leaves(&mut leaves);
                        if leaves.len() > 1 {
                            let cur = self.focused.and_then(|f| leaves.iter().position(|&l| l == f)).unwrap_or(0);
                            let next = if matches!(action, CraneShellAction::FocusNextPane) {
                                (cur + 1) % leaves.len()
                            } else {
                                (cur + leaves.len() - 1) % leaves.len()
                            };
                            self.focused = Some(leaves[next]);
                            self.commit_focused = false;
                        }
                    }
                }
            }
            CraneShellAction::CloseActiveTab => {
                if let Some(tab) = self.active_tab {
                    // Reuse the full CloseTab teardown path.
                    let cloned = CraneShellAction::CloseTab(tab);
                    self.handle_action(&cloned, ctx);
                }
            }
            CraneShellAction::OpenGitLog => self.toggle_gitlog(ctx),
            CraneShellAction::GitLogSelect(sha) => self.select_git_log_commit(sha.clone(), ctx),
            CraneShellAction::GitLogShowMenu { sha, x, y } => {
                self.git_log_menu = Some((sha.clone(), *x, *y));
            }
            CraneShellAction::GitLogCheckout(sha) => {
                self.git_log_menu = None;
                self.run_git_log_op(ctx, sha.clone(), GitLogOp::Checkout);
            }
            CraneShellAction::GitLogCherryPick(sha) => {
                self.git_log_menu = None;
                self.run_git_log_op(ctx, sha.clone(), GitLogOp::CherryPick);
            }
            CraneShellAction::GitLogRevert(sha) => {
                self.git_log_menu = None;
                self.run_git_log_op(ctx, sha.clone(), GitLogOp::Revert);
            }
            CraneShellAction::GitLogDetailFile(fi) => {
                self.git_log_detail_file = *fi;
                self.git_log_detail_scroll = 0;
                // Also open the file's CURRENT working-tree copy in the File
                // Edit pane (the inline patch above already shows the
                // commit's historical diff). Repo-relative path from the
                // commit detail, joined against the log's own repo root —
                // silently skipped if the file no longer exists on disk
                // (deleted / renamed since that commit).
                if let (Some(repo), Some(rel)) = (
                    self.git_log_repo.clone(),
                    self.git_log_detail.as_ref().and_then(|d| d.files.get(*fi)).map(|f| f.path.clone()),
                ) {
                    let abs = repo.join(&rel);
                    if abs.is_file() {
                        self.open_file(abs, ctx);
                    }
                }
            }
            CraneShellAction::GitLogBranchPrompt(sha) => {
                // Keep git_log_menu's (x, y) — the prompt overlay anchors
                // there; the menu itself stops rendering while the prompt is
                // open (render gates on branch_prompt.is_none()).
                self.git_log_branch_prompt = Some((sha.clone(), String::new()));
            }
            CraneShellAction::GitLogSetRefFilter(r) => {
                // Clicking the already-active ref clears the scope (toggle).
                self.git_log_ref_filter = if self.git_log_ref_filter == *r {
                    None
                } else {
                    r.clone()
                };
                self.reload_git_log(ctx);
            }
            CraneShellAction::GitLogFocusFilter => {
                self.git_log_filter_active = true;
            }
            CraneShellAction::GitLogStepSelection(down) => {
                let shown = self.git_log_shown_frame();
                if let Some(frame) = shown {
                    if let Some(sha) = crate::warpui::git_log::step_selection(
                        &frame.commits,
                        self.git_log_selected.as_deref(),
                        *down,
                    ) {
                        let row = frame
                            .commits
                            .iter()
                            .position(|c| c.sha == sha)
                            .unwrap_or(0);
                        self.select_git_log_commit(sha, ctx);
                        // Reveal with a conservative viewport estimate — the
                        // element owns the real row math; 15 rows keeps the
                        // selection comfortably in view for any dock height.
                        self.git_log_scroll.set(crate::warpui::git_log::reveal_offset(
                            self.git_log_scroll.get(),
                            row,
                            15,
                        ));
                    }
                }
            }
            CraneShellAction::GitLogFetchAll => {
                if !self.git_log_fetching {
                    if let Some(repo) = self.active_cwd.clone() {
                        self.git_log_fetching = true;
                        let fut = async move { crate::warpui::git::fetch_all(&repo) };
                        ctx.spawn(fut, move |this, res, vctx| {
                            this.git_log_fetching = false;
                            if let Err(e) = res {
                                this.commit_error = Some(e);
                            }
                            // The .git/refs FileWatcher usually fires too, but a
                            // no-op fetch produces no ref writes — reload anyway
                            // so "up to date" still refreshes the frame.
                            this.reload_git_log(vctx);
                            vctx.notify();
                        });
                    }
                }
            }
            CraneShellAction::GitLogDetailScroll(delta) => {
                let max = self
                    .git_log_detail
                    .as_ref()
                    .map(|d| d.diff.len().saturating_sub(1))
                    .unwrap_or(0);
                let next = self.git_log_detail_scroll as i64 + *delta as i64;
                self.git_log_detail_scroll = next.clamp(0, max as i64) as usize;
                ctx.notify();
            }
            CraneShellAction::OpenBrowser => self.open_browser(ctx),
            CraneShellAction::RelayoutPanes => {
                self.relayout_panes(ctx);
                // Skip the tail: per-drag-tick save_state / focus churn would
                // hammer the disk while the splitter moves.
                return;
            }
            CraneShellAction::TreeDragStart(drag) => {
                self.tree_drag = Some(drag.clone());
            }
            CraneShellAction::TreeDrop { x, y } => {
                self.apply_tree_drop(vec2f(*x, *y), ctx);
            }
            CraneShellAction::FsDragStart(path) => {
                self.fs_drag = Some(path.clone());
            }
            CraneShellAction::FsDrop { x, y } => {
                self.apply_fs_drop(vec2f(*x, *y), ctx);
            }
            CraneShellAction::FsExternalDrop { paths, x, y } => {
                self.apply_fs_external_drop(paths.clone(), vec2f(*x, *y), ctx);
            }
            CraneShellAction::BrowserNewTab => {
                if let Some(h) = self.focused.and_then(|id| self.browser_at(id)) {
                    h.update(ctx, |view, vctx| {
                        view.handle_action(
                            &crate::warpui::browser_view::BrowserAction::NewTab,
                            vctx,
                        )
                    });
                } else {
                    self.open_browser(ctx);
                }
            }
            CraneShellAction::OpenDiff(p) => self.open_diff(p.clone(), ctx),
            CraneShellAction::OpenWelcome => self.open_welcome(ctx),
            CraneShellAction::FontZoomIn
            | CraneShellAction::FontZoomOut
            | CraneShellAction::FontZoomReset => {
                let step = crate::warpui::fontsize::step();
                let level = match action {
                    CraneShellAction::FontZoomIn => crate::warpui::fontsize::zoom(step),
                    CraneShellAction::FontZoomOut => crate::warpui::fontsize::zoom(-step),
                    _ => crate::warpui::fontsize::reset(),
                };
                // Global magnification: scales EVERY rendered element (panels,
                // tabs, breadcrumb, status bar, menus, terminal, editor) and
                // invalidates all views — so no manual per-pane repaint needed.
                ctx.set_zoom_factor(level);
                #[cfg(debug_assertions)]
                eprintln!("crane: zoom action fired, level={level}");
                self.save_state(&*ctx);
            }
            CraneShellAction::NewTab => {
                match self.active_tab {
                    Some((pi, wi, _)) => self.add_tab(pi, wi, ctx),
                    // Empty state: seed a default tab so Cmd+Shift+T still works.
                    None => self.add_tab(0, 0, ctx),
                }
            }
            CraneShellAction::NewTabIn(pi, wi) => self.add_tab(*pi, *wi, ctx),
            CraneShellAction::CloseTab((pi, wi, tid)) => {
                // Guard: if this tab holds a running terminal or an editor with
                // unsaved edits, confirm before tearing it (and its PTYs) down.
                // Otherwise close immediately. The confirm's affirmative button
                // dispatches CloseTabConfirmed.
                let key = (*pi, *wi, *tid);
                let (running, unsaved) = self.tab_close_hazards(key, &*ctx);
                if running || unsaved {
                    self.tab_menu = None;
                    self.modal = Some(Modal::ConfirmCloseTab { key });
                } else {
                    let a = CraneShellAction::CloseTabConfirmed(key);
                    self.handle_action(&a, ctx);
                }
            }
            CraneShellAction::CloseTabConfirmed((pi, wi, tid)) => {
                self.modal = None;
                self.title_debounce.borrow_mut().remove(tid);
                // Drop the tab's layout + every pane it owns.
                if let Some(node) = self.layouts.remove(&(*pi, *wi, *tid)) {
                    let mut leaves = Vec::new();
                    node.leaves(&mut leaves);
                    for l in leaves {
                        // Fully tear down each pane: view (kills PTY), drag
                        // state, and cached rect — no ghosts left behind.
                        self.panes.remove(&l);
                        self.drag_states.remove(&l);
                        self.pane_rects.borrow_mut().remove(&l);
                    }
                }
                if let Some(tabs) = self.worktree_tabs.get_mut(&(*pi, *wi)) {
                    tabs.retain(|t| t.id != *tid);
                    // If the closed tab was active, fall back to the first one.
                    if self.active_tab == Some((*pi, *wi, *tid)) {
                        if let Some(first) = tabs.first() {
                            let key = (*pi, *wi, first.id);
                            self.active_tab = Some(key);
                            self.selected = key;
                            self.focused = self.layouts.get(&key).map(|n| n.first_leaf());
                        } else {
                            self.active_tab = None;
                            self.focused = None;
                        }
                    }
                }
            }
            // Cmd+B / Cmd+/ toggle the side panels only when no text-input widget
            // holds focus — don't toggle while typing in an editor / commit box.
            // Mirrors old shortcuts.rs `if toggle_left && !any_focus`.
            CraneShellAction::ToggleLeft => {
                if !self.any_text_input_focused(&*ctx) {
                    self.show_left = !self.show_left;
                    // Defensive clear (same pattern as `context_menu` in the
                    // structural handlers below): collapsing the panel unmounts
                    // every tooltip-owning button in the same frame, so the
                    // owning Hoverable never fires its hover-out and a shown
                    // tooltip would otherwise stay painted at stale coordinates.
                    self.hover_tip = None;
                }
            }
            CraneShellAction::ToggleRight => {
                if !self.any_text_input_focused(&*ctx) {
                    self.show_right = !self.show_right;
                }
            }
            CraneShellAction::CommentOrToggleRight => {
                // Editor pane focused → toggle the line comment; otherwise fall
                // back to the Right Panel toggle (Cmd+/'s legacy behavior).
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |v, vctx| {
                        v.apply(&crate::warpui::editor_view::EditAction::ToggleComment, vctx)
                    });
                } else {
                    self.handle_action(&CraneShellAction::ToggleRight, ctx);
                }
            }
            CraneShellAction::ToggleWordWrap => {
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |v, vctx| v.toggle_word_wrap(vctx));
                }
            }
            CraneShellAction::SetTab { files } => {
                self.files_tab = *files;
                self.refresh_panel(ctx);
            }
            CraneShellAction::ToggleDir(p) => {
                if !self.expanded_dirs.remove(p) {
                    self.expanded_dirs.insert(p.clone());
                }
                self.refresh_panel(ctx);
            }
            CraneShellAction::SelectFile(p) => {
                self.selected_file = Some(p.clone());
                self.open_file(p.clone(), ctx);
            }
            CraneShellAction::ToggleProject(i) => {
                if !self.expanded_projects.remove(i) {
                    self.expanded_projects.insert(*i);
                }
            }
            CraneShellAction::ToggleWorktree(p, w) => {
                let k = (*p, *w);
                if !self.expanded_worktrees.remove(&k) {
                    self.expanded_worktrees.insert(k);
                }
            }
            CraneShellAction::ToggleGroup(g) => {
                if !self.collapsed_groups.remove(g) {
                    self.collapsed_groups.insert(g.clone());
                }
            }
            CraneShellAction::ShowFolderMenu { group, x, y } => {
                self.folder_menu = Some((group.clone(), *x, *y));
            }
            CraneShellAction::SetGroupTint { group, tint } => {
                self.folder_menu = None;
                match tint {
                    Some(rgb) => {
                        self.group_tints.insert(group.clone(), *rgb);
                    }
                    None => {
                        self.group_tints.remove(group);
                    }
                }
            }
            CraneShellAction::RemoveGroup(group) => {
                self.folder_menu = None;
                // Member rows (and their tooltip-owning ＋s) unmount without a hover-out.
                self.hover_tip = None;
                self.remove_group(group, ctx);
            }
            CraneShellAction::SetTheme(name) => {
                if let Some(t) = crate::theme::find_by_name(name) {
                    crate::theme::set(t);
                }
            }
            CraneShellAction::ShowTooltip { text, x, y } => {
                self.hover_tip = Some((text.clone(), *x, *y));
            }
            CraneShellAction::HideTooltip => {
                self.hover_tip = None;
            }
            CraneShellAction::AddProject => {
                // Run the native folder picker as an ASYNC future so it does NOT
                // re-enter warpui's borrowed event dispatch (a blocking sync modal
                // here panics with "RefCell already borrowed"). The callback runs
                // on the main thread once the user confirms/cancels.
                let fut = rfd::AsyncFileDialog::new()
                    .set_title("Choose project folder")
                    .pick_folder();
                ctx.spawn(fut, |this, res: Option<rfd::FileHandle>, vctx| {
                    if let Some(folder) = res {
                        let p = folder.path().to_path_buf();
                        let path_str = p.to_string_lossy().to_string();
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path_str.clone());
                        if !this.projects.iter().any(|p| p.path == path_str)
                            && !this.added_projects.iter().any(|a| a.path == path_str)
                        {
                            let ap = crate::warpui::persist::AddedProject {
                                name,
                                path: path_str.clone(),
                            };
                            this.added_projects.push(ap.clone());
                            // Re-add in case the user had previously removed it.
                            this.removed_project_paths.retain(|r| r != &path_str);
                            // Shallow-expand ONLY the picked folder and APPEND it —
                            // no whole-tree reload and ZERO synchronous `git` on the
                            // UI thread. Appending (vs a full rebuild) keeps every
                            // existing project's (pi, *)-keyed state + already-filled
                            // badges intact. The new project appears + is usable
                            // instantly; its branch/diff/dirty fill in via the scan
                            // below.
                            let start = this.projects.len();
                            let new_nodes = crate::warpui::projects::load_one_shallow(
                                &ap,
                                &this.removed_project_paths,
                                &this.project_tints,
                            );
                            this.projects.extend(new_nodes);
                            // Watch the newly added Project(s) + their Workspaces so
                            // external / agent edits refresh the active repo.
                            this.sync_watches();
                            let new_paths = crate::warpui::projects::scan_paths(
                                &this.projects[start..],
                            );
                            this.spawn_git_scan(vctx, format!("add:{path_str}"), new_paths);
                            this.save_state(&*vctx);
                        }
                        // Open (expand + activate) the picked project so it becomes
                        // usable immediately. A container folder (git children) has
                        // no single project entry — its children just appear.
                        if let Some(pi) = this.projects.iter().position(|p| p.path == path_str) {
                            this.open_project(pi, vctx);
                        }
                    }
                    vctx.notify();
                });
            }
            CraneShellAction::OpenExternalFile => {
                // Async native file picker (see AddProject) — a sync modal here
                // re-enters warpui's borrowed dispatch and panics. Open the chosen
                // file into the Files pane on the main-thread callback.
                let fut = rfd::AsyncFileDialog::new()
                    .set_title("Open file")
                    .pick_file();
                ctx.spawn(fut, |this, res: Option<rfd::FileHandle>, vctx| {
                    if let Some(f) = res {
                        let path = f.path().to_path_buf();
                        this.selected_file = Some(path.clone());
                        this.open_file(path, vctx);
                    }
                    // ctx.spawn does not auto-dirty the view — without this the
                    // opened Editor pane stays invisible until an unrelated event.
                    vctx.notify();
                });
            }
            CraneShellAction::RemoveProject(i) => {
                self.context_menu = None;
                // Row (and its tooltip-owning ＋) unmounts without a hover-out.
                self.hover_tip = None;
                self.remove_project_at(*i, ctx);
            }
            CraneShellAction::ShowProjectMenu { project_idx, x, y } => {
                self.context_menu = Some(ProjectContextMenu {
                    project_idx: *project_idx,
                    x: *x,
                    y: *y,
                });
            }
            CraneShellAction::ShowWorktreeMenu { pi, wi, x, y } => {
                self.worktree_menu = Some((*pi, *wi, *x, *y));
            }
            CraneShellAction::ShowTabMenu { key, x, y } => {
                self.tab_menu = Some((*key, *x, *y));
            }
            CraneShellAction::CloseOtherTabs((pi, wi, tid)) => {
                self.tab_menu = None;
                let others: Vec<usize> = self
                    .worktree_tabs
                    .get(&(*pi, *wi))
                    .map(|tabs| {
                        tabs.iter().filter(|t| t.id != *tid).map(|t| t.id).collect()
                    })
                    .unwrap_or_default();
                for oid in others {
                    // Bulk close bypasses the per-tab confirm modal (only one
                    // modal can be open at a time); tear each down directly.
                    let a = CraneShellAction::CloseTabConfirmed((*pi, *wi, oid));
                    self.handle_action(&a, ctx);
                }
                // The kept tab becomes the active one.
                let key = (*pi, *wi, *tid);
                if self.layouts.contains_key(&key) {
                    self.active_tab = Some(key);
                    self.selected = key;
                    self.focused = self.layouts.get(&key).map(|n| n.first_leaf());
                }
            }
            CraneShellAction::WorktreeRowClick { pi, wi } => {
                // Double-click (same row within 400ms) starts an inline rename;
                // a single click toggles expand (mirrors old egui behaviour).
                let now = std::time::Instant::now();
                let dbl = self
                    .last_wt_click
                    .map(|((lp, lw), t)| {
                        lp == *pi
                            && lw == *wi
                            && now.duration_since(t) < std::time::Duration::from_millis(400)
                    })
                    .unwrap_or(false);
                if dbl {
                    self.last_wt_click = None;
                    let a = CraneShellAction::StartRenameWorktree { pi: *pi, wi: *wi };
                    self.handle_action(&a, ctx);
                } else {
                    self.last_wt_click = Some(((*pi, *wi), now));
                    let k = (*pi, *wi);
                    if !self.expanded_worktrees.remove(&k) {
                        self.expanded_worktrees.insert(k);
                    }
                }
            }
            CraneShellAction::TabRowClick { key, path } => {
                let now = std::time::Instant::now();
                let dbl = self
                    .last_tab_click
                    .map(|(k, t)| {
                        k == *key
                            && now.duration_since(t) < std::time::Duration::from_millis(400)
                    })
                    .unwrap_or(false);
                if dbl {
                    self.last_tab_click = None;
                    let a = CraneShellAction::StartRenameTab { key: *key };
                    self.handle_action(&a, ctx);
                } else {
                    self.last_tab_click = Some((*key, now));
                    let a = CraneShellAction::Select {
                        sel: *key,
                        path: path.clone(),
                    };
                    self.handle_action(&a, ctx);
                }
            }
            CraneShellAction::StartRenameWorktree { pi, wi } => {
                self.worktree_menu = None;
                if let Some(w) = self.projects.get(*pi).and_then(|p| p.worktrees.get(*wi)) {
                    let cur = self
                        .worktree_names
                        .get(&w.path)
                        .cloned()
                        .unwrap_or_else(|| w.name.clone());
                    self.renaming = Some(RenameState {
                        target: RenameTarget::Worktree { pi: *pi, wi: *wi },
                        buffer: cur,
                    });
                }
            }
            CraneShellAction::StartRenameTab { key } => {
                self.tab_menu = None;
                let cur = self
                    .worktree_tabs
                    .get(&(key.0, key.1))
                    .and_then(|tabs| tabs.iter().find(|t| t.id == key.2))
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                self.renaming = Some(RenameState {
                    target: RenameTarget::Tab { key: *key },
                    buffer: cur,
                });
            }
            CraneShellAction::RemoveWorktree { pi, wi } => {
                self.worktree_menu = None;
                // Member rows (and their tooltip-owning ＋s) unmount without a hover-out.
                self.hover_tip = None;
                let pi = *pi;
                let wi = *wi;
                let Some(main_path) = self.projects.get(pi).map(|p| p.path.clone()) else {
                    return;
                };
                let Some((wt_path, wt_name, wt_dirty)) = self
                    .projects
                    .get(pi)
                    .and_then(|p| p.worktrees.get(wi))
                    .map(|w| (w.path.clone(), w.name.clone(), w.dirty))
                else {
                    return;
                };
                // Primary working tree can't be `git worktree remove`d — there is
                // no worktree to detach (the project itself would have to be
                // removed). Don't even open the confirm; it would be a no-op.
                if wt_path.is_empty() || wt_path == main_path {
                    return;
                }
                // Compute the dirty / unpushed WARNING once (a couple of quick
                // shell-outs) and stash it for the card. The destructive
                // `git worktree remove` runs only on explicit confirm
                // (RemoveWorktreeConfirmed).
                let label = self
                    .worktree_names
                    .get(&wt_path)
                    .cloned()
                    .unwrap_or(wt_name);
                let wt_pathbuf = std::path::PathBuf::from(&wt_path);
                let dirty = wt_dirty || crate::warpui::git::is_dirty(&wt_pathbuf);
                let ahead = crate::warpui::git::ahead_behind(&wt_pathbuf)
                    .map(|(a, _)| a)
                    .unwrap_or(0);
                self.remove_wt_info = Some(RemoveWtInfo {
                    label,
                    path: wt_path,
                    dirty,
                    ahead,
                });
                self.modal = Some(Modal::ConfirmRemoveWorktree { pi, wi });
            }
            CraneShellAction::RemoveWorktreeConfirmed { pi, wi } => {
                self.modal = None;
                self.remove_wt_info = None;
                let pi = *pi;
                let wi = *wi;
                let Some(main_path) = self.projects.get(pi).map(|p| p.path.clone()) else {
                    return;
                };
                let Some(wt_path) = self
                    .projects
                    .get(pi)
                    .and_then(|p| p.worktrees.get(wi))
                    .map(|w| w.path.clone())
                else {
                    return;
                };
                // Guard: the primary working tree can't be `git worktree remove`d.
                // NOTE(completion): removing the main working tree is a no-op —
                // there is no worktree to detach; the project itself would have to
                // be removed instead (RemoveProject).
                if wt_path.is_empty() || wt_path == main_path {
                    return;
                }
                // Detach the worktree from git (local op; `--force` so dirty trees
                // still remove). Ignore failure — we still drop it from the UI.
                let _ = crate::warpui::git::remove_worktree(
                    std::path::Path::new(&main_path),
                    std::path::Path::new(&wt_path),
                );
                // Remove it in-memory and remap every (pi, wi, *)-keyed structure
                // by PATH (robust to the index shift), mirroring RemoveProject.
                let old_wt_paths: Vec<String> = self
                    .projects
                    .get(pi)
                    .map(|p| p.worktrees.iter().map(|w| w.path.clone()).collect())
                    .unwrap_or_default();
                if let Some(p) = self.projects.get_mut(pi) {
                    if wi < p.worktrees.len() {
                        p.worktrees.remove(wi);
                    }
                }
                // Stop watching the detached Workspace root.
                self.sync_watches();
                let new_wt_index: HashMap<String, usize> = self
                    .projects
                    .get(pi)
                    .map(|p| {
                        p.worktrees
                            .iter()
                            .enumerate()
                            .map(|(i, w)| (w.path.clone(), i))
                            .collect()
                    })
                    .unwrap_or_default();
                // old worktree index (within pi) -> new index, None = removed.
                let remap_w = |w: usize| -> Option<usize> {
                    old_wt_paths.get(w).and_then(|pt| new_wt_index.get(pt).copied())
                };
                // 1) Tear down layouts (+ PTYs) for the vanished worktree in pi.
                let dead: Vec<(usize, usize, usize)> = self
                    .layouts
                    .keys()
                    .copied()
                    .filter(|(p, w, _)| *p == pi && remap_w(*w).is_none())
                    .collect();
                for key in dead {
                    self.tear_down_layout(key);
                }
                // 2) Rekey the surviving layouts within pi.
                let old_layouts = std::mem::take(&mut self.layouts);
                for ((p, w, tid), node) in old_layouts {
                    if p == pi {
                        if let Some(nw) = remap_w(w) {
                            self.layouts.insert((p, nw, tid), node);
                        }
                    } else {
                        self.layouts.insert((p, w, tid), node);
                    }
                }
                // 3) Rekey worktree_tabs.
                let old_tabs = std::mem::take(&mut self.worktree_tabs);
                for ((p, w), tabs) in old_tabs {
                    if p == pi {
                        if let Some(nw) = remap_w(w) {
                            self.worktree_tabs.insert((p, nw), tabs);
                        }
                    } else {
                        self.worktree_tabs.insert((p, w), tabs);
                    }
                }
                // 4) Rekey expand state.
                self.expanded_worktrees = self
                    .expanded_worktrees
                    .iter()
                    .filter_map(|(p, w)| {
                        if *p == pi {
                            remap_w(*w).map(|nw| (*p, nw))
                        } else {
                            Some((*p, *w))
                        }
                    })
                    .collect();
                // 5) Repoint active_tab / selected.
                self.active_tab = self.active_tab.and_then(|(p, w, tid)| {
                    if p == pi {
                        remap_w(w).map(|nw| (p, nw, tid))
                    } else {
                        Some((p, w, tid))
                    }
                });
                let (sp, sw, st) = self.selected;
                self.selected = if sp == pi {
                    match remap_w(sw) {
                        Some(nw) => (sp, nw, st),
                        None => (0, 0, usize::MAX),
                    }
                } else {
                    (sp, sw, st)
                };
                // 6) Clear focused / files pane whose backing pane was torn down.
                if let Some(fp) = self.files_pane {
                    if !self.panes.contains_key(&fp) {
                        self.files_pane = None;
                        self.file_pane_paths.clear();
                        self.file_pane_active = 0;
                    }
                }
                if let Some(f) = self.focused {
                    if !self.panes.contains_key(&f) {
                        self.focused = None;
                    }
                }
                match self.active_tab {
                    Some(at) => {
                        if self.focused.is_none() {
                            self.focused = self.layouts.get(&at).map(|n| n.first_leaf());
                        }
                    }
                    None => {
                        self.focused = None;
                        self.active_cwd = None;
                    }
                }
                // Drop the removed worktree's path-keyed overrides.
                self.worktree_names.remove(&wt_path);
                self.worktree_tints.remove(&wt_path);
                self.tab_tints.retain(|(pt, _), _| pt != &wt_path);
                self.refresh_panel(ctx);
            }
            CraneShellAction::SetWorktreeTint { pi, wi, tint } => {
                self.worktree_menu = None;
                if let Some(w) = self.projects.get(*pi).and_then(|p| p.worktrees.get(*wi)) {
                    let path = w.path.clone();
                    match tint {
                        Some(rgb) => {
                            self.worktree_tints.insert(path, *rgb);
                        }
                        None => {
                            self.worktree_tints.remove(&path);
                        }
                    }
                }
            }
            CraneShellAction::SetTabTint { key, tint } => {
                self.tab_menu = None;
                let (pi, wi, tid) = *key;
                if let Some(w) = self.projects.get(pi).and_then(|p| p.worktrees.get(wi)) {
                    let path = w.path.clone();
                    match tint {
                        Some(rgb) => {
                            self.tab_tints.insert((path, tid), *rgb);
                        }
                        None => {
                            self.tab_tints.remove(&(path, tid));
                        }
                    }
                }
            }
            CraneShellAction::CloseContextMenu => {
                self.context_menu = None;
                self.row_menu = None;
                self.branch_picker = None;
                self.worktree_menu = None;
                self.tab_menu = None;
                self.folder_menu = None;
                self.git_log_menu = None;
                self.git_log_branch_prompt = None;
                self.new_pane_menu_open = false;
            }
            CraneShellAction::ToggleNewPaneMenu => {
                self.new_pane_menu_open = !self.new_pane_menu_open;
            }
            CraneShellAction::RevealProjectInFinder(i) => {
                self.context_menu = None;
                if let Some(p) = self.projects.get(*i) {
                    let path = p.path.clone();
                    #[cfg(target_os = "macos")]
                    let _ = std::process::Command::new("open").arg(&path).spawn();
                    #[cfg(target_os = "linux")]
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                }
            }
            CraneShellAction::CopyProjectPath(i) => {
                self.context_menu = None;
                if let Some(p) = self.projects.get(*i) {
                    ctx.clipboard().write(
                        warpui::clipboard::ClipboardContent::plain_text(p.path.clone()),
                    );
                }
            }
            CraneShellAction::SetProjectTint(i, tint) => {
                self.context_menu = None;
                // Update the tint IN PLACE — set both the in-memory node's `tint`
                // field (read by the sidebar render + save_state) and the persisted
                // `project_tints` map. NEVER call `reload_projects()` here: that
                // re-shells git (current_branch / diff_numstat / is_dirty) for every
                // project + worktree on the machine, which is dozens of subprocess
                // spawns per color click. A tint change touches no git state.
                if let Some(p) = self.projects.get_mut(*i) {
                    let path = p.path.clone();
                    p.tint = *tint;
                    match tint {
                        Some(rgb) => {
                            self.project_tints.insert(path, *rgb);
                        }
                        None => {
                            self.project_tints.remove(&path);
                        }
                    }
                }
            }
            CraneShellAction::InitGitProject(i) => {
                self.context_menu = None;
                if let Some(p) = self.projects.get(*i) {
                    let dir = std::path::PathBuf::from(&p.path);
                    // Shell out `git init` — never libgit2, per project rules.
                    let _ = crate::warpui::git::init(&dir);
                    // The path was cached as a loose (branch-less, clean) folder;
                    // `git init` just changed that, so drop its cached git status
                    // before the reload re-reads it — otherwise the TTL would
                    // serve the pre-init values and the branch row would be blank
                    // until it expires.
                    crate::warpui::projects::invalidate_git_cache(&p.path);
                }
                // Reload (SHALLOW — zero git on the UI thread) so `is_loose` is
                // recomputed and the CUBE icon / branch rows appear, then refill
                // branch/diff/dirty badges for the whole tree off-thread.
                self.reload_projects();
                self.rescan_all_git(ctx);
            }
            CraneShellAction::CloseModal => {
                self.modal = None;
                self.find_in_files = None;
                self.tab_switcher = None;
                self.switch_branch = None;
                self.new_workspace = None;
                self.remove_wt_info = None;
            }
            CraneShellAction::OpenHelp => {
                self.modal = Some(Modal::Help);
            }
            CraneShellAction::OpenSettings => {
                // Land on Appearance so the theme picker is the first thing shown
                // (themes moved into Settings now that the gear menu is gone).
                self.settings_section = SettingsSection::Appearance;
                self.modal = Some(Modal::Settings);
            }
            CraneShellAction::QuitConfirmed => {
                // The user approved the quit — flag it so the re-issued terminate
                // sails through the `on_should_terminate_app` guard, then request
                // termination (ForceTerminate: no further confirmation dialogs).
                self.modal = None;
                self.confirmed_quit = true;
                self.save_state(&*ctx);
                ctx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
            }
            CraneShellAction::ConfirmClosePane(id) => {
                self.modal = None;
                self.focused = Some(*id);
                if self.maximized == Some(*id) {
                    self.maximized = None;
                }
                self.close_focused(ctx);
            }
            CraneShellAction::OpenFindInFiles => {
                self.find_in_files = Some(FindInFilesState {
                    query: String::new(),
                    results: Vec::new(),
                    truncated: false,
                    selected: 0,
                });
                self.modal = Some(Modal::FindInFiles);
            }
            CraneShellAction::FindInFilesKey(ks) => {
                self.edit_find_in_files(ks, ctx);
            }
            CraneShellAction::OpenFifMatch { path, line } => {
                self.open_fif_match(path.clone(), *line, ctx);
            }
            CraneShellAction::AdvanceTabSwitcher(backward) => {
                self.advance_tab_switcher(*backward);
            }
            CraneShellAction::TabSwitcherKey(ks) => {
                self.edit_tab_switcher(ks, ctx);
            }
            CraneShellAction::ActivateSwitcherTab { key, path } => {
                self.activate_switcher_tab(*key, path.clone(), ctx);
            }
            CraneShellAction::LspGoto {
                path,
                line,
                character,
            } => {
                self.lsp_start_goto(path.clone(), *line, *character, ctx);
            }
            CraneShellAction::LspGotoAtCursor => {
                if let Some(path) = self.active_editor_path() {
                    if let Some(h) = self.editor_views.get(&path).cloned() {
                        let (line, character) = h.read(ctx, |v, app| v.cursor_line_char(app));
                        self.lsp_start_goto(path, line, character, ctx);
                    }
                }
            }
            CraneShellAction::SettingsGoto(section) => {
                self.settings_section = *section;
            }
            CraneShellAction::FontBaseStep { editor, delta } => {
                if *editor {
                    crate::warpui::fontsize::set_editor(
                        crate::warpui::fontsize::editor() + delta,
                    );
                } else {
                    crate::warpui::fontsize::set_base(crate::warpui::fontsize::base() + delta);
                }
            }
            CraneShellAction::ToggleWordWrapDefault => {
                self.word_wrap_default = !self.word_wrap_default;
                let on = self.word_wrap_default;
                let handles: Vec<_> = self.editor_views.values().cloned().collect();
                for h in handles {
                    h.update(ctx, |v, vctx| v.set_word_wrap(on, vctx));
                }
            }
            CraneShellAction::ToggleTrimOnSave => {
                self.trim_on_save = !self.trim_on_save;
                let on = self.trim_on_save;
                let handles: Vec<_> = self.editor_views.values().cloned().collect();
                for h in handles {
                    h.update(ctx, |v, _| v.set_trim_on_save(on));
                }
            }
            CraneShellAction::SetSyntaxOverride(name) => {
                self.syntax_override = name.clone();
                crate::syntax::set_theme_override(name.clone());
                // Recolor every open buffer with the new theme.
                let handles: Vec<_> = self.editor_views.values().cloned().collect();
                for h in handles {
                    h.update(ctx, |v, vctx| {
                        v.mark_diff_dirty();
                        vctx.notify();
                    });
                }
            }
            CraneShellAction::OpenThemesFolder => {
                let dir = crate::theme::themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&dir).spawn();
            }
            CraneShellAction::OpenUrl(url) => {
                let _ = webbrowser::open(url);
            }
            CraneShellAction::UpdateCheckNow => {
                // A manual check re-surfaces the banner even if it was closed
                // this session (old check.rs `manual_check` semantics), and —
                // unlike the silent routine background check — always gets a
                // visible answer: `manual_update_check` makes the banner show
                // "you're up to date" too when the result is Idle.
                self.update_dismissed_session = None;
                self.manual_update_check = true;
                let wake = self.ui_wake.clone();
                crate::warpui::update::spawn_recheck(move || wake());
            }
            CraneShellAction::UpdateRemindLater => {
                if let Some(v) = crate::warpui::update::latest_available() {
                    self.update_prompts.insert(
                        v.clone(),
                        UpdatePrompt::RemindAt(now_epoch_secs() + UPDATE_REMIND_SECS),
                    );
                    self.update_dismissed_session = Some(v);
                }
            }
            CraneShellAction::UpdateSkipVersion => {
                if let Some(v) = crate::warpui::update::latest_available() {
                    self.update_prompts.insert(v.clone(), UpdatePrompt::Dismissed);
                    self.update_dismissed_session = Some(v);
                }
            }
            CraneShellAction::UpdateDismissSession => {
                self.update_dismissed_session =
                    Some(crate::warpui::update::latest_available().unwrap_or_default());
                // Also closes the manual "you're up to date" banner so it
                // can't reappear on some later unrelated repaint.
                self.manual_update_check = false;
            }
            CraneShellAction::ToggleLsp => {
                self.lsp_enabled = !self.lsp_enabled;
                if self.lsp_enabled {
                    // Turning ON: did_open every live editor file so the matching
                    // server spawns and diagnostics start streaming — the same
                    // path taken at app startup / on file open when enabled.
                    let handles: Vec<(PathBuf, _)> = self
                        .editor_views
                        .iter()
                        .map(|(p, h)| (p.clone(), h.clone()))
                        .collect();
                    for (path, h) in handles {
                        if !self.lsp.is_tracked(&path) {
                            let content = h.read(ctx, |v, app| v.buffer_text(app));
                            self.lsp
                                .did_open(&self.lsp_wake, &path, &content, &self.lsp_configs);
                            let v0 = h.read(ctx, |v, app| v.buffer_version(app));
                            self.lsp_versions.insert(path.clone(), v0);
                        }
                    }
                } else {
                    // Turning OFF: shut down every running language server and
                    // wipe the diagnostics squiggles from all open editors.
                    self.lsp.shutdown_all();
                    self.lsp_versions.clear();
                    self.lsp_diag_sig.clear();
                    self.pending_gotos.clear();
                    let handles: Vec<_> = self.editor_views.values().cloned().collect();
                    for h in handles {
                        h.update(ctx, |v, c| v.set_diagnostics(Vec::new(), c));
                    }
                }
                // Persisted by the unconditional `save_state` at the end of
                // `handle_action`.
            }
            CraneShellAction::ToggleFormatOnSave => {
                self.format_on_save = !self.format_on_save;
                // Persisted by the unconditional `save_state` at the tail of
                // `handle_action`.
            }
            CraneShellAction::StartUpdateDownload => {
                // Hand the updater the shell repaint waker so Downloading /
                // Ready / Failed transitions surface in Settings > About without
                // waiting for an incidental repaint. Idempotent + non-blocking.
                let wake = self.ui_wake.clone();
                crate::warpui::update::start_download(move || wake());
            }
            CraneShellAction::ApplyUpdate(path) => {
                // Swaps the running install for the staged bundle and relaunches
                // (exits this process on success; macOS only).
                crate::warpui::update::apply_and_restart(path);
            }
            CraneShellAction::OpenSwitchBranch => {
                self.open_switch_branch(ctx);
            }
            CraneShellAction::SwitchBranchKey(ks) => {
                self.edit_switch_branch(ks, ctx);
            }
            CraneShellAction::CreateBranchCheckout(name) => {
                self.modal = None;
                self.switch_branch = None;
                let name = name.trim().to_string();
                if !name.is_empty() {
                    if let Some(root) = self.active_cwd.clone() {
                        match crate::warpui::git::create_branch(&root, &name, true) {
                            Ok(()) => {
                                self.sync_worktree_branch_label(&root);
                                self.refresh_panel(ctx);
                                self.invalidate_editor_diffs(&*ctx);
                            }
                            Err(e) => self.commit_error = Some(e),
                        }
                    }
                }
            }
            CraneShellAction::OpenNewWorkspace { pi, branch } => {
                // Close any Switch-Branch modal first (it may have opened this).
                self.switch_branch = None;
                let new_branch = branch.is_none();
                self.new_workspace = Some(NewWorkspaceState {
                    project_idx: *pi,
                    branch: branch.clone().unwrap_or_default(),
                    new_branch,
                    // An existing branch from the picker locks the field —
                    // the only sensible action is checkout-into-new-worktree.
                    branch_locked: branch.is_some(),
                    mode: LocationMode::Global,
                    custom_path: String::new(),
                    path_focused: false,
                    error: None,
                });
                self.modal = Some(Modal::NewWorkspace);
            }
            CraneShellAction::NewWorkspaceKey(ks) => {
                self.edit_new_workspace(ks, ctx);
            }
            CraneShellAction::NewWorkspaceSetMode(mode) => {
                if let Some(st) = self.new_workspace.as_mut() {
                    st.mode = *mode;
                    st.path_focused = *mode == LocationMode::Custom;
                }
            }
            CraneShellAction::NewWorkspaceFocusPath(on) => {
                if let Some(st) = self.new_workspace.as_mut() {
                    st.path_focused = *on;
                }
            }
            CraneShellAction::NewWorkspaceBrowse => {
                let start = self
                    .new_workspace
                    .as_ref()
                    .map(|st| st.custom_path.clone())
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
                let fut = rfd::AsyncFileDialog::new()
                    .set_title("Choose worktree parent folder")
                    .set_directory(start)
                    .pick_folder();
                ctx.spawn(fut, |this, res: Option<rfd::FileHandle>, vctx| {
                    if let (Some(handle), Some(st)) = (res, this.new_workspace.as_mut()) {
                        st.custom_path = handle.path().to_string_lossy().to_string();
                        st.mode = LocationMode::Custom;
                    }
                    vctx.notify();
                });
            }
            CraneShellAction::NewWorkspaceToggleNewBranch => {
                if let Some(st) = self.new_workspace.as_mut() {
                    st.new_branch = !st.new_branch;
                }
            }
            CraneShellAction::NewWorkspaceConfirm => {
                self.confirm_new_workspace(ctx);
            }
            CraneShellAction::Noop => {}
        }
        // Typing-rate actions (every keystroke routes through SendKeys and the
        // modal key-forwarders) must stay CHEAP: skip the per-action pane
        // relayout sweep (typing never changes geometry — notifying every
        // ChildView forced a full multi-pane relayout per key) and defer the
        // save_state disk write (JSON serialize + fs write, plus the 400ms
        // full-scrollback ANSI snapshot) to the 1.5s dirty-flush tick. This
        // was THE felt input latency across terminals and editors.
        // TermNotification joins this set for a different reason than typing
        // latency: it's dispatched from INSIDE the emitting TerminalView's own
        // `spawn_stream_local` closure (view.rs — its wake stream fires while
        // its ViewHandle is still checked out / mid-update). The relayout
        // sweep and `save_state` below both read or update EVERY terminal
        // pane, including that same one, which the framework's view registry
        // still has removed at this point — touching it panics with "circular
        // view reference"/"Circular view update" (crash.log, v0.5.3 onward).
        // `TermBell` already sidesteps this with its own early return just
        // below; TermNotification didn't, which was the actual crash trigger.
        let typing = matches!(
            action,
            CraneShellAction::SendKeys(_)
                | CraneShellAction::FindInFilesKey(_)
                | CraneShellAction::TabSwitcherKey(_)
                | CraneShellAction::SwitchBranchKey(_)
                | CraneShellAction::NewWorkspaceKey(_)
                | CraneShellAction::TermNotification { .. }
        );
        // Keep KEYBOARD focus in sync with the focused pane so it receives
        // keys/mouse (terminal, file, or warp editor view).
        if let Some(id) = self.focused {
            if let Some(h) = self.terminal_at(id) {
                ctx.focus(&h);
            } else if let Some(h) = self.file_at(id) {
                ctx.focus(&h);
            } else if let Some(PaneContent::Editor(h)) = self.panes.get(&id) {
                let h = h.clone();
                ctx.focus(&h);
            }
        }
        // Re-layout the active tab's panes so a CLOSE/SPLIT/DOCK resizes the
        // remaining terminals' grids NOW (SIGWINCH) instead of on the next PTY
        // byte. ChildView caches the child's element tree, so the child view
        // must be notified to re-run its layout at the new pane size. The same
        // pass keeps each terminal's focus-dim in sync (unfocused grids fade).
        if !typing {
            if let Some(tab) = self.active_tab {
                if let Some(node) = self.layouts.get(&tab) {
                    let mut leaves = Vec::new();
                    node.leaves(&mut leaves);
                    let multi = leaves.len() > 1;
                    let focused = self.focused;
                    for id in leaves {
                        if let Some(h) = self.terminal_at(id) {
                            h.update(ctx, |v, vctx| {
                                v.set_dimmed(multi && focused != Some(id));
                                vctx.notify();
                            });
                        } else if let Some(h) = self.file_at(id) {
                            h.update(ctx, |_, vctx| vctx.notify());
                        }
                    }
                }
            }
        }
        // Settle the attention pulse on whatever tab is now active — covers
        // every activation path (click, shortcut, toast-focus) from one choke
        // point instead of threading a clear into each (old egui parity).
        self.clear_active_attention();
        // Persist UI state after every non-typing action so a restart restores
        // the workspace; typing marks the state dirty and the poll tick
        // flushes it within 1.5s (a rename committed via Enter still lands).
        if typing {
            self.state_dirty.set(true);
        } else {
            self.save_state(&*ctx);
            self.state_dirty.set(false);
        }
        // Mark the view dirty so warpui re-renders.
        ctx.notify();
    }
}

/// True when `path` is a git-internal write that means "a ref moved" — a
/// commit / checkout / fetch-with-ref-update — and therefore warrants a badge
/// rescan + worktree poll. Deliberately NARROW: `git status` / `git diff`
/// (exactly what our own badge scans run) refresh and rewrite
/// `.git/worktrees/<name>/index`, so classifying every `/.git/worktrees/`
/// write as a ref event feeds the scan's own index writes back into another
/// scan — an infinite scan→event→scan loop that saturates the UI thread.
/// Under `/.git/worktrees/<name>/` only HEAD, ORIG_HEAD, and `refs/…` count;
/// `index`, `index.lock`, and `COMMIT_EDITMSG` never do. FETCH_HEAD is also
/// deliberately excluded (both levels): a fetch that actually moves a ref
/// writes under `refs/` anyway, and FETCH_HEAD alone changes no badge.
fn git_meta_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    if s.contains("/.git/refs/")
        || s.ends_with("/.git/HEAD")
        || s.ends_with("/.git/packed-refs")
        || s.ends_with("/.git/ORIG_HEAD")
    {
        return true;
    }
    // Linked-worktree private dir: `.git/worktrees/<name>/<rel>` — match on
    // the <rel> tail only.
    if let Some(idx) = s.find("/.git/worktrees/") {
        let after = &s[idx + "/.git/worktrees/".len()..];
        if let Some((_name, rel)) = after.split_once('/') {
            return rel == "HEAD" || rel == "ORIG_HEAD" || rel.starts_with("refs/");
        }
    }
    false
}

#[cfg(test)]
mod git_meta_tests {
    use super::git_meta_path;
    use std::path::Path;

    #[test]
    fn worktree_index_is_not_meta_but_head_and_refs_are() {
        // Scan side-effects must NOT classify as ref writes (loop breaker).
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x/index")));
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x/index.lock")));
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x/COMMIT_EDITMSG")));
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x/FETCH_HEAD")));
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x/logs/HEAD")));
        assert!(!git_meta_path(Path::new("/r/.git/worktrees/x")));

        // Real ref movement in a linked worktree DOES classify.
        assert!(git_meta_path(Path::new("/r/.git/worktrees/x/HEAD")));
        assert!(git_meta_path(Path::new("/r/.git/worktrees/x/ORIG_HEAD")));
        assert!(git_meta_path(Path::new("/r/.git/worktrees/x/refs/bisect/bad")));

        // Top level: refs / HEAD / packed-refs yes; index / FETCH_HEAD no.
        assert!(git_meta_path(Path::new("/r/.git/refs/heads/main")));
        assert!(git_meta_path(Path::new("/r/.git/HEAD")));
        assert!(git_meta_path(Path::new("/r/.git/packed-refs")));
        assert!(git_meta_path(Path::new("/r/.git/ORIG_HEAD")));
        assert!(!git_meta_path(Path::new("/r/.git/index")));
        assert!(!git_meta_path(Path::new("/r/.git/FETCH_HEAD")));
        assert!(!git_meta_path(Path::new("/r/src/main.rs")));
    }
}

#[cfg(test)]
mod title_debounce_tests {
    use super::TitleDebounce;
    use std::time::{Duration, Instant};

    const WINDOW: Duration = Duration::from_secs(3);

    #[test]
    fn first_title_adopts_immediately_then_churn_is_held_and_promoted() {
        let mut d = TitleDebounce::default();
        let t0 = Instant::now();

        // Fresh terminal: the very first title shows at once, no wait.
        assert_eq!(d.observe("zsh", t0, WINDOW), "zsh");

        // A new title churns — it must NOT replace the displayed one yet.
        assert_eq!(d.observe("claude · 1.2k", t0 + Duration::from_millis(100), WINDOW), "zsh");
        // Still churning before the window elapses: keeps showing "zsh", and
        // each distinct title resets the stability clock.
        assert_eq!(d.observe("claude · 1.5k", t0 + Duration::from_millis(600), WINDOW), "zsh");
        assert_eq!(d.observe("claude · 2.0k", t0 + Duration::from_secs(1), WINDOW), "zsh");

        // Once one title holds steady for the full window it gets promoted.
        assert_eq!(d.observe("nvim README.md", t0 + Duration::from_secs(2), WINDOW), "zsh");
        assert_eq!(
            d.observe("nvim README.md", t0 + Duration::from_secs(6), WINDOW),
            "nvim README.md"
        );

        // Re-seeing the already-displayed title is a stable no-op.
        assert_eq!(
            d.observe("nvim README.md", t0 + Duration::from_secs(9), WINDOW),
            "nvim README.md"
        );
    }

    #[test]
    fn stable_tab_ids_keep_entries_apart_across_position_shifts() {
        // The shell keys debounce state by the globally-unique tab id, not the
        // positional (pi, wi, tid) tuple — so when a project reorder/removal
        // shifts pi/wi, each tab keeps ITS OWN entry and never reads another
        // tab's state. Model two tabs (ids 7 and 42) whose tree positions swap.
        let mut map: std::collections::HashMap<usize, TitleDebounce> =
            std::collections::HashMap::new();
        let t0 = Instant::now();

        // Both tabs adopt their first title immediately.
        assert_eq!(map.entry(7).or_default().observe("zsh", t0, WINDOW), "zsh");
        assert_eq!(map.entry(42).or_default().observe("cargo build", t0, WINDOW), "cargo build");

        // Tab 42 starts churning; tab 7 stays put. (Positions may have swapped
        // in the tree — irrelevant: lookups go by id.)
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(map.entry(42).or_default().observe("claude · 3k", t1, WINDOW), "cargo build");
        assert_eq!(map.entry(7).or_default().observe("zsh", t1, WINDOW), "zsh");

        // No cross-talk: tab 7 seeing tab 42's old title is a fresh candidate
        // for tab 7, held back by the window — not instantly shown.
        let t2 = t0 + Duration::from_secs(1);
        assert_eq!(map.entry(7).or_default().observe("cargo build", t2, WINDOW), "zsh");

        // And each promotes independently once its own candidate holds steady.
        let t3 = t0 + Duration::from_secs(5);
        assert_eq!(map.entry(42).or_default().observe("claude · 3k", t3, WINDOW), "claude · 3k");
        assert_eq!(map.entry(7).or_default().observe("cargo build", t3, WINDOW), "cargo build");
    }
}

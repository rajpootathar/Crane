//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    Border, ChildView, ConstrainedBox, Container, CornerRadius, Dismiss, DispatchEventResult,
    Draggable, DraggableState, EventHandler, Expanded, Flex, ParentElement, Radius, Rect, Stack,
    Text,
};
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
    Change { path: String, staged: bool, x: f32, y: f32 },
    /// Files-tab row: Open / Reveal / Copy Path / New File / New Folder / Delete.
    File { path: PathBuf, is_dir: bool, x: f32, y: f32 },
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
    staged: bool,
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

    /// `(all_staged, any_staged)` across the subtree.
    fn staged_state(&self) -> (bool, bool) {
        let mut total = 0usize;
        let mut staged = 0usize;
        let mut any = false;
        fn walk(n: &ChangeDir, total: &mut usize, staged: &mut usize, any: &mut bool) {
            for c in n.dirs.values() {
                walk(c, total, staged, any);
            }
            for f in &n.files {
                *total += 1;
                if f.staged {
                    *staged += 1;
                    *any = true;
                }
            }
        }
        walk(self, &mut total, &mut staged, &mut any);
        (total > 0 && staged == total, any)
    }
}

pub struct CraneShellView {
    ui_font: FamilyId,
    icon_font: FamilyId,
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
    git_log_lines: Vec<String>,
    git_log_ratio: Rc<Cell<f32>>,
    git_log_drag: Rc<Cell<bool>>,
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
    /// Active project context menu, or None when no menu is open.
    context_menu: Option<ProjectContextMenu>,
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
}

#[derive(Clone)]
pub struct TabMeta {
    pub id: usize,
    pub name: String,
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
        let ui_font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .load_system_font("Helvetica Neue")
                .or_else(|_| cache.load_system_font("Menlo"))
                .expect("load ui font")
        });
        let icon_font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .load_family_from_bytes(
                    "phosphor",
                    vec![include_bytes!("assets/Phosphor.ttf").to_vec()],
                )
                .expect("load phosphor")
        });
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
        let projects = crate::warpui::projects::load_projects_extended(
            &init_added, &init_removed, &init_tints,
        );
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
        let mut active_tab = None;
        let mut focused = None;
        let mut selected = (0, 0, usize::MAX);
        let mut next_pane_id: PaneId = 0;
        let mut next_tab_id: usize = 0;
        // UI prefs, restored from warpui-state.json if present.
        let mut show_left = true;
        let mut show_right = true;
        let mut files_tab = true;
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
                            let mono = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
                                cache.load_system_font("Menlo").expect("load Menlo")
                            });
                            for p in &saved_paths {
                                let content = std::fs::read_to_string(p).unwrap_or_default();
                                let pc = p.clone();
                                let h = ctx.add_typed_action_view(move |ctx| {
                                    crate::warpui::editor_view::WarpEditorView::new(
                                        ctx, content, mono, pc,
                                    )
                                });
                                restored_editor_views.insert(p.clone(), h);
                            }
                            let active = saved_active.min(saved_paths.len() - 1);
                            let active_h = restored_editor_views[&saved_paths[active]].clone();
                            panes.insert(pid, PaneContent::Editor(active_h));
                            restored_files_pane = Some(pid);
                            restored_file_paths = saved_paths.clone();
                            restored_active = active;
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
                                PaneContent::Terminal(Self::spawn_terminal(ctx, wpath.clone())),
                            );
                        }
                        drag_states.insert(pid, DraggableState::default());
                    }
                    layouts.insert((*pi, *wi, stab.id), stab.layout.to_node());
                    metas.push(TabMeta {
                        id: stab.id,
                        name: stab.name.clone(),
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
                    TabMeta { id, name }
                })
                .collect();
            let first_id = metas[0].id;
            worktree_tabs.insert((0, 0), metas);
            let key = (0, 0, first_id);
            let pane = next_pane_id;
            next_pane_id += 1;
            panes.insert(pane, PaneContent::Terminal(Self::spawn_terminal(ctx, path)));
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
                this.refresh_panel();
                vctx.notify();
            },
            |_this, _vctx| {},
        );
        Self {
            ui_font,
            icon_font,
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
            branch: active_cwd
                .as_deref()
                .map(crate::warpui::git::current_branch)
                .unwrap_or_default(),
            commit_msg: String::new(),
            commit_focused: false,
            show_git_log: false,
            git_log_lines: Vec::new(),
            git_log_ratio: Rc::new(Cell::new(0.7)),
            git_log_drag: Rc::new(Cell::new(false)),
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
            context_menu: None,
            collapsed_groups: HashSet::new(),
            collapsed_change_dirs: HashSet::new(),
            commit_error: None,
            ahead_behind: None,
            file_status: HashMap::new(),
            dirty_dirs: HashSet::new(),
            git_op: Arc::new(Mutex::new(crate::warpui::git::OpStatus::default())),
            git_wake,
            _git_repaint: git_repaint,
            row_menu: None,
            pending_new_entry: None,
            pending_delete: None,
            branch_picker: None,
            branch_list: Vec::new(),
        }
    }

    /// Spawn a new persistent terminal view rooted at `path`. Each gets its own
    /// PTY + repaint waker; it is never respawned (history retained).
    fn spawn_terminal(
        ctx: &mut ViewContext<Self>,
        path: PathBuf,
    ) -> ViewHandle<TerminalView> {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: crate::warpui::controller::Wake = std::sync::Arc::new(move || {
            let _ = tx.try_send(());
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

    /// A 2px-wide accent vertical bar pinned to the left edge of a row, inset
    /// ~3px vertically. Layered ON TOP of the `row_active()` bg for the active /
    /// selected branch of a nav row — mirrors old egui `draw_row`'s `active_bar`
    /// (`Rect x+4, y+3, w=2, h=row_h-6`, accent).
    fn active_bar(&self, row_h: f32) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
                .with_width(2.0)
                .with_height((row_h - 6.0).max(0.0))
                .finish(),
        )
        .with_padding_left(4.0)
        .with_padding_top(3.0)
        .finish()
    }

    /// A single row inside the context menu (icon + label). Dispatches
    /// CloseContextMenu then the real `action` when clicked.
    fn menu_item(&self, glyph: &str, label: &str, action: CraneShellAction) -> Box<dyn Element> {
        let row = Flex::row()
            .with_child(
                Container::new(self.icon(glyph, 12.0, theme::text_muted()))
                    .with_padding_right(8.0)
                    .finish(),
            )
            .with_child(
                Text::new(label.to_string(), self.ui_font, 12.0)
                    .with_color(theme::text())
                    .finish(),
            )
            .finish();
        let content = Container::new(row)
            .with_padding_left(10.0)
            .with_padding_right(20.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish();
        EventHandler::new(content)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
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

    /// Build the project context menu overlay anchored at the stored click position.
    fn project_context_menu(&self, pm: &ProjectContextMenu) -> Box<dyn Element> {
        let pi = pm.project_idx;
        let is_loose = self.projects.get(pi).map(|p| p.is_loose).unwrap_or(false);
        let mut items = Flex::column();

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

        // Tint palette — 8 colored CUBE swatches in a single row.
        const PALETTE: [(&str, [u8; 3]); 8] = [
            ("Red",    [239,  83,  80]),
            ("Orange", [255, 152,   0]),
            ("Yellow", [255, 202,  40]),
            ("Green",  [102, 187, 106]),
            ("Teal",   [ 38, 166, 154]),
            ("Blue",   [ 66, 165, 245]),
            ("Purple", [171,  71, 188]),
            ("Pink",   [236,  64, 122]),
        ];
        let icon_font = self.icon_font;
        let mut swatches = Flex::row();
        for (_name, rgb) in &PALETTE {
            let color = ColorU::new(rgb[0], rgb[1], rgb[2], 255);
            let rgb_copy = *rgb;
            let swatch = EventHandler::new(
                Container::new(
                    Text::new(icons::CUBE.to_string(), icon_font, 14.0)
                        .with_color(color)
                        .finish(),
                )
                .with_uniform_padding(4.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                ctx.dispatch_typed_action(CraneShellAction::SetProjectTint(pi, Some(rgb_copy)));
                DispatchEventResult::StopPropagation
            })
            .finish();
            swatches = swatches.with_child(swatch);
        }
        let palette_row = Container::new(swatches.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(2.0)
            .finish();
        items = items.with_child(palette_row);

        items = items.with_child(self.menu_item(
            icons::ARROW_COUNTER_CLOCKWISE,
            "Default color",
            CraneShellAction::SetProjectTint(pi, None),
        ));
        items = items.with_child(self.menu_separator());

        items = items.with_child(self.menu_item(
            icons::TRASH,
            "Remove Project",
            CraneShellAction::RemoveProject(pi),
        ));

        let menu_box = ConstrainedBox::new(
            Container::new(items.finish())
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
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

        Box::new(
            Dismiss::new(positioned)
                .prevent_interaction_with_other_elements()
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                }),
        )
    }

    /// Wrap a built menu column in the standard 220px popover chrome +
    /// dismiss-on-outside-click overlay, positioned at (x, y).
    fn menu_popover(&self, items: Box<dyn Element>, x: f32, y: f32) -> Box<dyn Element> {
        let menu_box = ConstrainedBox::new(
            Container::new(items)
                .with_background_color(theme::surface())
                .with_border(Border::all(1.0).with_border_color(theme::border()))
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        )
        .with_width(220.0)
        .finish();
        let positioned = Container::new(menu_box)
            .with_padding_top(y)
            .with_padding_left(x)
            .finish();
        Box::new(
            Dismiss::new(positioned)
                .prevent_interaction_with_other_elements()
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                }),
        )
    }

    /// Right-Panel row context menu (Changes row or Files row).
    fn row_menu_overlay(&self, menu: &RowMenu) -> Box<dyn Element> {
        match menu {
            RowMenu::Change { path, staged, x, y } => {
                let mut items = Flex::column();
                if !*staged {
                    items = items.with_child(self.menu_item(
                        icons::PLUS,
                        "Stage",
                        CraneShellAction::StagePaths(vec![path.clone()]),
                    ));
                } else {
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
                    CraneShellAction::OpenFileAt(abs),
                ));
                items = items.with_child(self.menu_separator());
                items = items.with_child(self.menu_item(
                    icons::COPY,
                    "Copy Path",
                    CraneShellAction::CopyPathStr(path.clone()),
                ));
                self.menu_popover(items.finish(), *x, *y)
            }
            RowMenu::File { path, is_dir, x, y } => {
                let mut items = Flex::column();
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
                items = items.with_child(self.menu_item(
                    icons::TRASH,
                    "Delete",
                    CraneShellAction::RequestDelete(path.clone()),
                ));
                self.menu_popover(items.finish(), *x, *y)
            }
        }
    }

    /// The branch-picker overlay: a scrollable list of local + remote branches;
    /// clicking one checks it out. (Simple list — no fuzzy filter input yet.)
    fn branch_picker_overlay(&self, x: f32, y: f32) -> Box<dyn Element> {
        let mut items = Flex::column();
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
            let row = Flex::row()
                .with_child(
                    Container::new(self.icon(glyph, 12.0, color))
                        .with_padding_right(8.0)
                        .finish(),
                )
                .with_child(
                    Text::new(b.clone(), self.ui_font, 12.0).with_color(color).finish(),
                )
                .finish();
            let item = EventHandler::new(
                Container::new(row)
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(5.0)
                    .with_padding_bottom(5.0)
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::CloseContextMenu);
                ctx.dispatch_typed_action(CraneShellAction::CheckoutBranch(branch.clone()));
                DispatchEventResult::StopPropagation
            })
            .finish();
            items = items.with_child(item);
        }
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
                Text::new("Delete".to_string(), self.ui_font, 12.0)
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
                        "This cannot be undone.".to_string(),
                        self.ui_font,
                        11.0,
                    )
                    .with_color(theme::text_muted())
                    .finish(),
                )
                .with_child(Self::spacer(14.0))
                .with_child(
                    Flex::row()
                        .with_child(Expanded::new(1.0, Rect::new().finish()).finish())
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

    /// Reload the project list from session.json + the current overlay
    /// (added / removed / tints). Call after mutating any of those three fields.
    fn reload_projects(&mut self) {
        self.projects = crate::warpui::projects::load_projects_extended(
            &self.added_projects,
            &self.removed_project_paths,
            &self.project_tints,
        );
    }

    fn refresh_panel(&mut self) {
        let root = self.active_cwd.clone();
        self.branch = root
            .as_deref()
            .map(crate::warpui::git::current_branch)
            .unwrap_or_default();
        let Some(root) = root else {
            self.file_rows.clear();
            self.changes.clear();
            self.file_status.clear();
            self.dirty_dirs.clear();
            self.ahead_behind = None;
            return;
        };
        // Working-tree changes + upstream ahead/behind, always (the changes feed
        // BOTH the Changes tree and the Files-tab per-row status colours).
        self.changes = crate::warpui::git::changes(&root);
        self.ahead_behind = crate::warpui::git::ahead_behind(&root);
        // rel-path → status char, plus the set of directories that contain a
        // changed descendant (for folder-row tinting). Port of old egui
        // `git_status_map` in explorer.rs.
        self.file_status.clear();
        self.dirty_dirs.clear();
        for c in &self.changes {
            let rel = c.path.trim_end_matches('/').to_string();
            let ch = c.status.chars().next().unwrap_or(' ');
            self.file_status.insert(rel.clone(), ch);
            // Mark every ancestor directory dirty.
            let mut cur = std::path::Path::new(&rel);
            while let Some(parent) = cur.parent() {
                if parent.as_os_str().is_empty() {
                    break;
                }
                self.dirty_dirs.insert(parent.to_string_lossy().to_string());
                cur = parent;
            }
        }
        if self.files_tab {
            let skip = self.nested_repo_skip_set(&root);
            let mut rows = file_tree::build_rows_with_skip(&root, &self.expanded_dirs, &skip);
            for r in &mut rows {
                let rel = r
                    .path
                    .strip_prefix(&root)
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

    /// A bare icon button — Container records the hit + sizes to content.
    fn icon_button(&self, glyph: &str, action: CraneShellAction) -> Box<dyn Element> {
        let content = Container::new(self.icon(glyph, 15.0, theme::text_muted()))
            .with_background_color(theme::topbar_bg())
            .with_uniform_padding(5.0)
            .finish();
        EventHandler::new(content)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// A labelled pill button (icon + text on a surface pill).
    fn pill_button(&self, glyph: &str, label: &str, action: CraneShellAction) -> Box<dyn Element> {
        let inner = Flex::row()
            .with_child(
                Container::new(self.icon(glyph, 12.0, theme::text_muted()))
                    .with_padding_right(5.0)
                    .finish(),
            )
            .with_child(
                Text::new(label.to_string(), self.ui_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .finish();
        let content = Container::new(inner)
            .with_background_color(theme::surface())
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish();
        EventHandler::new(content)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
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
        selected: bool,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        let row_h = size + 8.0;
        let mut bg = Rect::new();
        if selected {
            bg = bg.with_background_color(theme::row_active());
        }
        let bg_layer = ConstrainedBox::new(bg.finish()).with_height(row_h).finish();

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
        let mut row = Stack::new().with_child(bg_layer);
        if selected {
            row = row.with_child(self.active_bar(row_h));
        }
        let row = row.with_child(label).with_child(hit_layer).finish();

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
        selected: bool,
        diff_stat: (u32, u32),
        dirty: bool,
        indent: f32,
        action: CraneShellAction,
    ) -> Box<dyn Element> {
        let size = 12.0_f32;
        let row_h = size + 8.0;
        let mut bg = Rect::new();
        if selected {
            bg = bg.with_background_color(theme::row_active());
        }
        let bg_layer = ConstrainedBox::new(bg.finish()).with_height(row_h).finish();

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
                    Text::new(name.to_string(), self.ui_font, size)
                        .with_color(label_color)
                        .finish(),
                )
                .finish(),
            );

        // +N / -M badges appended at right when there are line changes.
        let (added, deleted) = diff_stat;
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

        let label = Container::new(row_inner.finish())
            .with_padding_left(indent)
            .with_padding_top(4.0)
            .finish();
        let hit_layer = ConstrainedBox::new(Rect::new().finish())
            .with_height(row_h)
            .finish();
        let mut stack = Stack::new().with_child(bg_layer);
        if selected {
            stack = stack.with_child(self.active_bar(row_h));
        }
        let stack = stack.with_child(label).with_child(hit_layer).finish();
        EventHandler::new(stack)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// A tab row with a trailing close button. The close button's EventHandler returns
    /// `StopPropagation` so the outer select handler never fires when close is clicked.
    fn tab_closeable_row(
        &self,
        icon_color: ColorU,
        name: &str,
        selected: bool,
        indent: f32,
        select_action: CraneShellAction,
        close_action: CraneShellAction,
    ) -> Box<dyn Element> {
        let size = 11.0_f32;
        let row_h = size + 8.0;
        let mut bg = Rect::new();
        if selected {
            bg = bg.with_background_color(theme::row_active());
        }
        let bg_layer = ConstrainedBox::new(bg.finish()).with_height(row_h).finish();

        // Label: icon + text (no caret for tab leaves).
        let label_content = Flex::row()
            .with_child(
                Container::new(self.icon(icons::TERMINAL_WINDOW, size, icon_color))
                    .with_padding_right(6.0)
                    .finish(),
            )
            .with_child(
                Text::new(name.to_string(), self.ui_font, size)
                    .with_color(icon_color)
                    .finish(),
            )
            .finish();
        let label = Container::new(label_content)
            .with_padding_left(indent)
            .with_padding_top(4.0)
            .finish();

        // Close button — inner EventHandler stops propagation so select doesn't fire.
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

        // Compose: Expanded(label) + close button, layered over background.
        let row_content = Flex::row()
            .with_child(Expanded::new(1.0, label).finish())
            .with_child(close_btn)
            .finish();
        let mut stack = Stack::new().with_child(bg_layer);
        if selected {
            stack = stack.with_child(self.active_bar(row_h));
        }
        let stack = stack.with_child(row_content).finish();
        EventHandler::new(stack)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(select_action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
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

    fn divider(&self) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
            .with_width(1.0)
            .finish()
    }

    fn left_sidebar(&self) -> Box<dyn Element> {
        // Header row: just the "PROJECTS" label. The Add Project affordance is a
        // prominent accent pill pinned at the bottom of the panel (below).
        let header_row = Container::new(
            Text::new("PROJECTS", self.ui_font, 11.0)
                .with_color(theme::text_header())
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish();

        // Real project tree loaded from ~/.crane/session.json: the user's
        // actual projects -> worktrees (branches) -> tabs.
        let mut col = Flex::column();
        if self.projects.is_empty() {
            col = col.with_child(self.tree_row(
                "No projects. Click + to add one.",
                12.0,
                theme::text_muted(),
                12.0,
            ));
        }
        let sel = self.selected;
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
                col = col.with_child(self.nav_row(
                    Some(!group_collapsed),
                    folder_glyph,
                    theme::text(),
                    &group_label,
                    13.0,
                    theme::text(),
                    10.0,
                    false,
                    CraneShellAction::ToggleGroup(gp),
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
            let psel = sel == (pi, usize::MAX, usize::MAX);
            let tint = self.project_color_for(pi);
            // Feature 3: when the user has set an explicit tint, also apply it to the
            // project name text (not just the CUBE icon). Fall back to the normal
            // text color when no explicit tint is set, mirroring old egui projects.rs.
            let has_explicit_tint = self.projects.get(pi).and_then(|p| p.tint).is_some();
            let pcol = if psel {
                theme::text_hover()
            } else if has_explicit_tint {
                tint
            } else {
                theme::text()
            };
            // Loose projects (non-git folders) use a FOLDER icon; git projects use CUBE.
            let project_icon = if p.is_loose { icons::FOLDER } else { icons::CUBE };
            let base_row = self.nav_row(
                Some(p_expanded),
                project_icon,
                tint,
                &p.name,
                13.0,
                pcol,
                10.0 + group_offset,
                psel,
                CraneShellAction::ToggleProject(pi),
            );
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
            col = col.with_child(project_row);
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
                            let tsel = sel == tkey;
                            let tcol = if tsel {
                                theme::text_hover()
                            } else {
                                theme::text_muted()
                            };
                            col = col.with_child(self.tab_closeable_row(
                                tcol,
                                &t.name,
                                tsel,
                                24.0 + group_offset,
                                CraneShellAction::Select {
                                    sel: tkey,
                                    path: PathBuf::from(&w.path),
                                },
                                CraneShellAction::CloseTab(tkey),
                            ));
                        }
                    }
                    col = col.with_child(self.nav_row(
                        None,
                        icons::PLUS,
                        theme::text_muted(),
                        "New tab",
                        11.0,
                        theme::text_muted(),
                        24.0 + group_offset,
                        false,
                        CraneShellAction::NewTabIn(pi, wi),
                    ));
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
                let wsel = sel == (pi, wi, usize::MAX) || w_active;
                // Tint priority: explicit user tint wins over active-branch accent so
                // a user-tinted active worktree shows its tint, not the accent, on the icon.
                let wcol = if wsel {
                    theme::accent()
                } else {
                    theme::text_muted()
                };
                // Feature 1: pass the worktree's cached diff-stat to the row builder so
                // it renders the +N -M badge at the right side of the branch row.
                col = col.with_child(self.worktree_nav_row(
                    w_expanded,
                    &w.name,
                    wcol,
                    wcol,
                    wsel,
                    w.diff_stat,
                    w.dirty,
                    24.0 + group_offset,
                    CraneShellAction::ToggleWorktree(pi, wi),
                ));
                if !w_expanded {
                    continue;
                }
                // Tabs from the LIVE model (worktree_tabs), keyed by stable id.
                if let Some(tabs) = self.worktree_tabs.get(&(pi, wi)) {
                    for t in tabs {
                        let tkey = (pi, wi, t.id);
                        let tsel = sel == tkey;
                        let tcol = if tsel { theme::text_hover() } else { theme::text_muted() };
                        // Feature 4: each tab row has a trailing close button.
                        // The close button's EventHandler returns StopPropagation so
                        // clicking it does not also trigger the row's select action.
                        col = col.with_child(self.tab_closeable_row(
                            tcol,
                            &t.name,
                            tsel,
                            42.0 + group_offset,
                            CraneShellAction::Select {
                                sel: tkey,
                                path: PathBuf::from(&w.path),
                            },
                            CraneShellAction::CloseTab(tkey),
                        ));
                    }
                }
                // "+ New tab" row for this worktree.
                col = col.with_child(self.nav_row(
                    None,
                    icons::PLUS,
                    theme::text_muted(),
                    "New tab",
                    11.0,
                    theme::text_muted(),
                    42.0 + group_offset,
                    false,
                    CraneShellAction::NewTabIn(pi, wi),
                ));
            }
        }
        // Prominent "Add Project" pill, pinned at the bottom: accent border +
        // accent icon on a surface bg so it stands out from the dark sidebar.
        let add_inner = Flex::row()
            .with_child(self.icon(icons::FOLDER_PLUS, 13.0, theme::accent()))
            .with_child(
                Container::new(
                    Text::new("Add Project", self.ui_font, 12.0)
                        .with_color(theme::text())
                        .finish(),
                )
                .with_padding_left(8.0)
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Container::new(Text::new("", self.ui_font, 12.0).finish()).finish(),
                )
                .finish(),
            )
            .finish();
        let add_pill = EventHandler::new(
            Container::new(
                Container::new(add_inner)
                    .with_background_color(theme::surface())
                    .with_border(Border::all(1.0).with_border_color(theme::accent()))
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(8.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(CraneShellAction::AddProject);
            DispatchEventResult::StopPropagation
        })
        .finish();

        // Header, then the project list (fills), then the pinned Add Project pill.
        let outer = Flex::column()
            .with_child(header_row)
            .with_child(Expanded::new(1.0, col.finish()).finish())
            .with_child(add_pill)
            .finish();
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), outer)
    }

    fn tab_label(&self, text: &str, active: bool, action: CraneShellAction) -> Box<dyn Element> {
        let color = if active { theme::text_hover() } else { theme::text_muted() };
        let content = Container::new(
            Text::new(text.to_string(), self.ui_font, 12.0)
                .with_color(color)
                .finish(),
        )
        .with_background_color(theme::sidebar_bg())
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .finish();
        EventHandler::new(content)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
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
        let mut bg = Rect::new();
        if is_sel {
            bg = bg.with_background_color(theme::row_active());
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
        EventHandler::new(
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
        .finish()
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
            .with_child(Self::spacer(12.0))
            .with_child(self.tab_label(
                "Files",
                self.files_tab || loose,
                CraneShellAction::SetTab { files: true },
            ))
            .finish();
        let tabs = Container::new(tabs)
            .with_padding_left(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(6.0)
            .finish();

        let mut col = Flex::column().with_child(tabs);
        if show_changes {
            col = col.with_child(self.changes_header());
            if self.changes.is_empty() {
                col = col.with_child(self.tree_row(
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
                    col = col.with_child(r);
                }
            }
            col = col.with_child(self.commit_box());
        } else {
            if let Some(p) = &self.pending_new_entry {
                col = col.with_child(self.pending_entry_row(p));
            }
            if self.file_rows.is_empty() {
                col = col.with_child(self.tree_row("(empty)", 12.0, theme::text_muted(), 12.0));
            }
            for r in &self.file_rows {
                col = col.with_child(self.file_row(r));
            }
        }
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), col.finish())
    }

    /// The "Changes" tab chip. When the active Project is loose it renders greyed
    /// and inert (dispatches Noop) so it can't be selected.
    fn changes_tab_label(&self, active: bool, loose: bool) -> Box<dyn Element> {
        if loose {
            return Container::new(
                Text::new("Changes".to_string(), self.ui_font, 12.0)
                    .with_color(theme::pane_dim())
                    .finish(),
            )
            .with_background_color(theme::sidebar_bg())
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish();
        }
        self.tab_label("Changes", active, CraneShellAction::SetTab { files: false })
    }

    /// Branch + ahead/behind + Push/Pull/Fetch header at the top of the Changes
    /// area. Port of old egui `render_changes` top toolbar.
    fn changes_header(&self) -> Box<dyn Element> {
        let op = self.git_op.lock().clone();
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
            node.files.push(ChangeFile {
                name: (*file).to_string(),
                path: c.path.clone(),
                staged: c.staged,
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
        let (marker, marker_color) = if f.staged {
            (icons::MINUS, theme::success())
        } else {
            (icons::PLUS, theme::text_muted())
        };
        let stage_action = if f.staged {
            CraneShellAction::UnstagePaths(vec![f.path.clone()])
        } else {
            CraneShellAction::StagePaths(vec![f.path.clone()])
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
        EventHandler::new(row)
            .on_right_mouse_down(move |ctx, _app, pos| {
                ctx.dispatch_typed_action(CraneShellAction::ShowChangeMenu {
                    path: menu_path.clone(),
                    staged,
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
        let op = self.git_op.lock().clone();
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
    fn breadcrumb(&self) -> String {
        let (pi, wi, ti) = self.selected;
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = self.projects.get(pi) {
            parts.push(p.name.clone());
            if let Some(w) = p.worktrees.get(wi) {
                parts.push(w.name.clone());
                if ti != usize::MAX {
                    if let Some(t) = w.tabs.get(ti) {
                        parts.push(t.clone());
                    }
                }
            }
        }
        if parts.is_empty() {
            "Crane".to_string()
        } else {
            parts.join("  /  ")
        }
    }

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
        let crumb = Container::new(
            Text::new(self.breadcrumb(), self.ui_font, 12.0)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_padding_left(6.0)
        .with_padding_top(9.0)
        .finish();

        // Theme-cycle button: shows the active theme name; click advances to
        // the next theme in alphabetical order from load_all().
        let current_name = crate::theme::current().name;
        let next_theme = {
            let all = crate::theme::load_all();
            let pos = all.iter().position(|t| t.name == current_name);
            let next_pos = pos.map(|p| (p + 1) % all.len()).unwrap_or(0);
            all.into_iter().nth(next_pos).map(|t| t.name).unwrap_or_default()
        };
        let theme_btn = self.pill_button(
            icons::PAINT_BRUSH,
            &current_name,
            CraneShellAction::SetTheme(next_theme),
        );

        let row = Flex::row()
            .with_child(Self::spacer(80.0)) // macOS traffic-light inset
            .with_child(self.icon_button(icons::SIDEBAR, CraneShellAction::ToggleLeft))
            .with_child(crumb)
            .with_child(Expanded::new(1.0, Rect::new().finish()).finish())
            .with_child(self.pill_button(
                icons::TERMINAL_WINDOW,
                "Terminal",
                CraneShellAction::SplitFocused(Dir::Horizontal),
            ))
            .with_child(Self::spacer(6.0))
            .with_child(self.pill_button(icons::GLOBE, "Browser", CraneShellAction::OpenBrowser))
            .with_child(Self::spacer(6.0))
            .with_child(theme_btn)
            .with_child(Self::spacer(8.0))
            .with_child(self.icon_button(icons::GIT_BRANCH, CraneShellAction::OpenGitLog))
            .with_child(self.icon_button(icons::SIDEBAR, CraneShellAction::ToggleRight))
            .with_child(Self::spacer(8.0))
            .finish();
        ConstrainedBox::new(self.panel(theme::topbar_bg(), row))
            .with_height(theme::TOPBAR_H)
            .finish()
    }

    fn status_bar(&self) -> Box<dyn Element> {
        let label = if self.branch.is_empty() {
            "ready".to_string()
        } else {
            format!("{}  -  ready", self.branch)
        };
        let mut row = Flex::row().with_child(
            Container::new(self.icon(icons::GIT_BRANCH, 11.0, theme::text_muted()))
                .with_padding_left(10.0)
                .with_padding_right(5.0)
                .with_padding_top(7.0)
                .finish(),
        );
        row = row.with_child(
            Container::new(
                Text::new(label, self.ui_font, 11.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_padding_top(7.0)
            .finish(),
        );
        let content = row.finish();
        ConstrainedBox::new(self.panel(theme::topbar_bg(), content))
            .with_height(theme::STATUS_H)
            .finish()
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
            Some(PaneContent::Markdown(h)) => ChildView::new(h).finish(),
            Some(PaneContent::Diff(h)) => ChildView::new(h).finish(),
            None => Rect::new().with_background_color(theme::bg()).finish(),
        };
        // Click anywhere inside the pane body focuses it. `with_always_handle` so
        // it fires even when the child (e.g. the editor) consumes the click to
        // place its caret — otherwise clicking into the file wouldn't focus it.
        let body = EventHandler::new(inner)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FocusPane(id));
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

        let content = Flex::column()
            .with_child(header)
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

    /// Pane header: title (click to focus) + expand-to-full + close.
    fn pane_header(&self, id: PaneId, app: &AppContext) -> Box<dyn Element> {
        const H: f32 = 26.0;
        let focused = self.focused == Some(id);
        let bg = if focused { theme::surface() } else { theme::topbar_bg() };
        let fg = if focused { theme::text() } else { theme::text_muted() };
        let is_file_pane = self.files_pane == Some(id);

        // For a File pane the header IS the file tab strip (shell-driven, so
        // clicks route here). Other panes show a simple "Terminal" title.
        let title: Box<dyn Element> = if is_file_pane {
            let mut strip = Flex::row();
            for (i, path) in self.file_pane_paths.iter().enumerate() {
                let active = i == self.file_pane_active;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let tbg = if active { theme::surface() } else { theme::topbar_bg() };
                let tfg = if active { theme::text() } else { theme::text_muted() };
                // Unsaved indicator: a filled dot before the name when dirty.
                let dirty = self
                    .editor_views
                    .get(path)
                    .map(|h| h.as_ref(app).is_dirty(app))
                    .unwrap_or(false);
                let label = if dirty { format!("{name}  ") } else { name };
                let mut chip_row = Flex::row();
                if dirty {
                    chip_row = chip_row.with_child(
                        Container::new(self.icon(icons::CIRCLE, 8.0, theme::accent()))
                            .with_padding_left(8.0)
                            .with_padding_top(8.0)
                            .finish(),
                    );
                }
                let chip = EventHandler::new(
                    Container::new(
                        chip_row
                            .with_child(
                                Text::new(label, self.ui_font, 11.0).with_color(tfg).finish(),
                            )
                            .finish(),
                    )
                    .with_background_color(tbg)
                    .with_padding_left(if dirty { 2.0 } else { 10.0 })
                    .with_padding_right(4.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(CraneShellAction::FileTabSelect(i));
                    DispatchEventResult::StopPropagation
                })
                .finish();
                let close = EventHandler::new(
                    Container::new(self.icon(icons::X, 10.0, theme::text_muted()))
                        .with_background_color(tbg)
                        .with_padding_right(8.0)
                        .with_padding_top(6.0)
                        .with_padding_bottom(6.0)
                        .finish(),
                )
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(CraneShellAction::FileTabClose(i));
                    DispatchEventResult::StopPropagation
                })
                .finish();
                strip = strip.with_child(Flex::row().with_child(chip).with_child(close).finish());
            }
            strip.finish()
        } else {
            // Title + icon reflect the pane's content (Terminal is the default;
            // Welcome / Markdown / Diff panes name themselves).
            let (glyph, label): (&'static str, String) = match self.panes.get(&id) {
                Some(PaneContent::Welcome(_)) => (icons::CUBE, "Welcome".to_string()),
                Some(PaneContent::Markdown(h)) => {
                    (icons::FILE_TEXT, h.as_ref(app).title().to_string())
                }
                Some(PaneContent::Diff(h)) => {
                    (icons::GIT_DIFF, format!("Diff: {}", h.as_ref(app).title()))
                }
                _ => (icons::TERMINAL_WINDOW, "Terminal".to_string()),
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
                .with_padding_top(6.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FocusPane(id));
                DispatchEventResult::StopPropagation
            })
            .finish()
        };

        // The Expanded title fills the row, pushing these to the right edge.
        let buttons = Flex::row()
            .with_child(self.icon_button(icons::ARROWS_OUT, CraneShellAction::ToggleMaximize(id)))
            .with_child(self.icon_button(icons::X, CraneShellAction::ClosePane(id)))
            .finish();

        let row = Flex::row()
            .with_child(Expanded::new(1.0, title).finish())
            .with_child(buttons)
            .finish();
        ConstrainedBox::new(
            Stack::new()
                .with_child(Rect::new().with_background_color(bg).finish())
                .with_child(row)
                .finish(),
        )
        .with_height(H)
        .finish()
    }

    /// Spawn a new persistent terminal pane rooted at `path`; returns its id.
    fn new_pane(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let handle = Self::spawn_terminal(ctx, path);
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
            // Reuse the live files_pane (swap content) if it still holds a
            // document pane; else split a fresh pane on the RIGHT at 0.35.
            if let Some(fp) = self.files_pane {
                if matches!(
                    self.panes.get(&fp),
                    Some(PaneContent::Editor(_))
                        | Some(PaneContent::File(_))
                        | Some(PaneContent::Markdown(_))
                ) {
                    self.panes.insert(fp, PaneContent::Markdown(handle));
                    self.focused = Some(fp);
                    return;
                }
                self.files_pane = None; // pane was closed
            }
            self.files_pane = self.split_with_at(PaneContent::Markdown(handle), false, 0.35);
            return;
        }
        // Build the editor for this file once; reuse it on later opens/switches
        // so each tab keeps its own cursor / scroll / unsaved edits.
        let handle = if let Some(h) = self.editor_views.get(&path) {
            h.clone()
        } else {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let mono = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
                cache.load_system_font("Menlo").expect("load Menlo")
            });
            let p = path.clone();
            let h = ctx.add_typed_action_view(move |ctx| {
                crate::warpui::editor_view::WarpEditorView::new(ctx, content, mono, p)
            });
            self.editor_views.insert(path.clone(), h.clone());
            h
        };
        // Existing editor pane still alive? Swap its content to the active file.
        if let Some(fp) = self.files_pane {
            if matches!(
                self.panes.get(&fp),
                Some(PaneContent::Editor(_))
                    | Some(PaneContent::File(_))
                    | Some(PaneContent::Markdown(_))
            ) {
                self.panes.insert(fp, PaneContent::Editor(handle));
                self.focused = Some(fp);
                return;
            }
            self.files_pane = None; // pane was closed
            self.file_pane_paths = vec![path.clone()];
            self.file_pane_active = 0;
        }
        // First open: File pane goes on the RIGHT and takes ~65% width (the
        // existing pane keeps 35% as the first child). Full height by default;
        // the user can drag the splitter to resize. Backed by Warp's REAL editor.
        self.files_pane = self.split_with_at(PaneContent::Editor(handle), false, 0.35);
    }

    /// The warp editor view handle for a pane, if it is an Editor pane.
    fn editor_at(&self, id: PaneId) -> Option<ViewHandle<crate::warpui::editor_view::WarpEditorView>> {
        match self.panes.get(&id) {
            Some(PaneContent::Editor(h)) => Some(h.clone()),
            _ => None,
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

    /// Toggle the Git Log bottom dock for the active worktree.
    fn toggle_gitlog(&mut self) {
        self.show_git_log = !self.show_git_log;
        if self.show_git_log {
            self.git_log_lines = self
                .active_cwd
                .as_deref()
                .map(crate::warpui::git::log)
                .unwrap_or_else(|| vec!["<no active workspace>".to_string()]);
        }
    }

    /// The Git Log dock body — a header row (title + close) over the log lines.
    fn git_log_dock(&self) -> Box<dyn Element> {
        let header = ConstrainedBox::new(
            Stack::new()
                .with_child(Rect::new().with_background_color(theme::topbar_bg()).finish())
                .with_child(
                    Flex::row()
                        .with_child(
                            Container::new(
                                self.icon(icons::GIT_BRANCH, 12.0, theme::text_muted()),
                            )
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
                            .finish(),
                        )
                        .with_child(Expanded::new(1.0, Rect::new().finish()).finish())
                        .with_child(self.icon_button(icons::X, CraneShellAction::OpenGitLog))
                        .finish(),
                )
                .finish(),
        )
        .with_height(26.0)
        .finish();
        let mut col = Flex::column();
        for line in self.git_log_lines.iter().take(500) {
            col = col.with_child(
                Container::new(
                    Text::new(line.clone(), self.ui_font, 11.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_padding_left(10.0)
                .finish(),
            );
        }
        Flex::column()
            .with_child(header)
            .with_child(Expanded::new(1.0, self.panel(theme::bg(), col.finish())).finish())
            .finish()
    }

    /// Open a placeholder Browser pane (WKWebView embed pending).
    fn open_browser(&mut self, ctx: &mut ViewContext<Self>) {
        let lines = vec![
            "Browser pane".to_string(),
            String::new(),
            "(embedded WKWebView pending — old Crane's browser_view)".to_string(),
        ];
        let handle =
            ctx.add_view(move |ctx| FileView::from_text(ctx, "Browser".to_string(), lines));
        self.split_with(PaneContent::File(handle));
    }

    /// Open a read-only unified Diff pane (HEAD vs working copy) for `path` in a
    /// fresh pane beside the focused one (same placement as `open_browser`).
    fn open_diff(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        let repo_root = self.active_cwd.clone();
        let handle = ctx.add_typed_action_view(move |ctx| {
            crate::warpui::diff_view::WarpDiffView::new(ctx, repo_root, path)
        });
        self.split_with(PaneContent::Diff(handle));
    }

    /// Open the Welcome / landing pane beside the focused pane. Its action cards
    /// dispatch a `WelcomeAction` that this closure maps to the matching shell
    /// action (mirrors the top-bar pills). Created with `add_typed_action_view`
    /// so the shell is recorded as the pane's responder-chain parent — without
    /// that, the card's `CraneShellAction` would never bubble up to the shell.
    fn open_welcome(&mut self, ctx: &mut ViewContext<Self>) {
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
    fn edit_new_entry(&mut self, ks: &warpui::keymap::Keystroke) {
        match ks.key.as_str() {
            "enter" | "return" | "numpadenter" => self.commit_pending_entry(),
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
    fn commit_pending_entry(&mut self) {
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
                self.refresh_panel();
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
    fn any_text_input_focused(&self) -> bool {
        if self.commit_focused || self.pending_new_entry.is_some() {
            return true;
        }
        self.active_input_pane()
            .map(|id| {
                matches!(
                    self.panes.get(&id),
                    Some(PaneContent::Editor(_)) | Some(PaneContent::File(_))
                )
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
            .push(TabMeta { id, name });
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
        self.refresh_panel();
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
            self.refresh_panel();
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
    }

    /// Close the focused pane (and its terminal). Collapses the split tree.
    fn close_focused(&mut self) {
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
                    self.active_tab = None;
                    self.focused = None;
                }
            }
        }
        self.panes.remove(&focused);
        self.drag_states.remove(&focused);
        self.pane_rects.borrow_mut().remove(&focused);
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
                    self.left_sidebar(),
                    inner,
                    self.left_ratio.clone(),
                    self.left_drag.clone(),
                    theme::divider(),
                )
                .finish()
            }
            (true, false) => SplitBox::new(
                Dir::Horizontal,
                self.left_sidebar(),
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
            .with_child(self.status_bar())
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
        if let Some((x, y)) = self.branch_picker {
            root_stack = root_stack.with_child(self.branch_picker_overlay(x, y));
        }
        if let Some(p) = &self.pending_delete {
            root_stack = root_stack.with_child(self.delete_confirm_overlay(p));
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
            .on_keydown(|ctx, _app, ks| {
                if ks.cmd && !ks.ctrl && !ks.alt {
                    // Shift uppercases the key ("D"), so normalize the case.
                    let key = ks.key.to_ascii_lowercase();
                    let act = match key.as_str() {
                        "b" => Some(CraneShellAction::ToggleLeft),
                        "/" => Some(CraneShellAction::ToggleRight),
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
                        // route through the editor's own input_key).
                        // TODO(parity): Cmd+Shift+F should open the cross-file
                        // Find-in-Files modal (needs the modal framework, not yet
                        // ported). For now Shift falls through to the in-file bar.
                        "f" => Some(CraneShellAction::FindFocused),
                        "h" => Some(CraneShellAction::ReplaceFocused),
                        "g" => Some(CraneShellAction::GotoLineFocused),
                        // Cmd+Shift+O adds a project (folder picker); Cmd+O opens
                        // an external file (file picker). Matches old shortcuts.rs.
                        "o" if ks.shift => Some(CraneShellAction::AddProject),
                        "o" => Some(CraneShellAction::OpenExternalFile),
                        // Cmd+[ / Cmd+] cycle focus across panes in the active tab.
                        "[" => Some(CraneShellAction::FocusPrevPane),
                        "]" => Some(CraneShellAction::FocusNextPane),
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
    SetTab { files: bool },
    ToggleDir(PathBuf),
    SelectFile(PathBuf),
    ToggleProject(usize),
    ToggleWorktree(usize, usize),
    /// Toggle collapse/expand of a folder group, keyed by its shared parent
    /// directory path (`ProjectNode::group_path`).
    ToggleGroup(String),
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
    ShowChangeMenu { path: String, staged: bool, x: f32, y: f32 },
    /// Open the Files-row right-click menu.
    ShowFileMenu { path: PathBuf, is_dir: bool, x: f32, y: f32 },
    /// Open an absolute path in the editor/Files pane (context-menu Open).
    OpenFileAt(PathBuf),
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
    /// Open a Browser pane (placeholder).
    OpenBrowser,
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
    /// Close a tab (project, worktree, tab_id) from the strip.
    CloseTab((usize, usize, usize)),
    /// Switch to a named theme (cycles through all installed themes).
    SetTheme(String),
    /// Open a native folder picker and add the chosen directory as a new project.
    AddProject,
    /// Open a native file picker and open the chosen file into the Files pane.
    OpenExternalFile,
    /// Remove the project at index `i` from the project list and persist.
    RemoveProject(usize),
    /// Show the project context menu anchored at the given window position.
    ShowProjectMenu { project_idx: usize, x: f32, y: f32 },
    /// Dismiss the active project context menu.
    CloseContextMenu,
    /// Reveal the project folder in the system file manager.
    RevealProjectInFinder(usize),
    /// Copy the project path to the clipboard.
    CopyProjectPath(usize),
    /// Set or clear a per-project tint. `None` resets to the palette default.
    SetProjectTint(usize, Option<[u8; 3]>),
    /// Run `git init` in the project folder and reload the project list so it
    /// flips from loose (FOLDER icon) to a real git project (CUBE icon + branches).
    InitGitProject(usize),
    Noop,
}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
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
                self.refresh_panel();
            }
            CraneShellAction::SplitFocused(dir) => self.split_focused(*dir, ctx),
            // TODO(parity): Cmd+W should close the active File Tab first when a
            // Files/Editor pane has >1 tabs, and stage a running-process confirm
            // modal for terminals with a live foreground process. Both need the
            // (unported) confirm-modal framework; for now it tears the pane down.
            CraneShellAction::CloseFocused => self.close_focused(),
            CraneShellAction::FocusPane(id) => {
                self.focused = Some(*id);
                self.commit_focused = false;
            }
            CraneShellAction::ClosePane(id) => {
                self.focused = Some(*id);
                if self.maximized == Some(*id) {
                    self.maximized = None;
                }
                self.close_focused();
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
                if self.pending_new_entry.is_some() {
                    self.edit_new_entry(ks);
                } else if self.commit_focused {
                    self.edit_commit(ks);
                } else if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.write_keystroke(ks));
                    } else if let Some(h) = self.editor_at(id) {
                        // Warp editor pane: translate the keystroke and apply it.
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
                    h.update(ctx, |view, vctx| {
                        view.save(vctx);
                    });
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
                // TODO(parity): with NO editor focus, Cmd+Z should undo the last
                // Files-pane move/trash op (old `undo_last_file_op`). Deferred
                // until a Files-pane file-op undo stack is ported to warpui.
                if let Some(h) = self.active_input_pane().and_then(|id| self.file_at(id)) {
                    h.update(ctx, |view, vctx| {
                        view.undo();
                        vctx.notify();
                    });
                } else if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.undo(vctx));
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
                            self.close_focused();
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
                let text = ctx.clipboard().read().plain_text;
                if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.paste_text(&text));
                    } else if let Some(h) = self.file_at(id) {
                        h.update(ctx, |view, vctx| {
                            view.paste_at_cursor(&text);
                            vctx.notify();
                        });
                    } else if let Some(h) = self.editor_at(id) {
                        h.update(ctx, |view, vctx| view.paste(&text, vctx));
                    }
                }
            }
            CraneShellAction::SelectAllFocused => {
                // Terminal panes select the whole grid (old terminal/view.rs
                // Cmd+A); editor panes select all buffer text.
                if let Some(id) = self.active_input_pane() {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |view, _| view.select_all());
                    } else if let Some(h) = self.editor_at(id) {
                        h.update(ctx, |view, vctx| view.select_all(vctx));
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
                    self.refresh_panel();
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
                    self.refresh_panel();
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
            CraneShellAction::ShowChangeMenu { path, staged, x, y } => {
                self.row_menu = Some(RowMenu::Change {
                    path: path.clone(),
                    staged: *staged,
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
                self.refresh_panel();
            }
            CraneShellAction::RequestDelete(path) => {
                self.row_menu = None;
                self.pending_delete = Some(path.clone());
            }
            CraneShellAction::ConfirmDelete => {
                if let Some(path) = self.pending_delete.take() {
                    let _ = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    if self.selected_file.as_deref() == Some(path.as_path()) {
                        self.selected_file = None;
                    }
                    self.refresh_panel();
                }
            }
            CraneShellAction::CancelDelete => {
                self.pending_delete = None;
            }
            CraneShellAction::ShowBranchPicker { x, y } => {
                if let Some(root) = self.active_cwd.clone() {
                    let mut list = crate::warpui::git::list_local_branches(&root);
                    for r in crate::warpui::git::list_remote_branches(&root) {
                        list.push(r);
                    }
                    self.branch_list = list;
                    self.branch_picker = Some((*x, *y));
                }
            }
            CraneShellAction::CheckoutBranch(branch) => {
                self.branch_picker = None;
                if let Some(root) = self.active_cwd.clone() {
                    match crate::warpui::git::checkout_branch(&root, branch) {
                        Ok(()) => self.refresh_panel(),
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
            CraneShellAction::OpenGitLog => self.toggle_gitlog(),
            CraneShellAction::OpenBrowser => self.open_browser(ctx),
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
                self.save_state(&*ctx);
            }
            CraneShellAction::NewTab => {
                if let Some((pi, wi, _)) = self.active_tab {
                    self.add_tab(pi, wi, ctx);
                }
            }
            CraneShellAction::NewTabIn(pi, wi) => self.add_tab(*pi, *wi, ctx),
            CraneShellAction::CloseTab((pi, wi, tid)) => {
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
                if !self.any_text_input_focused() {
                    self.show_left = !self.show_left;
                }
            }
            CraneShellAction::ToggleRight => {
                if !self.any_text_input_focused() {
                    self.show_right = !self.show_right;
                }
            }
            CraneShellAction::SetTab { files } => {
                self.files_tab = *files;
                self.refresh_panel();
            }
            CraneShellAction::ToggleDir(p) => {
                if !self.expanded_dirs.remove(p) {
                    self.expanded_dirs.insert(p.clone());
                }
                self.refresh_panel();
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
            CraneShellAction::SetTheme(name) => {
                if let Some(t) = crate::theme::find_by_name(name) {
                    crate::theme::set(t);
                }
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
                        if !this.projects.iter().any(|p| p.path == path_str) {
                            this.added_projects.push(crate::warpui::persist::AddedProject {
                                name,
                                path: path_str.clone(),
                            });
                            // Re-add in case the user had previously removed it.
                            this.removed_project_paths.retain(|r| r != &path_str);
                            this.reload_projects();
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
                });
            }
            CraneShellAction::RemoveProject(i) => {
                self.context_menu = None;
                if let Some(p) = self.projects.get(*i) {
                    let path = p.path.clone();
                    self.added_projects.retain(|ap| ap.path != path);
                    if !self.removed_project_paths.contains(&path) {
                        self.removed_project_paths.push(path);
                    }
                }
                self.reload_projects();
            }
            CraneShellAction::ShowProjectMenu { project_idx, x, y } => {
                self.context_menu = Some(ProjectContextMenu {
                    project_idx: *project_idx,
                    x: *x,
                    y: *y,
                });
            }
            CraneShellAction::CloseContextMenu => {
                self.context_menu = None;
                self.row_menu = None;
                self.branch_picker = None;
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
                if let Some(p) = self.projects.get(*i) {
                    let path = p.path.clone();
                    match tint {
                        Some(rgb) => {
                            self.project_tints.insert(path, *rgb);
                        }
                        None => {
                            self.project_tints.remove(&path);
                        }
                    }
                }
                self.reload_projects();
            }
            CraneShellAction::InitGitProject(i) => {
                self.context_menu = None;
                if let Some(p) = self.projects.get(*i) {
                    let dir = std::path::PathBuf::from(&p.path);
                    // Shell out `git init` — never libgit2, per project rules.
                    let _ = crate::warpui::git::init(&dir);
                }
                // Reload so `is_loose` is recomputed and the CUBE icon / branch
                // rows appear on the next render.
                self.reload_projects();
            }
            CraneShellAction::Noop => {}
        }
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
        // must be notified to re-run its layout at the new pane size.
        if let Some(tab) = self.active_tab {
            if let Some(node) = self.layouts.get(&tab) {
                let mut leaves = Vec::new();
                node.leaves(&mut leaves);
                for id in leaves {
                    if let Some(h) = self.terminal_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    } else if let Some(h) = self.file_at(id) {
                        h.update(ctx, |_, vctx| vctx.notify());
                    }
                }
            }
        }
        // Persist UI state after every action so a restart restores the
        // workspace. Re-snapshotting terminal scrollback is expensive, so only
        // do it on non-keystroke actions (a keystroke's output is captured on
        // the next heavier action, e.g. focus/switch/split).
        self.save_state(&*ctx);
        // Mark the view dirty so warpui re-renders.
        ctx.notify();
    }
}

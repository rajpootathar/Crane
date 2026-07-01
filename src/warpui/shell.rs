//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::warpui::file_pane::FileView;
use crate::warpui::file_tree;
use crate::warpui::icons;
use crate::warpui::layout::{Dir, Node, PaneId};
use crate::warpui::rect_probe::{pane_under, DockEdge, PaneRect, RectProbe};
use crate::warpui::split::SplitBox;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, DispatchEventResult, Draggable, DraggableState,
    EventHandler, Expanded, Flex, ParentElement, Rect, Stack, Text,
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
}

#[derive(Clone)]
pub struct TabMeta {
    pub id: usize,
    pub name: String,
}

/// What a leaf pane holds (warpui port of old Crane's `PaneContent`). More
/// variants (Browser, GitLog, Markdown) follow.
pub enum PaneContent {
    Terminal(ViewHandle<TerminalView>),
    File(ViewHandle<FileView>),
    /// Warp's real text editor (warp_editor) — warp-quality file editing.
    Editor(ViewHandle<crate::warpui::editor_view::WarpEditorView>),
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
        let projects = crate::warpui::projects::load_projects();
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
        if let Some(st) = crate::warpui::persist::load() {
            // Restore the active theme BEFORE building any UI so every colour
            // token call below reads the right palette.
            if !st.theme_name.is_empty() {
                if let Some(t) = crate::theme::find_by_name(&st.theme_name) {
                    crate::theme::set(t);
                }
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
        });
    }

    fn refresh_panel(&mut self) {
        let root = self.active_cwd.clone();
        self.branch = root
            .as_deref()
            .map(crate::warpui::git::current_branch)
            .unwrap_or_default();
        match root {
            Some(root) if self.files_tab => {
                self.file_rows = file_tree::build_rows(&root, &self.expanded_dirs);
            }
            Some(root) => {
                self.changes = crate::warpui::git::changes(&root);
            }
            None => {
                self.file_rows.clear();
                self.changes.clear();
            }
        }
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
        // Real project tree loaded from ~/.crane/session.json: the user's
        // actual projects -> worktrees (branches) -> tabs.
        let mut col = Flex::column().with_child(self.header("PROJECTS"));
        if self.projects.is_empty() {
            col = col.with_child(self.tree_row(
                "(no ~/.crane/session.json)",
                12.0,
                theme::text_muted(),
                12.0,
            ));
        }
        let sel = self.selected;
        for (pi, p) in self.projects.iter().enumerate() {
            let p_expanded = self.expanded_projects.contains(&pi);
            let psel = sel == (pi, usize::MAX, usize::MAX);
            let pcol = if psel { theme::text_hover() } else { theme::text() };
            col = col.with_child(self.nav_row(
                Some(p_expanded),
                icons::CUBE,
                project_tint(pi),
                &p.name,
                13.0,
                pcol,
                10.0,
                psel,
                CraneShellAction::ToggleProject(pi),
            ));
            if !p_expanded {
                continue;
            }
            for (wi, w) in p.worktrees.iter().enumerate() {
                let w_expanded = self.expanded_worktrees.contains(&(pi, wi));
                let wsel = sel == (pi, wi, usize::MAX);
                let wcol = if wsel { theme::text_hover() } else { theme::accent() };
                col = col.with_child(self.nav_row(
                    Some(w_expanded),
                    icons::GIT_BRANCH,
                    wcol,
                    &w.name,
                    12.0,
                    wcol,
                    24.0,
                    wsel,
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
                        col = col.with_child(self.nav_row(
                            None,
                            icons::TERMINAL_WINDOW,
                            tcol,
                            &t.name,
                            11.0,
                            tcol,
                            42.0,
                            tsel,
                            CraneShellAction::Select {
                                sel: tkey,
                                path: PathBuf::from(&w.path),
                            },
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
                    42.0,
                    false,
                    CraneShellAction::NewTabIn(pi, wi),
                ));
            }
        }
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), col.finish())
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
        let glyph = if r.is_dir { icons::FOLDER } else { icons::FILE };
        let text_color = if r.is_dir { theme::text() } else { theme::text_muted() };
        let label_inner = Flex::row()
            .with_child(chevron)
            .with_child(
                Container::new(self.icon(glyph, 12.0, theme::text_muted()))
                    .with_padding_right(5.0)
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
        EventHandler::new(row)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    fn change_row(&self, ch: &crate::warpui::git::Change) -> Box<dyn Element> {
        let color = match ch.status.as_str() {
            "A" => theme::success(),
            "D" | "U" => theme::error(),
            "M" => theme::warning(),
            "R" | "C" => theme::accent(),
            _ => theme::text_muted(), // "?" untracked
        };
        // Leading marker: + = click to stage, − = click to unstage.
        let (marker, marker_color) = if ch.staged {
            (icons::MINUS, theme::success())
        } else {
            (icons::PLUS, theme::text_muted())
        };
        // Leading +/- marker: click to stage / unstage (own hit target).
        let stage_action = CraneShellAction::StageToggle {
            path: ch.path.clone(),
            staged: ch.staged,
        };
        let marker_btn = EventHandler::new(
            Container::new(self.icon(marker, 11.0, marker_color))
                .with_background_color(theme::sidebar_bg())
                .with_padding_left(10.0)
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

        // Status letter + filename: click to OPEN the file in the editor pane.
        let label_inner = Flex::row()
            .with_child(
                ConstrainedBox::new(
                    Text::new(ch.status.clone(), self.ui_font, 11.0)
                        .with_color(color)
                        .finish(),
                )
                .with_width(18.0)
                .finish(),
            )
            .with_child(
                Text::new(ch.path.clone(), self.ui_font, 12.0)
                    .with_color(if ch.staged { theme::text() } else { theme::text_muted() })
                    .finish(),
            )
            .finish();
        let abs = self
            .active_cwd
            .as_ref()
            .map(|c| c.join(&ch.path))
            .unwrap_or_else(|| PathBuf::from(&ch.path));
        let open_action = CraneShellAction::SelectFile(abs);
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

        Flex::row()
            .with_child(marker_btn)
            .with_child(Expanded::new(1.0, label_btn).finish())
            .finish()
    }

    fn right_sidebar(&self) -> Box<dyn Element> {
        let tabs = Flex::row()
            .with_child(self.tab_label(
                "Changes",
                !self.files_tab,
                CraneShellAction::SetTab { files: false },
            ))
            .with_child(Self::spacer(12.0))
            .with_child(self.tab_label(
                "Files",
                self.files_tab,
                CraneShellAction::SetTab { files: true },
            ))
            .finish();
        let tabs = Container::new(tabs)
            .with_padding_left(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(6.0)
            .finish();

        let mut col = Flex::column().with_child(tabs);
        // Read CACHED contents (rebuilt in refresh_panel on action, not here).
        if self.files_tab {
            if self.file_rows.is_empty() {
                col = col.with_child(self.tree_row("(empty)", 12.0, theme::text_muted(), 12.0));
            }
            for r in &self.file_rows {
                col = col.with_child(self.file_row(r));
            }
        } else {
            if self.changes.is_empty() {
                col = col.with_child(self.tree_row("No changes", 12.0, theme::text_muted(), 12.0));
            }
            for ch in &self.changes {
                col = col.with_child(self.change_row(ch));
            }
            col = col.with_child(self.commit_box());
        }
        // No fixed width — the enclosing SplitBox sizes it (draggable).
        self.panel(theme::sidebar_bg(), col.finish())
    }

    /// Commit message field + Commit button at the bottom of the Changes tab.
    fn commit_box(&self) -> Box<dyn Element> {
        let staged = self.changes.iter().filter(|c| c.staged).count();
        let (text, color) = if self.commit_msg.is_empty() {
            ("Commit message…".to_string(), theme::text_muted())
        } else {
            // Caret when focused.
            let caret = if self.commit_focused { "|" } else { "" };
            (format!("{}{}", self.commit_msg, caret), theme::text())
        };
        // Click the field to focus it (keys route here instead of the terminal).
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

        let btn_label = format!("Commit ({staged})");
        let commit_btn = self.pill_button(icons::GIT_BRANCH, &btn_label, CraneShellAction::CommitStaged);

        Container::new(
            Flex::column()
                .with_child(field)
                .with_child(Self::spacer(6.0))
                .with_child(commit_btn)
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(8.0)
        .finish()
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
            EventHandler::new(
                Container::new(
                    Flex::row()
                        .with_child(
                            Container::new(self.icon(icons::TERMINAL_WINDOW, 12.0, fg))
                                .with_padding_right(5.0)
                                .finish(),
                        )
                        .with_child(
                            Text::new("Terminal".to_string(), self.ui_font, 11.0)
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
                Some(PaneContent::Editor(_)) | Some(PaneContent::File(_))
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

    /// Commit staged changes with the current message, then clear + refresh.
    fn commit_now(&mut self) {
        let msg = self.commit_msg.trim().to_string();
        if msg.is_empty() {
            return;
        }
        if let Some(root) = self.active_cwd.clone() {
            if crate::warpui::git::commit(&root, &msg).is_ok() {
                self.commit_msg.clear();
                self.commit_focused = false;
                self.refresh_panel();
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

        let root = Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(column)
            .finish();

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
                        "w" => Some(CraneShellAction::CloseFocused),
                        "v" => Some(CraneShellAction::PasteFocused),
                        "k" => Some(CraneShellAction::ClearFocused),
                        "s" => Some(CraneShellAction::SaveFocusedFile),
                        "a" => Some(CraneShellAction::SelectAllFocused),
                        "z" if ks.shift => Some(CraneShellAction::RedoFocused),
                        "z" => Some(CraneShellAction::UndoFocused),
                        "c" => Some(CraneShellAction::CopyFocused),
                        "x" => Some(CraneShellAction::CutFocused),
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
                ctx.dispatch_typed_action(CraneShellAction::SendKeys(ks.clone()));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }
}

/// Distinct per-project icon tint (stand-in until session.json tints are read).
fn project_tint(idx: usize) -> ColorU {
    const P: [(u8, u8, u8); 8] = [
        (232, 146, 42),
        (68, 170, 153),
        (170, 102, 204),
        (90, 135, 220),
        (204, 119, 221),
        (119, 204, 204),
        (232, 108, 108),
        (120, 200, 120),
    ];
    let (r, g, b) = P[idx % 8];
    ColorU::new(r, g, b, 255)
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
    /// Toggle stage/unstage for a changed file (click in the Changes tab).
    StageToggle { path: String, staged: bool },
    /// Give the commit message box keyboard focus.
    FocusCommit,
    /// Commit staged changes with the current message.
    CommitStaged,
    /// Open a Git log pane.
    OpenGitLog,
    /// Open a Browser pane (placeholder).
    OpenBrowser,
    /// Add a new tab to the active workspace.
    NewTab,
    /// Add a new tab to a specific worktree (left-panel + button).
    NewTabIn(usize, usize),
    /// Close a tab (project, worktree, tab_id) from the strip.
    CloseTab((usize, usize, usize)),
    /// Switch to a named theme (cycles through all installed themes).
    SetTheme(String),
    Noop,
}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CraneShellAction::Select { sel, path } => {
                self.selected = *sel;
                self.active_cwd = Some(path.clone());
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
                if self.commit_focused {
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
            CraneShellAction::UndoFocused => {
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
                        // Swap the Editor pane to show this file's live editor.
                        if let Some(h) = self.editor_views.get(&path).cloned() {
                            self.panes.insert(fp, PaneContent::Editor(h));
                        }
                    }
                }
            }
            CraneShellAction::FileTabClose(i) => {
                if let Some(fp) = self.files_pane {
                    if *i < self.file_pane_paths.len() {
                        let removed = self.file_pane_paths.remove(*i);
                        self.editor_views.remove(&removed);
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
                            if let Some(h) = self.editor_views.get(&path).cloned() {
                                self.panes.insert(fp, PaneContent::Editor(h));
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
                if let Some(h) = self.active_input_pane().and_then(|id| self.editor_at(id)) {
                    h.update(ctx, |view, vctx| view.select_all(vctx));
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
            CraneShellAction::StageToggle { path, staged } => {
                if let Some(root) = self.active_cwd.clone() {
                    let _ = if *staged {
                        crate::warpui::git::unstage(&root, path)
                    } else {
                        crate::warpui::git::stage(&root, path)
                    };
                    self.refresh_panel();
                }
            }
            CraneShellAction::OpenGitLog => self.toggle_gitlog(),
            CraneShellAction::OpenBrowser => self.open_browser(ctx),
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
            CraneShellAction::ToggleLeft => self.show_left = !self.show_left,
            CraneShellAction::ToggleRight => self.show_right = !self.show_right,
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
            CraneShellAction::SetTheme(name) => {
                if let Some(t) = crate::theme::find_by_name(name) {
                    crate::theme::set(t);
                }
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

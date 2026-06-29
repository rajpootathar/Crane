//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::file_tree;
use crate::icons;
use crate::split::SplitRow;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, DispatchEventResult, EventHandler, Expanded, Flex,
    ParentElement, Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::theme;
use crate::view::TerminalView;

pub struct CraneShellView {
    ui_font: FamilyId,
    icon_font: FamilyId,
    /// One PERSISTENT terminal per opened tab — created once, kept alive with
    /// full scrollback, switched (never respawned). History is retained.
    terminals: HashMap<(usize, usize, usize), ViewHandle<TerminalView>>,
    /// Which tab's terminal is shown in the center.
    active_tab: Option<(usize, usize, usize)>,
    projects: Vec<crate::projects::ProjectNode>,
    /// Active worktree dir — drives the Files/Changes panel root.
    active_cwd: Option<PathBuf>,
    /// Center split ratio (terminal | files), draggable.
    split_ratio: Rc<Cell<f32>>,
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
    changes: Vec<crate::git::Change>,
    /// Left tree expand state.
    expanded_projects: HashSet<usize>,
    expanded_worktrees: HashSet<(usize, usize)>,
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
                    vec![include_bytes!("../assets/Phosphor.ttf").to_vec()],
                )
                .expect("load phosphor")
        });
        let projects = crate::projects::load_projects();
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
        // Open one persistent terminal for the default worktree.
        let mut terminals: HashMap<(usize, usize, usize), ViewHandle<TerminalView>> =
            HashMap::new();
        let mut active_tab = None;
        if let Some(path) = default_cwd {
            let key = (0, 0, usize::MAX);
            terminals.insert(key, Self::spawn_terminal(ctx, path));
            active_tab = Some(key);
        }
        Self {
            ui_font,
            icon_font,
            terminals,
            active_tab,
            projects,
            active_cwd,
            split_ratio: Rc::new(Cell::new(0.68)),
            selected: (0, 0, usize::MAX),
            show_left: true,
            show_right: true,
            files_tab: true,
            expanded_dirs: HashSet::new(),
            selected_file: None,
            file_rows,
            changes: Vec::new(),
            expanded_projects: HashSet::from([0]),
            expanded_worktrees: HashSet::from([(0, 0)]),
        }
    }

    /// Spawn a new persistent terminal view rooted at `path`. Each gets its own
    /// PTY + repaint waker; it is never respawned (history retained).
    fn spawn_terminal(
        ctx: &mut ViewContext<Self>,
        path: PathBuf,
    ) -> ViewHandle<TerminalView> {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: crate::controller::Wake = std::sync::Arc::new(move || {
            let _ = tx.try_send(());
        });
        let cwd = Rc::new(RefCell::new(Some(path)));
        ctx.add_view(move |ctx| TerminalView::new_with(ctx, cwd, wake, rx))
    }

    /// Rebuild the cached right-panel contents for the active tab. Called from
    /// `handle_action` (never from render) so the FS walk / `git` shell-out
    /// happens once per change, not every repaint.
    fn refresh_panel(&mut self) {
        let root = self.active_cwd.clone();
        match root {
            Some(root) if self.files_tab => {
                self.file_rows = file_tree::build_rows(&root, &self.expanded_dirs);
            }
            Some(root) => {
                self.changes = crate::git::changes(&root);
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
        let content = Container::new(self.icon(glyph, 15.0, theme::TEXT_MUTED))
            .with_background_color(theme::TOPBAR_BG)
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
                Container::new(self.icon(glyph, 12.0, theme::TEXT_MUTED))
                    .with_padding_right(5.0)
                    .finish(),
            )
            .with_child(
                Text::new(label.to_string(), self.ui_font, 12.0)
                    .with_color(theme::TEXT_MUTED)
                    .finish(),
            )
            .finish();
        let content = Container::new(inner)
            .with_background_color(theme::SURFACE)
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
            bg = bg.with_background_color(theme::ROW_ACTIVE);
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
                Container::new(self.icon(glyph, 9.0, theme::TEXT_MUTED))
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
                .with_color(theme::TEXT_HEADER)
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
        ConstrainedBox::new(Rect::new().with_background_color(theme::DIVIDER).finish())
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
                theme::TEXT_MUTED,
                12.0,
            ));
        }
        let sel = self.selected;
        for (pi, p) in self.projects.iter().enumerate() {
            let p_expanded = self.expanded_projects.contains(&pi);
            let psel = sel == (pi, usize::MAX, usize::MAX);
            let pcol = if psel { theme::TEXT_HOVER } else { theme::TEXT };
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
                let wcol = if wsel { theme::TEXT_HOVER } else { theme::ACCENT };
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
                for (ti, t) in w.tabs.iter().enumerate() {
                    let tkey = (pi, wi, ti);
                    let tsel = sel == tkey;
                    let tcol = if tsel { theme::TEXT_HOVER } else { theme::TEXT_MUTED };
                    col = col.with_child(self.nav_row(
                        None,
                        icons::TERMINAL_WINDOW,
                        tcol,
                        t,
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
        }
        ConstrainedBox::new(self.panel(theme::SIDEBAR_BG, col.finish()))
            .with_width(theme::LEFT_W)
            .finish()
    }

    fn tab_label(&self, text: &str, active: bool, action: CraneShellAction) -> Box<dyn Element> {
        let color = if active { theme::TEXT_HOVER } else { theme::TEXT_MUTED };
        let content = Container::new(
            Text::new(text.to_string(), self.ui_font, 12.0)
                .with_color(color)
                .finish(),
        )
        .with_background_color(theme::SIDEBAR_BG)
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
                theme::TEXT_MUTED,
            ))
            .with_padding_right(4.0)
            .finish()
        } else {
            Self::spacer(13.0)
        };
        let glyph = if r.is_dir { icons::FOLDER } else { icons::FILE };
        let text_color = if r.is_dir { theme::TEXT } else { theme::TEXT_MUTED };
        let label_inner = Flex::row()
            .with_child(chevron)
            .with_child(
                Container::new(self.icon(glyph, 12.0, theme::TEXT_MUTED))
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
            bg = bg.with_background_color(theme::ROW_ACTIVE);
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

    fn change_row(&self, ch: &crate::git::Change) -> Box<dyn Element> {
        let color = match ch.status.as_str() {
            "A" => theme::SUCCESS,
            "D" | "U" => theme::ERROR,
            "M" => theme::WARNING,
            "R" | "C" => theme::ACCENT,
            _ => theme::TEXT_MUTED, // "?" untracked
        };
        let inner = Flex::row()
            .with_child(
                ConstrainedBox::new(
                    Text::new(ch.status.clone(), self.ui_font, 11.0)
                        .with_color(color)
                        .finish(),
                )
                .with_width(22.0)
                .finish(),
            )
            .with_child(
                Text::new(ch.path.clone(), self.ui_font, 12.0)
                    .with_color(theme::TEXT)
                    .finish(),
            )
            .finish();
        Container::new(inner)
            .with_padding_left(12.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
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
                col = col.with_child(self.tree_row("(empty)", 12.0, theme::TEXT_MUTED, 12.0));
            }
            for r in &self.file_rows {
                col = col.with_child(self.file_row(r));
            }
        } else {
            if self.changes.is_empty() {
                col = col.with_child(self.tree_row("No changes", 12.0, theme::TEXT_MUTED, 12.0));
            }
            for ch in &self.changes {
                col = col.with_child(self.change_row(ch));
            }
        }
        ConstrainedBox::new(self.panel(theme::SIDEBAR_BG, col.finish()))
            .with_width(theme::RIGHT_W)
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
                .with_color(theme::TEXT_MUTED)
                .finish(),
        )
        .with_padding_left(6.0)
        .with_padding_top(9.0)
        .finish();
        let row = Flex::row()
            .with_child(Self::spacer(80.0)) // macOS traffic-light inset
            .with_child(self.icon_button(icons::SIDEBAR, CraneShellAction::ToggleLeft))
            .with_child(crumb)
            .with_child(Expanded::new(1.0, Rect::new().finish()).finish())
            .with_child(self.pill_button(icons::TERMINAL_WINDOW, "Terminal", CraneShellAction::Noop))
            .with_child(Self::spacer(6.0))
            .with_child(self.pill_button(icons::GLOBE, "Browser", CraneShellAction::Noop))
            .with_child(Self::spacer(8.0))
            .with_child(self.icon_button(icons::GIT_BRANCH, CraneShellAction::Noop))
            .with_child(self.icon_button(icons::SIDEBAR, CraneShellAction::ToggleRight))
            .with_child(Self::spacer(8.0))
            .finish();
        ConstrainedBox::new(self.panel(theme::TOPBAR_BG, row))
            .with_height(theme::TOPBAR_H)
            .finish()
    }

    fn status_bar(&self) -> Box<dyn Element> {
        let content = Container::new(
            Text::new("main  -  ready", self.ui_font, 11.0)
                .with_color(theme::TEXT_MUTED)
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_top(7.0)
        .finish();
        ConstrainedBox::new(self.panel(theme::TOPBAR_BG, content))
            .with_height(theme::STATUS_H)
            .finish()
    }

    fn center(&self) -> Box<dyn Element> {
        // Show the active tab's PERSISTENT terminal (history retained); other
        // tabs' terminals stay alive in `self.terminals`, just not rendered.
        let body: Box<dyn Element> = match self.active_tab.and_then(|k| self.terminals.get(&k)) {
            Some(handle) => ChildView::new(handle).finish(),
            None => Rect::new().with_background_color(theme::BG).finish(),
        };
        self.panel(theme::BG, body)
    }
}

impl Entity for CraneShellView {
    type Event = ();
}

impl View for CraneShellView {
    fn ui_name() -> &'static str {
        "CraneShellView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        let mut body = Flex::row();
        if self.show_left {
            body = body.with_child(self.left_sidebar()).with_child(self.divider());
        }
        body = body.with_child(Expanded::new(1.0, self.center()).finish());
        if self.show_right {
            body = body.with_child(self.divider()).with_child(self.right_sidebar());
        }
        let body = body.finish();

        let column = Flex::column()
            .with_child(self.top_bar())
            .with_child(Expanded::new(1.0, body).finish())
            .with_child(self.status_bar())
            .finish();

        Stack::new()
            .with_child(Rect::new().with_background_color(theme::BG).finish())
            .with_child(column)
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
    Noop,
}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CraneShellAction::Select { sel, path } => {
                self.selected = *sel;
                self.active_cwd = Some(path.clone());
                // Open this tab's persistent terminal once; thereafter just show
                // it (its PTY + scrollback have been running the whole time).
                if !self.terminals.contains_key(sel) {
                    let handle = Self::spawn_terminal(ctx, path.clone());
                    self.terminals.insert(*sel, handle);
                }
                self.active_tab = Some(*sel);
                self.refresh_panel();
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
            CraneShellAction::SelectFile(p) => self.selected_file = Some(p.clone()),
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
            CraneShellAction::Noop => {}
        }
        // Mark the view dirty so warpui re-renders.
        ctx.notify();
    }
}

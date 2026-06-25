//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

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
    terminal: ViewHandle<TerminalView>,
    projects: Vec<crate::projects::ProjectNode>,
    /// Shared with the terminal view; a sidebar click writes the project
    /// path here and the terminal respawns there.
    requested_cwd: Rc<RefCell<Option<PathBuf>>>,
    /// Center split ratio (terminal | files), draggable.
    split_ratio: Rc<Cell<f32>>,
    /// Selected (project_idx, worktree_idx, tab_idx) — drives breadcrumb +
    /// row highlight. `tab_idx == usize::MAX` means the worktree row itself.
    /// Plain view state: mutated in `handle_action` so warpui re-renders.
    selected: (usize, usize, usize),
    show_left: bool,
    show_right: bool,
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
        let requested_cwd: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(default_cwd));
        let terminal = {
            let rc = requested_cwd.clone();
            ctx.add_view(move |ctx| TerminalView::new_with(ctx, rc))
        };
        Self {
            ui_font,
            icon_font,
            terminal,
            projects,
            requested_cwd,
            split_ratio: Rc::new(Cell::new(0.68)),
            selected: (0, 0, usize::MAX),
            show_left: true,
            show_right: true,
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
    fn nav_row(
        &self,
        icon_glyph: &str,
        icon_color: ColorU,
        text: &str,
        size: f32,
        color: ColorU,
        pad: f32,
        path: &str,
        sel: (usize, usize, usize),
    ) -> Box<dyn Element> {
        let is_sel = self.selected == sel;
        let row_h = size + 8.0;
        // 3 layers, bottom -> top:
        //   1. highlight bar (colored only when selected)
        //   2. the label text
        //   3. a TRANSPARENT hit-recording Rect on top — this MUST be the
        //      topmost layer, because the EventHandler hit-tests at the
        //      child's *max* z-index. (Text records no hit geometry, and a
        //      hit Rect placed below the text sits under that max-z and is
        //      never found.) Transparent so the text shows through.
        let mut bg = Rect::new();
        if is_sel {
            bg = bg.with_background_color(theme::ROW_ACTIVE);
        }
        let bg_layer = ConstrainedBox::new(bg.finish()).with_height(row_h).finish();
        let label_inner = Flex::row()
            .with_child(
                Container::new(self.icon(icon_glyph, size, icon_color))
                    .with_padding_right(6.0)
                    .finish(),
            )
            .with_child(
                Text::new(text.to_string(), self.ui_font, size)
                    .with_color(color)
                    .finish(),
            )
            .finish();
        let label = Container::new(label_inner)
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

        if path.is_empty() {
            return row;
        }
        let target = PathBuf::from(path);
        EventHandler::new(row)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                // Route through a typed action so warpui re-renders the view.
                ctx.dispatch_typed_action(CraneShellAction::Select {
                    sel,
                    path: target.clone(),
                });
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
            let pkey = (pi, usize::MAX, usize::MAX);
            let pcol = if sel == pkey { theme::TEXT_HOVER } else { theme::TEXT };
            col = col.with_child(self.nav_row(
                icons::CUBE,
                project_tint(pi),
                &p.name,
                13.0,
                pcol,
                12.0,
                &p.path,
                pkey,
            ));
            for (wi, w) in p.worktrees.iter().enumerate() {
                let wkey = (pi, wi, usize::MAX);
                let wcol = if sel == wkey { theme::TEXT_HOVER } else { theme::ACCENT };
                col = col.with_child(self.nav_row(
                    icons::GIT_BRANCH,
                    wcol,
                    &w.name,
                    12.0,
                    wcol,
                    26.0,
                    &w.path,
                    wkey,
                ));
                for (ti, t) in w.tabs.iter().enumerate() {
                    let tkey = (pi, wi, ti);
                    let tcol = if sel == tkey { theme::TEXT_HOVER } else { theme::TEXT_MUTED };
                    col = col.with_child(self.nav_row(
                        icons::TERMINAL_WINDOW,
                        tcol,
                        t,
                        11.0,
                        tcol,
                        40.0,
                        &w.path,
                        tkey,
                    ));
                }
            }
        }
        ConstrainedBox::new(self.panel(theme::SIDEBAR_BG, col.finish()))
            .with_width(theme::LEFT_W)
            .finish()
    }

    fn right_sidebar(&self) -> Box<dyn Element> {
        let content = Flex::column()
            .with_child(self.header("CHANGES"))
            .with_child(self.row("M  src/view.rs", theme::TEXT))
            .with_child(self.row("M  src/shell.rs", theme::TEXT))
            .with_child(self.row("A  src/theme.rs", theme::ACCENT))
            .finish();
        ConstrainedBox::new(self.panel(theme::SIDEBAR_BG, content))
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

    fn spacer(w: f32) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().finish()).with_width(w).finish()
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
        // Draggable split: terminal | files. Drag the strip between them.
        SplitRow::new(
            ChildView::new(&self.terminal).finish(),
            self.files_pane(),
            self.split_ratio.clone(),
            theme::BORDER,
        )
        .finish()
    }

    fn files_pane(&self) -> Box<dyn Element> {
        let content = Flex::column()
            .with_child(self.header("FILES"))
            .with_child(self.tree_row("src/", 13.0, theme::TEXT, 12.0))
            .with_child(self.tree_row("Cargo.toml", 13.0, theme::TEXT_MUTED, 12.0))
            .with_child(self.tree_row("README.md", 13.0, theme::TEXT_MUTED, 12.0))
            .finish();
        self.panel(theme::BG, content)
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
    Noop,
}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CraneShellAction::Select { sel, path } => {
                self.selected = *sel;
                *self.requested_cwd.borrow_mut() = Some(path.clone());
            }
            CraneShellAction::ToggleLeft => self.show_left = !self.show_left,
            CraneShellAction::ToggleRight => self.show_right = !self.show_right,
            CraneShellAction::Noop => {}
        }
        // Mark the view dirty so warpui re-renders.
        ctx.notify();
    }
}

//! CraneShellView — the warpui app-shell prototype. Composes the same
//! Left/Center/Right + StatusBar structure as Crane's egui app, with the
//! real (already-ported) terminal pane docked in the center. Side panels
//! are placeholder content; the point is to prove the whole-app layout +
//! theme render in warpui exactly like the egui version.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

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
    terminal: ViewHandle<TerminalView>,
    projects: Vec<crate::projects::ProjectNode>,
    /// Shared with the terminal view; a sidebar click writes the project
    /// path here and the terminal respawns there.
    requested_cwd: Rc<RefCell<Option<PathBuf>>>,
}

impl CraneShellView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let ui_font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .load_system_font("Helvetica Neue")
                .or_else(|_| cache.load_system_font("Menlo"))
                .expect("load ui font")
        });
        let requested_cwd: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let terminal = {
            let rc = requested_cwd.clone();
            ctx.add_view(move |ctx| TerminalView::new_with(ctx, rc))
        };
        let projects = crate::projects::load_projects();
        Self {
            ui_font,
            terminal,
            projects,
            requested_cwd,
        }
    }

    /// A clickable project/worktree row — clicking respawns the terminal in
    /// `path` (empty path = non-clickable, e.g. a tab label).
    fn nav_row(
        &self,
        text: &str,
        size: f32,
        color: ColorU,
        pad: f32,
        path: &str,
    ) -> Box<dyn Element> {
        let inner = self.tree_row(text, size, color, pad);
        if path.is_empty() {
            return inner;
        }
        let cwd = self.requested_cwd.clone();
        let target = PathBuf::from(path);
        EventHandler::new(inner)
            .on_left_mouse_down(move |_ctx, _app, _pos| {
                *cwd.borrow_mut() = Some(target.clone());
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
        for p in &self.projects {
            col = col.with_child(self.nav_row(&p.name, 13.0, theme::TEXT, 12.0, &p.path));
            for w in &p.worktrees {
                col = col.with_child(self.nav_row(&w.name, 12.0, theme::ACCENT, 26.0, &w.path));
                for t in &w.tabs {
                    col = col.with_child(self.tree_row(t, 11.0, theme::TEXT_MUTED, 40.0));
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
    fn top_bar(&self) -> Box<dyn Element> {
        let content = Container::new(
            Text::new("crane  /  main  /  terminal", self.ui_font, 12.0)
                .with_color(theme::TEXT_MUTED)
                .finish(),
        )
        .with_padding_left(84.0)
        .with_padding_top(8.0)
        .finish();
        ConstrainedBox::new(self.panel(theme::TOPBAR_BG, content))
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
        ChildView::new(&self.terminal).finish()
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
        let body = Flex::row()
            .with_child(self.left_sidebar())
            .with_child(self.divider())
            .with_child(Expanded::new(1.0, self.center()).finish())
            .with_child(self.divider())
            .with_child(self.right_sidebar())
            .finish();

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

#[derive(Debug, Clone)]
pub enum CraneShellAction {}

impl TypedActionView for CraneShellView {
    type Action = CraneShellAction;
    fn handle_action(&mut self, _action: &Self::Action, _ctx: &mut ViewContext<Self>) {}
}

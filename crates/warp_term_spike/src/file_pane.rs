//! `FileView` — a dedicated File pane that holds multiple open files as TABS
//! (the warpui port of old Crane's `FilesPane`). v1 is read-only; editing +
//! syntect highlighting + find/replace are the (large) follow-up — old Crane's
//! `views/file_view.rs` has them.

use std::path::PathBuf;

use warpui::elements::{
    ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    ParentElement, Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext};

use crate::theme;

const MAX_LINES: usize = 2000;

struct OpenFile {
    name: String,
    lines: Vec<String>,
}

fn read_file(path: &PathBuf) -> OpenFile {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| format!("<cannot read {}: {e}>", path.display()));
    let lines = content.lines().take(MAX_LINES).map(str::to_string).collect();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    OpenFile { name, lines }
}

pub struct FileView {
    font: FamilyId,
    files: Vec<OpenFile>,
    active: usize,
    /// Set when this pane was created from pre-built text (git log etc.) — then
    /// it shows that single doc with no tab strip.
    is_doc: bool,
}

impl FileView {
    pub fn new(ctx: &mut ViewContext<Self>, path: PathBuf) -> Self {
        let font = Self::font(ctx);
        Self {
            font,
            files: vec![read_file(&path)],
            active: 0,
            is_doc: false,
        }
    }

    /// A single read-only doc pane from pre-built lines (git log, placeholders).
    pub fn from_text(ctx: &mut ViewContext<Self>, title: String, lines: Vec<String>) -> Self {
        let font = Self::font(ctx);
        Self {
            font,
            files: vec![OpenFile {
                name: title,
                lines: lines.into_iter().take(MAX_LINES).collect(),
            }],
            active: 0,
            is_doc: true,
        }
    }

    fn font(ctx: &mut ViewContext<Self>) -> FamilyId {
        warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache.load_system_font("Menlo").expect("load Menlo")
        })
    }

    /// Open `path` as a new tab (or switch to it if already open).
    pub fn open(&mut self, path: PathBuf) {
        let f = read_file(&path);
        if let Some(i) = self.files.iter().position(|of| of.name == f.name) {
            self.active = i;
        } else {
            self.files.push(f);
            self.active = self.files.len() - 1;
        }
    }

    fn tab_strip(&self) -> Box<dyn Element> {
        let mut row = Flex::row();
        for (i, f) in self.files.iter().enumerate() {
            let active = i == self.active;
            let bg = if active { theme::SURFACE } else { theme::TOPBAR_BG };
            let fg = if active { theme::TEXT } else { theme::TEXT_MUTED };
            let chip = EventHandler::new(
                Container::new(Text::new(f.name.clone(), self.font, 11.0).with_color(fg).finish())
                    .with_background_color(bg)
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(FileViewAction::Switch(i));
                DispatchEventResult::StopPropagation
            })
            .finish();
            row = row.with_child(chip);
        }
        ConstrainedBox::new(self.panel(theme::TOPBAR_BG, row.finish()))
            .with_height(28.0)
            .finish()
    }

    fn panel(&self, bg: warpui::color::ColorU, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(bg).finish())
            .with_child(content)
            .finish()
    }
}

impl Entity for FileView {
    type Event = ();
}

#[derive(Debug, Clone)]
pub enum FileViewAction {
    Switch(usize),
}

impl TypedActionView for FileView {
    type Action = FileViewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FileViewAction::Switch(i) => {
                if *i < self.files.len() {
                    self.active = *i;
                }
            }
        }
        ctx.notify();
    }
}

impl View for FileView {
    fn ui_name() -> &'static str {
        "FileView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        let mut content = Flex::column();
        // Tab strip only for real file panes with >0 files (doc panes are single).
        if !self.is_doc {
            content = content.with_child(self.tab_strip());
        }
        let mut body = Flex::column();
        if let Some(f) = self.files.get(self.active) {
            for line in &f.lines {
                body = body.with_child(
                    Text::new(line.clone(), self.font, 12.0)
                        .with_color(theme::TEXT)
                        .finish(),
                );
            }
        }
        content = content.with_child(Expanded::new(1.0, body.finish()).finish());
        self.panel(theme::BG, content.finish())
    }
}

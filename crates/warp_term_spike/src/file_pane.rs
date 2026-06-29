//! `FileView` — a minimal read-only file pane (the warpui port of old Crane's
//! `views/file_view.rs`). v1: reads the file and renders its lines in a
//! monospace column. (Syntax highlighting, editing, find/replace, and scroll
//! are follow-ups — old Crane's file_view has them.)

use std::path::PathBuf;

use warpui::elements::{Element, Flex, ParentElement, Rect, Stack, Text};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, SingletonEntity as _, View, ViewContext};

use crate::theme;

/// Cap lines rendered (no virtualization yet) so a huge file can't blow up the
/// element tree.
const MAX_LINES: usize = 2000;

pub struct FileView {
    font: FamilyId,
    #[allow(dead_code)]
    path: PathBuf,
    lines: Vec<String>,
}

impl FileView {
    pub fn new(ctx: &mut ViewContext<Self>, path: PathBuf) -> Self {
        let font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache.load_system_font("Menlo").expect("load Menlo")
        });
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| format!("<cannot read {}: {e}>", path.display()));
        let lines: Vec<String> = content.lines().take(MAX_LINES).map(str::to_string).collect();
        Self { font, path, lines }
    }
}

impl Entity for FileView {
    type Event = ();
}

impl View for FileView {
    fn ui_name() -> &'static str {
        "FileView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        let mut col = Flex::column();
        for line in &self.lines {
            col = col.with_child(
                Text::new(line.clone(), self.font, 12.0)
                    .with_color(theme::TEXT)
                    .finish(),
            );
        }
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::BG).finish())
            .with_child(col.finish())
            .finish()
    }
}

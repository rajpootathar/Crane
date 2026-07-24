//! `FileView` — a dedicated File pane that holds multiple open files as TABS
//! (the warpui port of old Crane's `FilesPane`). v1 is read-only; editing +
//! syntect highlighting + find/replace are the (large) follow-up — old Crane's
//! `views/file_view.rs` has them.

use std::path::PathBuf;

use warpui::elements::{
    ConstrainedBox, DispatchEventResult, Element, EventHandler, Expanded, Flex, ParentElement,
    Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext};

use crate::warpui::theme;

/// Render cap (NOT a storage cap — the full file is kept; only a window is
/// drawn until real scroll/virtualization lands, so a huge file can't blow up
/// the element tree). Storing the whole file avoids the silent data loss the
/// 1:1 review flagged.
const RENDER_LINES: usize = 2000;

struct OpenFile {
    /// Full path — tabs are keyed by this so same-named files don't collide.
    path: PathBuf,
    lines: Vec<String>,
    /// Unsaved edits in THIS file (per-file, not per-view).
    dirty: bool,
}

pub struct FileView {
    font: FamilyId,
    files: Vec<OpenFile>,
    active: usize,
    /// Edit cursor (line, column) in CHAR units — char-indexed to stay
    /// unicode-safe.
    cursor: (usize, usize),
    /// Undo/redo snapshots of (active-file lines, cursor), keyed per active
    /// file index so switching tabs doesn't cross-contaminate history.
    undo: Vec<(usize, Vec<String>, (usize, usize))>,
    redo: Vec<(usize, Vec<String>, (usize, usize))>,
    /// First visible line (scroll offset).
    scroll: usize,
    /// Monospace cell metrics for click→(line,col) mapping.
    char_w: f32,
    line_h: f32,
    /// Set when this pane was created from pre-built text (git log etc.) — then
    /// it shows that single doc with no tab strip.
    is_doc: bool,
}

impl FileView {
    /// Apply an editing keystroke to the active file. Char-indexed so unicode
    /// stays correct. Returns false for doc panes (read-only).
    pub fn edit(&mut self, ks: &warpui::keymap::Keystroke) {
        if self.is_doc {
            return;
        }
        let active = self.active;
        // Snapshot for undo on mutating keys (not pure cursor moves), capped.
        let mutating = matches!(
            ks.key.as_str(),
            "backspace" | "enter" | "return" | "numpadenter" | "tab"
        ) || ks.key.chars().count() == 1;
        if mutating {
            if let Some(f) = self.files.get(active) {
                self.undo.push((active, f.lines.clone(), self.cursor));
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
        }
        let Some(f) = self.files.get_mut(active) else {
            return;
        };
        if f.lines.is_empty() {
            f.lines.push(String::new());
        }
        let (mut l, mut c) = self.cursor;
        l = l.min(f.lines.len() - 1);
        c = c.min(f.lines[l].chars().count());
        let mut changed = false;

        // Helper: rebuild line `li` from a char Vec.
        match ks.key.as_str() {
            "backspace" => {
                if c > 0 {
                    let mut ch: Vec<char> = f.lines[l].chars().collect();
                    ch.remove(c - 1);
                    f.lines[l] = ch.into_iter().collect();
                    c -= 1;
                    changed = true;
                } else if l > 0 {
                    let cur = f.lines.remove(l);
                    let prev_len = f.lines[l - 1].chars().count();
                    f.lines[l - 1].push_str(&cur);
                    l -= 1;
                    c = prev_len;
                    changed = true;
                }
            }
            "enter" | "return" | "numpadenter" => {
                let ch: Vec<char> = f.lines[l].chars().collect();
                let left: String = ch[..c].iter().collect();
                let right: String = ch[c..].iter().collect();
                f.lines[l] = left;
                f.lines.insert(l + 1, right);
                l += 1;
                c = 0;
                changed = true;
            }
            "tab" => {
                let mut ch: Vec<char> = f.lines[l].chars().collect();
                for _ in 0..4 {
                    ch.insert(c, ' ');
                }
                f.lines[l] = ch.into_iter().collect();
                c += 4;
                changed = true;
            }
            "left" => {
                if c > 0 {
                    c -= 1;
                } else if l > 0 {
                    l -= 1;
                    c = f.lines[l].chars().count();
                }
            }
            "right" => {
                let len = f.lines[l].chars().count();
                if c < len {
                    c += 1;
                } else if l + 1 < f.lines.len() {
                    l += 1;
                    c = 0;
                }
            }
            "up" => {
                if l > 0 {
                    l -= 1;
                    c = c.min(f.lines[l].chars().count());
                }
            }
            "down" => {
                if l + 1 < f.lines.len() {
                    l += 1;
                    c = c.min(f.lines[l].chars().count());
                }
            }
            k if k.chars().count() == 1 => {
                let chr = k.chars().next().unwrap();
                let mut ch: Vec<char> = f.lines[l].chars().collect();
                ch.insert(c, chr);
                f.lines[l] = ch.into_iter().collect();
                c += 1;
                changed = true;
            }
            _ => {}
        }
        self.cursor = (l, c);
        if changed {
            self.files[active].dirty = true;
        }
    }

    /// Copy the active (cursor) line — old Crane's empty-selection Cmd+C.
    /// Returns the line text (with trailing newline) for the clipboard.
    pub fn copy_line(&self) -> Option<String> {
        let f = self.files.get(self.active)?;
        let line = f.lines.get(self.cursor.0)?;
        Some(format!("{line}\n"))
    }

    /// Cut the active line — old Crane's empty-selection Cmd+X. Returns the line
    /// text and removes it (kept undoable).
    pub fn cut_line(&mut self) -> Option<String> {
        if self.is_doc {
            return None;
        }
        let idx = self.active;
        let l = self.cursor.0;
        let f = self.files.get(idx)?;
        if l >= f.lines.len() {
            return None;
        }
        let text = format!("{}\n", f.lines[l]);
        // Snapshot for undo.
        self.undo.push((idx, f.lines.clone(), self.cursor));
        self.redo.clear();
        let f = self.files.get_mut(idx)?;
        f.lines.remove(l);
        if f.lines.is_empty() {
            f.lines.push(String::new());
        }
        f.dirty = true;
        let new_l = l.min(f.lines.len() - 1);
        self.cursor = (new_l, 0);
        Some(text)
    }

    /// Undo the last edit (restores buffer + cursor).
    pub fn undo(&mut self) {
        if let Some((idx, lines, cur)) = self.undo.pop() {
            if let Some(f) = self.files.get_mut(idx) {
                self.redo.push((idx, f.lines.clone(), self.cursor));
                f.lines = lines;
                f.dirty = true;
                self.active = idx;
                self.cursor = cur;
            }
        }
    }

    /// Redo the last undone edit.
    pub fn redo(&mut self) {
        if let Some((idx, lines, cur)) = self.redo.pop() {
            if let Some(f) = self.files.get_mut(idx) {
                self.undo.push((idx, f.lines.clone(), self.cursor));
                f.lines = lines;
                f.dirty = true;
                self.active = idx;
                self.cursor = cur;
            }
        }
    }

    /// Insert clipboard `text` at the cursor (handles multi-line paste).
    pub fn paste_at_cursor(&mut self, text: &str) {
        if self.is_doc || text.is_empty() {
            return;
        }
        let active = self.active;
        if self.files.get(active).is_none() {
            return;
        }
        let (mut l, mut c) = self.cursor;
        for ch in text.chars() {
            let f = &mut self.files[active];
            if l >= f.lines.len() {
                f.lines.push(String::new());
            }
            let mut chars: Vec<char> = f.lines[l].chars().collect();
            let col = c.min(chars.len());
            if ch == '\n' {
                let left: String = chars[..col].iter().collect();
                let right: String = chars[col..].iter().collect();
                f.lines[l] = left;
                f.lines.insert(l + 1, right);
                l += 1;
                c = 0;
            } else {
                chars.insert(col, ch);
                f.lines[l] = chars.into_iter().collect();
                c += 1;
            }
        }
        self.cursor = (l, c);
        self.files[active].dirty = true;
    }

    /// Write the active file's buffer back to disk (Cmd+S).
    pub fn save(&mut self) -> bool {
        let Some(f) = self.files.get_mut(self.active) else {
            return false;
        };
        if f.path.as_os_str().is_empty() {
            return false; // doc pane (git log / browser)
        }
        if std::fs::write(&f.path, f.lines.join("\n")).is_ok() {
            f.dirty = false;
            true
        } else {
            false
        }
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
    /// Scroll by N lines (positive = down).
    Scroll(i32),
}

impl TypedActionView for FileView {
    type Action = FileViewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FileViewAction::Scroll(delta) => {
                let max = self
                    .files
                    .get(self.active)
                    .map(|f| f.lines.len().saturating_sub(1))
                    .unwrap_or(0);
                let next = self.scroll as i64 + *delta as i64;
                self.scroll = next.clamp(0, max as i64) as usize;
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
        // NOTE: the file TAB STRIP is rendered by the SHELL in the pane header
        // (so clicks route through the shell), not here. This view shows only
        // the active file's content.
        let mut content = Flex::column();
        let mut body = Flex::column();
        if let Some(f) = self.files.get(self.active) {
            // Render a WINDOW of lines from the scroll offset (manual scroll —
            // same approach as the terminal, avoids unbounded element trees).
            let start = self.scroll.min(f.lines.len().saturating_sub(1));
            for (i, line) in f
                .lines
                .iter()
                .enumerate()
                .skip(start)
                .take(RENDER_LINES)
            {
                // Each line is ALWAYS a single Text (so soft-wrap is consistent).
                // The caret is OVERLAID via a Stack — positioned with a spacer of
                // width col*char_w — so it never changes how the line wraps.
                let text = Text::new(line.clone(), self.font, 12.0)
                    .with_color(theme::text())
                    .finish();
                if !self.is_doc && i == self.cursor.0 {
                    let col = self.cursor.1.min(line.chars().count());
                    let caret_x = col as f32 * self.char_w;
                    let caret_overlay = Flex::row()
                        .with_child(
                            ConstrainedBox::new(Rect::new().finish())
                                .with_width(caret_x)
                                .with_height(1.0)
                                .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Rect::new().with_background_color(theme::accent()).finish(),
                            )
                            .with_width(2.0)
                            .with_height(self.line_h.max(12.0))
                            .finish(),
                        )
                        .finish();
                    body = body.with_child(
                        Stack::new().with_child(text).with_child(caret_overlay).finish(),
                    );
                } else {
                    body = body.with_child(text);
                }
            }
        }
        // Scroll wheel adjusts the line window.
        let scroll_body = EventHandler::new(Expanded::new(1.0, body.finish()).finish())
            .on_scroll_wheel(move |ctx, _app, delta, _mods| {
                let lines = (-delta.y() / 8.0).round() as i32;
                if lines != 0 {
                    ctx.dispatch_typed_action(FileViewAction::Scroll(lines));
                }
                DispatchEventResult::StopPropagation
            })
            .finish();
        content = content.with_child(Expanded::new(1.0, scroll_body).finish());
        self.panel(theme::bg(), content.finish())
    }
}

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

use crate::warpui::theme;

/// Render cap (NOT a storage cap — the full file is kept; only a window is
/// drawn until real scroll/virtualization lands, so a huge file can't blow up
/// the element tree). Storing the whole file avoids the silent data loss the
/// 1:1 review flagged.
const RENDER_LINES: usize = 2000;

struct OpenFile {
    /// Full path — tabs are keyed by this so same-named files don't collide.
    path: PathBuf,
    name: String,
    lines: Vec<String>,
    /// Unsaved edits in THIS file (per-file, not per-view).
    dirty: bool,
}

fn read_file(path: &PathBuf) -> OpenFile {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| format!("<cannot read {}: {e}>", path.display()));
    let lines = content.lines().map(str::to_string).collect(); // full content
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    OpenFile {
        path: path.clone(),
        name,
        lines,
        dirty: false,
    }
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
            cursor: (0, 0),
            undo: Vec::new(),
            redo: Vec::new(),
            scroll: 0,
            is_doc: false,
        }
    }

    /// A single read-only doc pane from pre-built lines (git log, placeholders).
    pub fn from_text(ctx: &mut ViewContext<Self>, title: String, lines: Vec<String>) -> Self {
        let font = Self::font(ctx);
        Self {
            font,
            files: vec![OpenFile {
                path: PathBuf::new(),
                name: title,
                lines,
                dirty: false,
            }],
            active: 0,
            cursor: (0, 0),
            undo: Vec::new(),
            redo: Vec::new(),
            scroll: 0,
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
        if let Some(i) = self.files.iter().position(|of| of.path == f.path) {
            self.active = i;
        } else {
            self.files.push(f);
            self.active = self.files.len() - 1;
        }
        self.cursor = (0, 0);
    }

    pub fn is_dirty(&self) -> bool {
        self.files.get(self.active).map(|f| f.dirty).unwrap_or(false)
    }

    /// Switch the active file tab (shell-driven).
    pub fn switch(&mut self, i: usize) {
        if i < self.files.len() {
            self.active = i;
            self.cursor = (0, 0);
            self.scroll = 0;
        }
    }

    /// Close file tab `i` (shell-driven; keeps >=1 file).
    pub fn close_tab(&mut self, i: usize) {
        if i < self.files.len() && self.files.len() > 1 {
            self.files.remove(i);
            if self.active >= self.files.len() {
                self.active = self.files.len() - 1;
            } else if self.active > i {
                self.active -= 1;
            }
            self.cursor = (0, 0);
        }
    }

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

    fn tab_strip(&self) -> Box<dyn Element> {
        let mut row = Flex::row();
        for (i, f) in self.files.iter().enumerate() {
            let active = i == self.active;
            let bg = if active { theme::SURFACE } else { theme::TOPBAR_BG };
            let fg = if active { theme::TEXT } else { theme::TEXT_MUTED };
            // Per-file dirty marker.
            let label = if f.dirty {
                format!("* {}", f.name)
            } else {
                f.name.clone()
            };
            let chip = EventHandler::new(
                Container::new(Text::new(label, self.font, 11.0).with_color(fg).finish())
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
            // Close button (ASCII "x" — FileView only loads the mono font).
            let close = EventHandler::new(
                Container::new(
                    Text::new("x".to_string(), self.font, 11.0)
                        .with_color(theme::TEXT_MUTED)
                        .finish(),
                )
                .with_background_color(bg)
                .with_padding_left(2.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(FileViewAction::Close(i));
                DispatchEventResult::StopPropagation
            })
            .finish();
            row = row.with_child(Flex::row().with_child(chip).with_child(close).finish());
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
    Close(usize),
    /// Scroll by N lines (positive = down).
    Scroll(i32),
}

impl TypedActionView for FileView {
    type Action = FileViewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FileViewAction::Switch(i) => {
                if *i < self.files.len() {
                    self.active = *i;
                    self.cursor = (0, 0);
                }
            }
            FileViewAction::Close(i) => {
                // Keep at least one file open (closing the pane itself is the
                // shell's job — out of scope here).
                if *i < self.files.len() && self.files.len() > 1 {
                    self.files.remove(*i);
                    if self.active >= self.files.len() {
                        self.active = self.files.len() - 1;
                    } else if self.active > *i {
                        self.active -= 1;
                    }
                    self.cursor = (0, 0);
                }
            }
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
                // Cursor line: split at the caret column and insert a thin caret
                // Rect (no character shift). Other lines: plain text.
                if !self.is_doc && i == self.cursor.0 {
                    let ch: Vec<char> = line.chars().collect();
                    let col = self.cursor.1.min(ch.len());
                    let before: String = ch[..col].iter().collect();
                    let after: String = ch[col..].iter().collect();
                    let caret = ConstrainedBox::new(
                        Rect::new().with_background_color(theme::ACCENT).finish(),
                    )
                    .with_width(2.0)
                    .with_height(15.0)
                    .finish();
                    body = body.with_child(
                        Flex::row()
                            .with_child(
                                Text::new(before, self.font, 12.0)
                                    .with_color(theme::TEXT)
                                    .finish(),
                            )
                            .with_child(caret)
                            .with_child(
                                Text::new(after, self.font, 12.0)
                                    .with_color(theme::TEXT)
                                    .finish(),
                            )
                            .finish(),
                    );
                } else {
                    body = body.with_child(
                        Text::new(line.clone(), self.font, 12.0)
                            .with_color(theme::TEXT)
                            .finish(),
                    );
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
        self.panel(theme::BG, content.finish())
    }
}

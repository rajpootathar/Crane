//! Shared single-line text-edit model for every modal / bar text field.
//!
//! Replaces the append/pop `String` fields (New Workspace, Switch Branch,
//! Find in Files, git-log filter/prompt, inline rename, commit box, pending
//! new entry, editor find bar) with a real editing model: caret position,
//! selection, word navigation, and clipboard intents.
//!
//! The model is PURE — it never touches the clipboard or dispatches actions.
//! `handle_key` returns an [`Outcome`]; the caller performs clipboard reads/
//! writes and its own side effects (re-run search, clear error, …). Keys the
//! model doesn't own (enter, escape, tab, up/down) return `Ignored` so each
//! call site keeps its own confirm/cancel/navigation semantics.
//!
//! Caret and selection anchor are BYTE offsets into `text`, always on char
//! boundaries (same convention as `editor_view`).

use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::keymap::Keystroke;
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

/// What a keystroke did to the field. Callers match on this to run their
/// side effects (e.g. re-filter on `Changed`) and to service clipboard
/// intents (`Copy`/`Cut` → write clipboard; `Paste` → read clipboard then
/// call [`LineEdit::insert_str`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Text changed (insert / delete).
    Changed,
    /// Caret or selection moved; text unchanged.
    CaretMoved,
    /// Cmd+C with a selection: write this to the clipboard.
    Copy(String),
    /// Cmd+X with a selection: write this to the clipboard (text already
    /// deleted from the field).
    Cut(String),
    /// Cmd+V: caller reads the clipboard and calls `insert_str`.
    Paste,
    /// Key not owned by the model (enter / escape / tab / up / down / other
    /// app chords). Caller decides.
    Ignored,
}

/// Single-line editable text field state: buffer + caret + selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// Caret byte offset, always on a char boundary.
    caret: usize,
    /// Selection anchor byte offset. `Some(a)` with `a != caret` means the
    /// range `min..max` of the two is selected. `Some(a)` with `a == caret`
    /// is treated as no selection.
    anchor: Option<usize>,
}

impl LineEdit {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.len();
        Self {
            text,
            caret,
            anchor: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Selected byte range (start < end), or None when the selection is
    /// empty / collapsed.
    pub fn selection(&self) -> Option<(usize, usize)> {
        match self.anchor {
            Some(a) if a != self.caret => Some((a.min(self.caret), a.max(self.caret))),
            _ => None,
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(s, e)| &self.text[s..e])
    }

    /// Replace the whole content (e.g. programmatic prefill), caret to end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.len();
        self.anchor = None;
    }

    /// Take the buffer out, resetting the field.
    pub fn take(&mut self) -> String {
        self.caret = 0;
        self.anchor = None;
        std::mem::take(&mut self.text)
    }

    /// Move the caret to a byte offset (snapped to the nearest char
    /// boundary at or below `to`, clamped to the text). Used for
    /// click-to-place.
    pub fn set_caret(&mut self, to: usize) {
        let mut to = to.min(self.text.len());
        while to > 0 && !self.text.is_char_boundary(to) {
            to -= 1;
        }
        self.caret = to;
        self.anchor = None;
    }

    /// Insert text at the caret, replacing the selection if any. Strips
    /// control chars and newlines (single-line field; pastes of multi-line
    /// clipboard content collapse to one line).
    pub fn insert_str(&mut self, s: &str) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        if let Some((start, end)) = self.selection() {
            self.text.replace_range(start..end, "");
            self.caret = start;
            self.anchor = None;
        }
        self.text.insert_str(self.caret, &clean);
        self.caret += clean.len();
    }

    /// Apply a keystroke. See [`Outcome`] for the caller contract.
    pub fn handle_key(&mut self, ks: &Keystroke) -> Outcome {
        let key = ks.key.as_str();

        // Cmd chords (macOS editing conventions).
        if ks.cmd && !ks.ctrl && !ks.alt {
            match key {
                "a" => {
                    if self.text.is_empty() {
                        return Outcome::CaretMoved;
                    }
                    self.anchor = Some(0);
                    self.caret = self.text.len();
                    return Outcome::CaretMoved;
                }
                "c" => {
                    return match self.selected_text() {
                        Some(sel) => Outcome::Copy(sel.to_string()),
                        None => Outcome::Ignored,
                    };
                }
                "x" => {
                    if let Some((start, end)) = self.selection() {
                        let cut = self.text[start..end].to_string();
                        self.text.replace_range(start..end, "");
                        self.caret = start;
                        self.anchor = None;
                        return Outcome::Cut(cut);
                    }
                    return Outcome::Ignored;
                }
                "v" => return Outcome::Paste,
                "left" => return self.move_to(0, ks.shift),
                "right" => return self.move_to(self.text.len(), ks.shift),
                "backspace" => {
                    // Cmd+Backspace: delete to line start (kill-to-BOL).
                    if self.selection().is_some() {
                        self.delete_selection();
                        return Outcome::Changed;
                    }
                    if self.caret == 0 {
                        return Outcome::CaretMoved;
                    }
                    self.text.replace_range(0..self.caret, "");
                    self.caret = 0;
                    return Outcome::Changed;
                }
                _ => return Outcome::Ignored,
            }
        }
        if ks.cmd {
            return Outcome::Ignored;
        }

        // Readline-style ctrl chords common on macOS text fields.
        if ks.ctrl && !ks.alt {
            match key {
                "a" => return self.move_to(0, ks.shift),
                "e" => return self.move_to(self.text.len(), ks.shift),
                _ => return Outcome::Ignored,
            }
        }
        if ks.ctrl {
            return Outcome::Ignored;
        }

        match key {
            "left" => {
                let target = if ks.alt {
                    self.prev_word_boundary(self.caret)
                } else if let (Some((start, _)), false) = (self.selection(), ks.shift) {
                    // Collapse an existing selection to its left edge.
                    start
                } else {
                    self.prev_char_boundary(self.caret)
                };
                self.move_to(target, ks.shift)
            }
            "right" => {
                let target = if ks.alt {
                    self.next_word_boundary(self.caret)
                } else if let (Some((_, end)), false) = (self.selection(), ks.shift) {
                    end
                } else {
                    self.next_char_boundary(self.caret)
                };
                self.move_to(target, ks.shift)
            }
            "home" => self.move_to(0, ks.shift),
            "end" => self.move_to(self.text.len(), ks.shift),
            "backspace" => {
                if self.selection().is_some() {
                    self.delete_selection();
                    return Outcome::Changed;
                }
                if self.caret == 0 {
                    return Outcome::CaretMoved;
                }
                let from = if ks.alt {
                    self.prev_word_boundary(self.caret)
                } else {
                    self.prev_char_boundary(self.caret)
                };
                self.text.replace_range(from..self.caret, "");
                self.caret = from;
                Outcome::Changed
            }
            "delete" => {
                if self.selection().is_some() {
                    self.delete_selection();
                    return Outcome::Changed;
                }
                if self.caret == self.text.len() {
                    return Outcome::CaretMoved;
                }
                let to = if ks.alt {
                    self.next_word_boundary(self.caret)
                } else {
                    self.next_char_boundary(self.caret)
                };
                self.text.replace_range(self.caret..to, "");
                Outcome::Changed
            }
            "space" => {
                self.insert_str(" ");
                Outcome::Changed
            }
            k if k.chars().count() == 1 && !ks.alt => {
                self.insert_str(k);
                Outcome::Changed
            }
            // Alt+char composes accents / special chars at OS level; the
            // keystroke still arrives as the composed char when it does, so a
            // 1-char alt key is inserted too.
            k if k.chars().count() == 1 => {
                self.insert_str(k);
                Outcome::Changed
            }
            _ => Outcome::Ignored,
        }
    }

    fn delete_selection(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.text.replace_range(start..end, "");
            self.caret = start;
        }
        self.anchor = None;
    }

    /// Move the caret to `to`; with `extend` the anchor stays (starting one
    /// at the old caret if there was none), otherwise the selection clears.
    fn move_to(&mut self, to: usize, extend: bool) -> Outcome {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = to;
        Outcome::CaretMoved
    }

    fn prev_char_boundary(&self, from: usize) -> usize {
        let mut i = from.saturating_sub(1);
        while i > 0 && !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn next_char_boundary(&self, from: usize) -> usize {
        if from >= self.text.len() {
            return self.text.len();
        }
        let mut i = from + 1;
        while i < self.text.len() && !self.text.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    /// Start of the word before `from`: skip separators leftward, then word
    /// chars leftward (readline `backward-word`).
    fn prev_word_boundary(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 {
            let p = self.prev_char_boundary(i);
            if Self::is_word_char(self.char_at(p)) {
                break;
            }
            i = p;
        }
        while i > 0 {
            let p = self.prev_char_boundary(i);
            if !Self::is_word_char(self.char_at(p)) {
                break;
            }
            i = p;
        }
        i
    }

    /// End of the word after `from`: skip separators rightward, then word
    /// chars rightward (readline `forward-word`).
    fn next_word_boundary(&self, from: usize) -> usize {
        let len = self.text.len();
        let mut i = from;
        while i < len && !Self::is_word_char(self.char_at(i)) {
            i = self.next_char_boundary(i);
        }
        while i < len && Self::is_word_char(self.char_at(i)) {
            i = self.next_char_boundary(i);
        }
        i
    }

    fn char_at(&self, byte: usize) -> char {
        self.text[byte..].chars().next().unwrap_or('\0')
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }
}

/// Painted single-line field: text with a real caret at its true x position,
/// selection highlight, placeholder, and click-to-place-caret. Pure painted
/// element in the `FindBarElement` idiom — glyphs drawn via `draw_glyph`,
/// widths from `glyph_advance`, so caret x and click mapping are exact for
/// proportional fonts too.
///
/// The element renders exactly one line of TEXT (no border / background /
/// padding) — call sites wrap it in their own `Container` chrome, so every
/// existing field keeps its look and only gains editing behavior.
pub struct LineEditField {
    text: String,
    caret: usize,
    selection: Option<(usize, usize)>,
    /// Draw the caret (the field has key focus). Unfocused fields render text
    /// only.
    focused: bool,
    placeholder: String,
    font: FamilyId,
    font_size: f32,
    color: ColorU,
    placeholder_color: ColorU,
    selection_bg: ColorU,
    caret_color: ColorU,
    /// Click-to-place callback with the clicked BYTE offset.
    on_click: Option<Rc<dyn Fn(&mut EventContext, usize)>>,

    size: Option<Vector2F>,
    origin: Option<Point>,
    /// Per-char (byte_offset, x_before_char, advance) built at paint time;
    /// used by `dispatch_event` to map a click x to a byte offset.
    advances: Vec<(usize, f32, f32)>,
    bounds: Option<RectF>,
}

impl LineEditField {
    pub fn new(le: &LineEdit, focused: bool, font: FamilyId, font_size: f32) -> Self {
        Self {
            text: le.text().to_string(),
            caret: le.caret(),
            selection: le.selection(),
            focused,
            placeholder: String::new(),
            font,
            font_size,
            color: crate::warpui::theme::text(),
            placeholder_color: crate::warpui::theme::text_muted(),
            selection_bg: crate::warpui::theme::selection_wash(),
            caret_color: crate::warpui::theme::accent(),
            on_click: None,
            size: None,
            origin: None,
            advances: Vec::new(),
            bounds: None,
        }
    }

    pub fn with_color(mut self, color: ColorU) -> Self {
        self.color = color;
        self
    }

    pub fn with_placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Byte offset the caller should move the caret to when the field is
    /// clicked at x. Also refocuses the field (callers decide in the closure).
    pub fn on_click_index(mut self, f: impl Fn(&mut EventContext, usize) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }

    fn advance_for(
        fc: &warpui::fonts::Cache,
        font_id: warpui::fonts::FontId,
        ch: char,
        size: f32,
    ) -> f32 {
        fc.glyph_for_char(font_id, ch, false)
            .and_then(|(gid, gf)| fc.glyph_advance(gf, size, gid).ok().map(|a| a.x()))
            .unwrap_or(size * 0.6)
    }
}

impl Element for LineEditField {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let fc = app.font_cache();
        let font_id = fc.select_font(self.font, Properties::default());
        let ascent = fc.ascent(font_id, self.font_size);
        let descent = fc.descent(font_id, self.font_size);
        let h = (ascent - descent).ceil();
        // Fill the parent's width when bounded (fields sit in bordered
        // containers); otherwise size to the text + caret slack so an
        // unbounded measure pass never produces an infinite rect.
        let max_w = constraint.max.x();
        let w = if max_w.is_finite() {
            max_w
        } else {
            let shown = if self.text.is_empty() { &self.placeholder } else { &self.text };
            shown
                .chars()
                .map(|c| Self::advance_for(fc, font_id, c, self.font_size))
                .sum::<f32>()
                + 2.0
        };
        let size = vec2f(w.max(constraint.min.x()), h.max(constraint.min.y()));
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        self.bounds = Some(RectF::new(origin, size));
        let fc = app.font_cache();
        let font_id = fc.select_font(self.font, Properties::default());
        let ascent = fc.ascent(font_id, self.font_size);
        let baseline = origin.y() + ascent;

        // Build per-char advances once per paint (also the click-map).
        self.advances.clear();
        let mut x = origin.x();
        for (byte, ch) in self.text.char_indices() {
            let adv = Self::advance_for(fc, font_id, ch, self.font_size);
            self.advances.push((byte, x, adv));
            x += adv;
        }
        let end_x = x;
        let x_for_byte = |b: usize| -> f32 {
            for (byte, cx, _) in &self.advances {
                if *byte >= b {
                    return *cx;
                }
            }
            end_x
        };

        // Selection wash under the selected byte range.
        if let Some((s, e)) = self.selection {
            let (sx, ex) = (x_for_byte(s), x_for_byte(e));
            if ex > sx {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(sx, origin.y()),
                        vec2f(ex - sx, size.y()),
                    ))
                    .with_background(Fill::Solid(self.selection_bg));
            }
        }

        // Text (or placeholder when empty).
        if self.text.is_empty() {
            let mut px = origin.x();
            for ch in self.placeholder.chars() {
                if let Some((gid, gf)) = fc.glyph_for_char(font_id, ch, false) {
                    ctx.scene
                        .draw_glyph(vec2f(px, baseline), gid, gf, self.font_size, self.placeholder_color);
                }
                px += Self::advance_for(fc, font_id, ch, self.font_size);
            }
        } else {
            for (i, (_, cx, _)) in self.advances.iter().enumerate() {
                let ch = self.text[self.advances[i].0..].chars().next().unwrap_or('\0');
                if let Some((gid, gf)) = fc.glyph_for_char(font_id, ch, false) {
                    ctx.scene
                        .draw_glyph(vec2f(*cx, baseline), gid, gf, self.font_size, self.color);
                }
            }
        }

        // Caret: steady 1.5px accent bar at the caret's true x.
        if self.focused {
            let cx = x_for_byte(self.caret).min(origin.x() + size.x() - 1.5);
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    vec2f(cx, origin.y()),
                    vec2f(1.5, size.y()),
                ))
                .with_background(Fill::Solid(self.caret_color));
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        let Some(on_click) = &self.on_click else {
            return false;
        };
        let Event::LeftMouseDown { position, .. } = event.raw_event() else {
            return false;
        };
        let Some(bounds) = self.bounds else {
            return false;
        };
        let inside = position.x() >= bounds.origin().x()
            && position.x() <= bounds.origin().x() + bounds.size().x()
            && position.y() >= bounds.origin().y()
            && position.y() <= bounds.origin().y() + bounds.size().y();
        if !inside {
            return false;
        }
        // Map click x to the nearest char boundary: past a char's midpoint
        // places the caret after it.
        let mut idx = self.text.len();
        for (byte, cx, adv) in &self.advances {
            if position.x() < cx + adv / 2.0 {
                idx = *byte;
                break;
            }
        }
        (on_click)(ctx, idx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str) -> Keystroke {
        Keystroke {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
            key: key.to_string(),
        }
    }

    fn ks_mod(key: &str, ctrl: bool, alt: bool, shift: bool, cmd: bool) -> Keystroke {
        Keystroke {
            ctrl,
            alt,
            shift,
            cmd,
            meta: false,
            key: key.to_string(),
        }
    }

    #[test]
    fn typing_inserts_at_caret_not_only_at_end() {
        let mut le = LineEdit::new("wr");
        le.set_caret(1); // w|r
        assert_eq!(le.handle_key(&ks("o")), Outcome::Changed);
        assert_eq!(le.text(), "wor");
        assert_eq!(le.caret(), 2);
    }

    #[test]
    fn arrows_move_caret_and_stop_at_edges() {
        let mut le = LineEdit::new("ab");
        assert_eq!(le.caret(), 2);
        le.handle_key(&ks("left"));
        assert_eq!(le.caret(), 1);
        le.handle_key(&ks("left"));
        le.handle_key(&ks("left")); // at 0, stays
        assert_eq!(le.caret(), 0);
        le.handle_key(&ks("right"));
        assert_eq!(le.caret(), 1);
        le.handle_key(&ks("right"));
        le.handle_key(&ks("right")); // at end, stays
        assert_eq!(le.caret(), 2);
    }

    #[test]
    fn backspace_and_delete_work_mid_string() {
        let mut le = LineEdit::new("abc");
        le.set_caret(2); // ab|c
        le.handle_key(&ks("backspace"));
        assert_eq!(le.text(), "ac");
        assert_eq!(le.caret(), 1);
        le.handle_key(&ks("delete"));
        assert_eq!(le.text(), "a");
        assert_eq!(le.caret(), 1);
        // Edge no-ops.
        le.handle_key(&ks("delete"));
        assert_eq!(le.text(), "a");
        le.set_caret(0);
        le.handle_key(&ks("backspace"));
        assert_eq!(le.text(), "a");
    }

    #[test]
    fn utf8_multibyte_navigation_and_deletion() {
        let mut le = LineEdit::new("aéb"); // é is 2 bytes
        assert_eq!(le.caret(), 4);
        le.handle_key(&ks("left")); // over b
        assert_eq!(le.caret(), 3);
        le.handle_key(&ks("left")); // over é
        assert_eq!(le.caret(), 1);
        le.handle_key(&ks("delete")); // delete é
        assert_eq!(le.text(), "ab");
        assert_eq!(le.caret(), 1);
    }

    #[test]
    fn word_navigation_alt_arrows() {
        let mut le = LineEdit::new("feat/chrome-polish");
        le.set_caret(0);
        le.handle_key(&ks_mod("right", false, true, false, false));
        assert_eq!(le.caret(), 4); // after "feat"
        le.handle_key(&ks_mod("right", false, true, false, false));
        assert_eq!(le.caret(), 11); // after "chrome"
        le.handle_key(&ks_mod("left", false, true, false, false));
        assert_eq!(le.caret(), 5); // start of "chrome"
    }

    #[test]
    fn alt_backspace_deletes_word() {
        let mut le = LineEdit::new("new branch");
        le.handle_key(&ks_mod("backspace", false, true, false, false));
        assert_eq!(le.text(), "new ");
        le.handle_key(&ks_mod("backspace", false, true, false, false));
        assert_eq!(le.text(), "");
    }

    #[test]
    fn home_end_and_cmd_arrows() {
        let mut le = LineEdit::new("abc");
        le.handle_key(&ks("home"));
        assert_eq!(le.caret(), 0);
        le.handle_key(&ks("end"));
        assert_eq!(le.caret(), 3);
        le.handle_key(&ks_mod("left", false, false, false, true));
        assert_eq!(le.caret(), 0);
        le.handle_key(&ks_mod("right", false, false, false, true));
        assert_eq!(le.caret(), 3);
        // readline ctrl-a / ctrl-e
        le.handle_key(&ks_mod("a", true, false, false, false));
        assert_eq!(le.caret(), 0);
        le.handle_key(&ks_mod("e", true, false, false, false));
        assert_eq!(le.caret(), 3);
    }

    #[test]
    fn shift_arrows_select_and_typing_replaces() {
        let mut le = LineEdit::new("abcd");
        le.handle_key(&ks_mod("left", false, false, true, false));
        le.handle_key(&ks_mod("left", false, false, true, false));
        assert_eq!(le.selection(), Some((2, 4)));
        assert_eq!(le.selected_text(), Some("cd"));
        assert_eq!(le.handle_key(&ks("x")), Outcome::Changed);
        assert_eq!(le.text(), "abx");
        assert_eq!(le.caret(), 3);
        assert_eq!(le.selection(), None);
    }

    #[test]
    fn plain_arrow_collapses_selection_to_edge() {
        let mut le = LineEdit::new("abcd");
        le.set_caret(1);
        le.handle_key(&ks_mod("right", false, false, true, false));
        le.handle_key(&ks_mod("right", false, false, true, false));
        assert_eq!(le.selection(), Some((1, 3)));
        le.handle_key(&ks("left"));
        assert_eq!(le.caret(), 1);
        assert_eq!(le.selection(), None);
    }

    #[test]
    fn select_all_copy_cut_paste_flow() {
        let mut le = LineEdit::new("hello");
        le.handle_key(&ks_mod("a", false, false, false, true));
        assert_eq!(le.selection(), Some((0, 5)));
        assert_eq!(
            le.handle_key(&ks_mod("c", false, false, false, true)),
            Outcome::Copy("hello".to_string())
        );
        assert_eq!(le.text(), "hello"); // copy doesn't mutate
        assert_eq!(
            le.handle_key(&ks_mod("x", false, false, false, true)),
            Outcome::Cut("hello".to_string())
        );
        assert_eq!(le.text(), "");
        assert_eq!(
            le.handle_key(&ks_mod("v", false, false, false, true)),
            Outcome::Paste
        );
        le.insert_str("world");
        assert_eq!(le.text(), "world");
        assert_eq!(le.caret(), 5);
    }

    #[test]
    fn copy_without_selection_is_ignored() {
        let mut le = LineEdit::new("hello");
        assert_eq!(
            le.handle_key(&ks_mod("c", false, false, false, true)),
            Outcome::Ignored
        );
        assert_eq!(
            le.handle_key(&ks_mod("x", false, false, false, true)),
            Outcome::Ignored
        );
    }

    #[test]
    fn selection_replaced_by_backspace_and_paste() {
        let mut le = LineEdit::new("abcd");
        le.set_caret(1);
        le.handle_key(&ks_mod("right", false, false, true, false));
        le.handle_key(&ks_mod("right", false, false, true, false)); // select "bc"
        le.handle_key(&ks("backspace"));
        assert_eq!(le.text(), "ad");
        assert_eq!(le.caret(), 1);

        le.handle_key(&ks_mod("a", false, false, false, true));
        le.insert_str("zz"); // paste-over-selection path
        assert_eq!(le.text(), "zz");
    }

    #[test]
    fn cmd_backspace_kills_to_line_start() {
        let mut le = LineEdit::new("abcdef");
        le.set_caret(4);
        le.handle_key(&ks_mod("backspace", false, false, false, true));
        assert_eq!(le.text(), "ef");
        assert_eq!(le.caret(), 0);
    }

    #[test]
    fn insert_str_strips_newlines_and_controls() {
        let mut le = LineEdit::new("");
        le.insert_str("git\ncheckout\tmain\r");
        assert_eq!(le.text(), "gitcheckoutmain");
    }

    #[test]
    fn enter_escape_tab_are_ignored_for_caller() {
        let mut le = LineEdit::new("x");
        assert_eq!(le.handle_key(&ks("enter")), Outcome::Ignored);
        assert_eq!(le.handle_key(&ks("escape")), Outcome::Ignored);
        assert_eq!(le.handle_key(&ks("tab")), Outcome::Ignored);
        assert_eq!(le.handle_key(&ks("up")), Outcome::Ignored);
        assert_eq!(le.handle_key(&ks("down")), Outcome::Ignored);
        assert_eq!(le.text(), "x");
    }

    #[test]
    fn set_caret_snaps_to_char_boundary() {
        let mut le = LineEdit::new("aé"); // é bytes 1..3
        le.set_caret(2); // inside é → snaps down to 1
        assert_eq!(le.caret(), 1);
        le.set_caret(99); // clamps to len
        assert_eq!(le.caret(), 3);
    }
}

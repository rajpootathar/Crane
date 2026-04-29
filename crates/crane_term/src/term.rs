//! Terminal core: glues grid + scrollback + mode + cursor and
//! implements the [`Handler`] trait. This is where the
//! TUI-scrollback fix lives — see [`Term::linefeed`].

use crate::cell::{Color, Flags};
use crate::grid::{Cursor, Grid};
use crate::handler::{Handler, ProcessorInput, ScrollDelta};
use crate::mode::TermMode;
use crate::row::Row;
use crate::scrollback::Scrollback;

#[derive(Debug)]
pub struct Term {
    pub grid: Grid,
    pub scrollback: Scrollback,
    pub mode: TermMode,
    /// True while a `?2026` synchronized-output block is being
    /// buffered upstream. Mostly informational — the live-grid
    /// scroll routing does not branch on this; the cursor-position
    /// check in [`Term::linefeed`] is what gates scrollback writes.
    pub in_sync_frame: bool,
    /// Bumped on every grid / scrollback mutation. Crane's PTY
    /// reader thread reads this after each parse pass and only
    /// requests a repaint when it actually changed — avoids the
    /// per-byte repaint storm that the old alacritty-based path
    /// hit with Ink-style TUIs.
    pub dirty_epoch: u64,
    /// DECSC cursor save slot. `None` until the first `save_cursor`.
    pub saved_cursor: Option<Cursor>,
}

impl Term {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            grid: Grid::new(rows, cols),
            scrollback: Scrollback::default(),
            mode: TermMode::default(),
            in_sync_frame: false,
            dirty_epoch: 0,
            saved_cursor: None,
        }
    }

    /// Resize the viewport. Scrollback rows are widened/narrowed to
    /// match so painting stored history at the new width is one
    /// memcpy per row.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let template = self.grid.cursor.template.clone();
        self.grid.resize(rows, cols);
        self.scrollback.resize_columns(cols, &template);
        self.mark_dirty();
    }

    fn mark_dirty(&mut self) {
        self.dirty_epoch = self.dirty_epoch.wrapping_add(1);
    }

    /// Evict the row at the top of the active scroll region into
    /// scrollback and shift the rest up by one. The new bottom row
    /// is reset against the cursor template. Called only by
    /// [`Term::linefeed`] when the cursor sits at scroll-region
    /// bottom — that's the single chokepoint for scrollback writes.
    fn scroll_up_one(&mut self) {
        let region = self.grid.scroll_region.clone();
        if region.is_empty() {
            return;
        }
        let evicted = std::mem::replace(
            &mut self.grid.rows[region.start],
            Row::new(self.grid.columns, &self.grid.cursor.template),
        );
        if !self.mode.contains(TermMode::ALT_SCREEN) {
            self.scrollback.push(evicted);
        }
        for r in region.start..region.end.saturating_sub(1) {
            self.grid.rows.swap(r, r + 1);
        }
        let bottom = region.end.saturating_sub(1);
        if let Some(row) = self.grid.rows.get_mut(bottom) {
            row.reset(&self.grid.cursor.template);
        }
    }

    /// Scroll the active region down by one (rows shift toward the
    /// bottom). Used by `reverse_index` at the top of region and
    /// by explicit `scroll_down`. Never feeds scrollback — those
    /// rows are evicted off the bottom.
    fn scroll_down_one(&mut self) {
        let region = self.grid.scroll_region.clone();
        if region.is_empty() {
            return;
        }
        let bottom = region.end.saturating_sub(1);
        // Reset the bottom row first so the swap chain doesn't
        // carry its previous content upward.
        if let Some(row) = self.grid.rows.get_mut(bottom) {
            row.reset(&self.grid.cursor.template);
        }
        for r in (region.start + 1..region.end).rev() {
            self.grid.rows.swap(r, r - 1);
        }
        if let Some(row) = self.grid.rows.get_mut(region.start) {
            row.reset(&self.grid.cursor.template);
        }
    }

    /// Apply one SGR `Attr` to the cursor template. The template
    /// is the prototype every newly-written cell is cloned from,
    /// so future `input(c)` calls inherit the new style.
    fn apply_attr(&mut self, attr: vte::ansi::Attr) {
        use vte::ansi::Attr;
        let t = &mut self.grid.cursor.template;
        match attr {
            Attr::Reset => {
                t.fg = Color::default();
                t.bg = Color::Named(crate::cell::NamedColor::Background);
                t.flags = Flags::empty();
            }
            Attr::Bold => t.flags.insert(Flags::BOLD),
            Attr::Dim => t.flags.insert(Flags::DIM),
            Attr::Italic => t.flags.insert(Flags::ITALIC),
            Attr::Underline => t.flags.insert(Flags::UNDERLINE),
            Attr::DoubleUnderline => t.flags.insert(Flags::DOUBLE_UNDERLINE),
            // Curly / dotted / dashed underlines collapse to plain
            // underline for now — egui-side renderer handles only
            // straight + double. A v2 enhancement.
            Attr::Undercurl | Attr::DottedUnderline | Attr::DashedUnderline => {
                t.flags.insert(Flags::UNDERLINE);
            }
            Attr::Reverse => t.flags.insert(Flags::INVERSE),
            Attr::Hidden => t.flags.insert(Flags::HIDDEN),
            Attr::Strike => t.flags.insert(Flags::STRIKEOUT),
            Attr::CancelBold => t.flags.remove(Flags::BOLD),
            Attr::CancelBoldDim => t.flags.remove(Flags::BOLD | Flags::DIM),
            Attr::CancelItalic => t.flags.remove(Flags::ITALIC),
            Attr::CancelUnderline => {
                t.flags.remove(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE);
            }
            Attr::CancelReverse => t.flags.remove(Flags::INVERSE),
            Attr::CancelHidden => t.flags.remove(Flags::HIDDEN),
            Attr::CancelStrike => t.flags.remove(Flags::STRIKEOUT),
            Attr::Foreground(c) => t.fg = Color::from_vte(c),
            Attr::Background(c) => t.bg = Color::from_vte(c),
            // Blink and underline-color are accepted but not yet
            // visualized — keeps streams that emit them from
            // looking corrupt.
            Attr::BlinkSlow
            | Attr::BlinkFast
            | Attr::CancelBlink
            | Attr::UnderlineColor(_) => {}
        }
    }

    /// Reset every cell on the current row to the template, range
    /// gated by `mode`. Backs `clear_line`.
    fn clear_line_range(&mut self, mode: vte::ansi::LineClearMode) {
        use vte::ansi::LineClearMode;
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col = self.grid.cursor.col.min(self.grid.columns - 1);
        let template = self.grid.cursor.template.clone();
        let row = match self.grid.rows.get_mut(row_idx) {
            Some(r) => r,
            None => return,
        };
        match mode {
            LineClearMode::Right => {
                for c in row.cells.iter_mut().skip(col) {
                    *c = template.clone();
                }
                row.mark_touched(self.grid.columns.saturating_sub(1));
            }
            LineClearMode::Left => {
                for c in row.cells.iter_mut().take(col + 1) {
                    *c = template.clone();
                }
                row.mark_touched(col);
            }
            LineClearMode::All => {
                for c in row.cells.iter_mut() {
                    *c = template.clone();
                }
                row.mark_touched(self.grid.columns.saturating_sub(1));
            }
        }
    }
}

impl Handler for Term {
    /// THE fix.
    ///
    /// Alacritty pushes a row to scrollback whenever cursor advance
    /// past the visible bottom collides with auto-wrap; that's
    /// correct for streaming stdout but wrong for TUIs that step
    /// down through their own redraw region. We only push when the
    /// cursor sits at the bottom of the active scroll region — a
    /// `cursor-up + LF + LF + LF` redraw lands mid-region and
    /// touches no history.
    fn linefeed(&mut self) -> ScrollDelta {
        let delta = if self.grid.cursor_at_scroll_bottom() {
            self.scroll_up_one();
            ScrollDelta::Up { lines: 1 }
        } else {
            self.grid.cursor.row = self
                .grid
                .cursor
                .row
                .saturating_add(1)
                .min(self.grid.visible_rows.saturating_sub(1));
            ScrollDelta::Zero
        };
        self.mark_dirty();
        delta
    }

    fn carriage_return(&mut self) {
        self.grid.cursor.col = 0;
        self.grid.cursor.input_needs_wrap = false;
    }

    fn newline(&mut self) {
        self.carriage_return();
        let _ = self.linefeed();
    }

    fn input(&mut self, c: char) {
        // DECAWM: when the previous write filled the right margin,
        // defer the wrap until the next character arrives. xterm
        // semantics — without this, "echo $LINE" with a string the
        // exact width of the terminal scrolls early and TUIs
        // mis-position their next paint.
        if self.grid.cursor.input_needs_wrap && self.mode.contains(TermMode::LINE_WRAP) {
            self.carriage_return();
            let _ = self.linefeed();
        }

        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col_idx = self.grid.cursor.col.min(self.grid.columns - 1);
        let template = self.grid.cursor.template.clone();
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            if let Some(cell) = row.cells.get_mut(col_idx) {
                cell.ch = c;
                cell.fg = template.fg;
                cell.bg = template.bg;
                cell.flags = template.flags;
                cell.extra = None;
            }
            row.mark_touched(col_idx);
        }
        if self.grid.cursor.col + 1 >= self.grid.columns {
            self.grid.cursor.col = self.grid.columns - 1;
            self.grid.cursor.input_needs_wrap = true;
        } else {
            self.grid.cursor.col += 1;
            self.grid.cursor.input_needs_wrap = false;
        }
        self.mark_dirty();
    }

    fn backspace(&mut self) {
        if self.grid.cursor.col > 0 {
            self.grid.cursor.col -= 1;
        }
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_up(&mut self, n: usize) {
        let n = n.max(1);
        self.grid.cursor.row = self.grid.cursor.row.saturating_sub(n);
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_down(&mut self, n: usize) {
        let n = n.max(1);
        let last = self.grid.visible_rows.saturating_sub(1);
        self.grid.cursor.row = self.grid.cursor.row.saturating_add(n).min(last);
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_forward(&mut self, n: usize) {
        let n = n.max(1);
        let last = self.grid.columns.saturating_sub(1);
        self.grid.cursor.col = self.grid.cursor.col.saturating_add(n).min(last);
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_backward(&mut self, n: usize) {
        let n = n.max(1);
        self.grid.cursor.col = self.grid.cursor.col.saturating_sub(n);
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_up_and_cr(&mut self, n: usize) {
        self.move_up(n);
        self.grid.cursor.col = 0;
    }

    fn move_down_and_cr(&mut self, n: usize) {
        self.move_down(n);
        self.grid.cursor.col = 0;
    }

    fn goto(&mut self, line: usize, col: usize) {
        self.grid.cursor.row = line.min(self.grid.visible_rows.saturating_sub(1));
        self.grid.cursor.col = col.min(self.grid.columns.saturating_sub(1));
        self.grid.cursor.input_needs_wrap = false;
    }

    fn goto_line(&mut self, line: usize) {
        self.grid.cursor.row = line.min(self.grid.visible_rows.saturating_sub(1));
        self.grid.cursor.input_needs_wrap = false;
    }

    fn goto_col(&mut self, col: usize) {
        self.grid.cursor.col = col.min(self.grid.columns.saturating_sub(1));
        self.grid.cursor.input_needs_wrap = false;
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.grid.cursor.clone());
    }

    fn restore_cursor(&mut self) {
        if let Some(c) = self.saved_cursor.clone() {
            self.grid.cursor = c;
        }
    }

    fn reverse_index(&mut self) -> ScrollDelta {
        // RI: like a reverse linefeed. At the top of the scroll
        // region, scroll the region DOWN one (rows shift toward
        // the bottom). Mid-region, just move the cursor up.
        if self.grid.cursor.row == self.grid.scroll_region.start {
            self.scroll_down_one();
            self.mark_dirty();
            ScrollDelta::Down { lines: 1 }
        } else {
            self.grid.cursor.row = self.grid.cursor.row.saturating_sub(1);
            ScrollDelta::Zero
        }
    }

    fn scroll_up(&mut self, n: usize) -> ScrollDelta {
        let n = n.max(1);
        for _ in 0..n {
            self.scroll_up_one();
        }
        self.mark_dirty();
        ScrollDelta::Up { lines: n }
    }

    fn scroll_down(&mut self, n: usize) -> ScrollDelta {
        let n = n.max(1);
        for _ in 0..n {
            self.scroll_down_one();
        }
        self.mark_dirty();
        ScrollDelta::Down { lines: n }
    }

    fn insert_blank(&mut self, n: usize) {
        let n = n.max(1);
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col = self.grid.cursor.col.min(self.grid.columns - 1);
        let template = self.grid.cursor.template.clone();
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            // Shift cells right of cursor by `n`, dropping the
            // overflow off the right edge. Then fill the gap with
            // template cells.
            let cols = row.cells.len();
            for c in (col + n..cols).rev() {
                row.cells[c] = row.cells[c - n].clone();
            }
            for c in col..(col + n).min(cols) {
                row.cells[c] = template.clone();
            }
            row.mark_touched(cols.saturating_sub(1));
        }
        self.mark_dirty();
    }

    fn erase_chars(&mut self, n: usize) {
        let n = n.max(1);
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col = self.grid.cursor.col.min(self.grid.columns - 1);
        let cols = self.grid.columns;
        let template = self.grid.cursor.template.clone();
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            for c in col..(col + n).min(cols) {
                row.cells[c] = template.clone();
            }
            row.mark_touched((col + n).min(cols).saturating_sub(1));
        }
        self.mark_dirty();
    }

    fn delete_chars(&mut self, n: usize) {
        let n = n.max(1);
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col = self.grid.cursor.col.min(self.grid.columns - 1);
        let cols = self.grid.columns;
        let template = self.grid.cursor.template.clone();
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            for c in col..cols.saturating_sub(n) {
                row.cells[c] = row.cells[c + n].clone();
            }
            for c in cols.saturating_sub(n)..cols {
                row.cells[c] = template.clone();
            }
            row.mark_touched(cols.saturating_sub(1));
        }
        self.mark_dirty();
    }

    fn insert_blank_lines(&mut self, n: usize) -> ScrollDelta {
        let n = n.max(1);
        // CSI L: insert `n` blank lines at the cursor row. Lines at
        // and below the cursor shift down within the scroll region;
        // overflow off the bottom is discarded (no scrollback push).
        if !self.grid.scroll_region.contains(&self.grid.cursor.row) {
            return ScrollDelta::Zero;
        }
        let region = self.grid.scroll_region.clone();
        let cursor_row = self.grid.cursor.row;
        let n = n.min(region.end - cursor_row);
        let template = self.grid.cursor.template.clone();
        // Bubble blank rows down: walk from bottom, swapping.
        for _ in 0..n {
            for r in (cursor_row + 1..region.end).rev() {
                self.grid.rows.swap(r, r - 1);
            }
            if let Some(row) = self.grid.rows.get_mut(cursor_row) {
                row.reset(&template);
            }
        }
        self.mark_dirty();
        ScrollDelta::Down { lines: n }
    }

    fn delete_lines(&mut self, n: usize) -> ScrollDelta {
        let n = n.max(1);
        // CSI M: delete `n` lines starting at cursor row. Lines
        // below shift up; vacated bottom of region fills with
        // template rows. Like `insert_blank_lines`, no scrollback.
        if !self.grid.scroll_region.contains(&self.grid.cursor.row) {
            return ScrollDelta::Zero;
        }
        let region = self.grid.scroll_region.clone();
        let cursor_row = self.grid.cursor.row;
        let n = n.min(region.end - cursor_row);
        let template = self.grid.cursor.template.clone();
        for _ in 0..n {
            for r in cursor_row..region.end.saturating_sub(1) {
                self.grid.rows.swap(r, r + 1);
            }
            if let Some(row) = self.grid.rows.get_mut(region.end - 1) {
                row.reset(&template);
            }
        }
        self.mark_dirty();
        ScrollDelta::Up { lines: n }
    }

    fn clear_line(&mut self, mode: vte::ansi::LineClearMode) {
        self.clear_line_range(mode);
        self.mark_dirty();
    }

    fn clear_screen(&mut self, mode: vte::ansi::ClearMode) {
        use vte::ansi::ClearMode;
        let template = self.grid.cursor.template.clone();
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col = self.grid.cursor.col.min(self.grid.columns - 1);
        match mode {
            ClearMode::Below => {
                // Cursor row: clear from cursor to right margin.
                if let Some(row) = self.grid.rows.get_mut(row_idx) {
                    for c in col..self.grid.columns {
                        row.cells[c] = template.clone();
                    }
                    row.mark_touched(self.grid.columns - 1);
                }
                // Rows below cursor: full reset.
                for r in (row_idx + 1)..self.grid.rows.len() {
                    self.grid.rows[r].reset(&template);
                }
            }
            ClearMode::Above => {
                for r in 0..row_idx {
                    self.grid.rows[r].reset(&template);
                }
                if let Some(row) = self.grid.rows.get_mut(row_idx) {
                    for c in 0..=col {
                        row.cells[c] = template.clone();
                    }
                    row.mark_touched(col);
                }
            }
            ClearMode::All => {
                for r in self.grid.rows.iter_mut() {
                    r.reset(&template);
                }
            }
            ClearMode::Saved => {
                self.scrollback.clear();
            }
        }
        self.mark_dirty();
    }

    fn clear_tabs(&mut self, _mode: vte::ansi::TabulationClearMode) {
        // Tab stops aren't tracked yet — `put_tab` just advances
        // by 8 columns. Stops table lands when a TUI actually
        // exercises CSI g.
    }

    fn put_tab(&mut self, count: u16) {
        let count = count.max(1) as usize;
        for _ in 0..count {
            // Advance to the next 8-column boundary.
            let next = (self.grid.cursor.col / 8 + 1) * 8;
            self.grid.cursor.col = next.min(self.grid.columns.saturating_sub(1));
        }
        self.grid.cursor.input_needs_wrap = false;
    }

    fn terminal_attribute(&mut self, attr: vte::ansi::Attr) {
        self.apply_attr(attr);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bot = bottom
            .unwrap_or(self.grid.visible_rows)
            .min(self.grid.visible_rows);
        let top = top.min(bot);
        if top < bot {
            self.grid.scroll_region = top..bot;
            self.grid.cursor.row = top;
            self.grid.cursor.col = 0;
            self.grid.cursor.input_needs_wrap = false;
        }
    }

    fn set_mode(&mut self, _mode: vte::ansi::Mode) {
        // Plain (non-private) modes are mostly insert / line-feed
        // flavor toggles; left as a stub until we wire up the
        // matching Handler trait methods.
    }

    fn set_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        if let vte::ansi::PrivateMode::Named(named) = mode {
            match named {
                vte::ansi::NamedPrivateMode::ShowCursor => {
                    self.mode |= TermMode::SHOW_CURSOR;
                }
                vte::ansi::NamedPrivateMode::CursorKeys => {
                    self.mode |= TermMode::APP_CURSOR;
                }
                vte::ansi::NamedPrivateMode::LineWrap => {
                    self.mode |= TermMode::LINE_WRAP;
                }
                vte::ansi::NamedPrivateMode::Origin => {
                    self.mode |= TermMode::ORIGIN;
                }
                vte::ansi::NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.mode |= TermMode::ALT_SCREEN | TermMode::ALT_SCREEN_1049;
                    self.save_cursor();
                }
                vte::ansi::NamedPrivateMode::SyncUpdate => {
                    self.mode |= TermMode::SYNC_OUTPUT;
                    self.in_sync_frame = true;
                }
                vte::ansi::NamedPrivateMode::BracketedPaste => {
                    self.mode |= TermMode::BRACKETED_PASTE;
                }
                _ => {}
            }
        }
    }

    fn unset_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        if let vte::ansi::PrivateMode::Named(named) = mode {
            match named {
                vte::ansi::NamedPrivateMode::ShowCursor => {
                    self.mode -= TermMode::SHOW_CURSOR;
                }
                vte::ansi::NamedPrivateMode::CursorKeys => {
                    self.mode -= TermMode::APP_CURSOR;
                }
                vte::ansi::NamedPrivateMode::LineWrap => {
                    self.mode -= TermMode::LINE_WRAP;
                }
                vte::ansi::NamedPrivateMode::Origin => {
                    self.mode -= TermMode::ORIGIN;
                }
                vte::ansi::NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.mode -= TermMode::ALT_SCREEN | TermMode::ALT_SCREEN_1049;
                    self.restore_cursor();
                }
                vte::ansi::NamedPrivateMode::SyncUpdate => {
                    self.mode -= TermMode::SYNC_OUTPUT;
                    self.in_sync_frame = false;
                }
                vte::ansi::NamedPrivateMode::BracketedPaste => {
                    self.mode -= TermMode::BRACKETED_PASTE;
                }
                _ => {}
            }
        }
    }

    fn reset_state(&mut self) {
        let (rows, cols) = (self.grid.visible_rows, self.grid.columns);
        let scrollback = std::mem::take(&mut self.scrollback);
        *self = Self::new(rows, cols);
        self.scrollback = scrollback;
        self.mark_dirty();
    }

    fn on_finish_byte_processing(&mut self, _input: &ProcessorInput) {
        // Frame boundary marker. Renderer hookup lives in Crane's
        // pane_view, not here — `Term` just exposes the grid +
        // scrollback for the painter to read.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Color, NamedColor};
    use vte::ansi::{Attr, ClearMode, LineClearMode};

    /// The actual fix: linefeed in the middle of the scroll region
    /// just moves the cursor down. Scrollback stays empty.
    #[test]
    fn linefeed_mid_region_does_not_evict() {
        let mut t = Term::new(24, 80);
        t.goto(10, 0);
        let delta = t.linefeed();
        assert_eq!(delta, ScrollDelta::Zero);
        assert_eq!(t.grid.cursor.row, 11);
        assert!(t.scrollback.is_empty());
    }

    /// Cursor at scroll-region bottom + LF: the row is evicted and
    /// the rest of the region slides up.
    #[test]
    fn linefeed_at_region_bottom_evicts_row() {
        let mut t = Term::new(24, 80);
        t.goto(23, 0);
        let delta = t.linefeed();
        assert_eq!(delta, ScrollDelta::Up { lines: 1 });
        assert_eq!(t.scrollback.len(), 1);
        assert_eq!(t.grid.cursor.row, 23);
    }

    /// The TUI redraw pattern: `cursor-up N` then `LF` repeated N
    /// times to step back down through the redraw region. Nothing
    /// should land in scrollback.
    #[test]
    fn tui_redraw_does_not_pollute_scrollback() {
        let mut t = Term::new(24, 80);
        for _ in 0..24 {
            t.input('x');
            t.carriage_return();
            if t.grid.cursor.row + 1 < t.grid.visible_rows {
                t.move_down(1);
            }
        }
        let scrollback_before = t.scrollback.len();
        t.move_up(5);
        for _ in 0..5 {
            let delta = t.linefeed();
            assert_eq!(delta, ScrollDelta::Zero);
        }
        assert_eq!(t.scrollback.len(), scrollback_before);
    }

    #[test]
    fn dirty_epoch_bumps_on_input() {
        let mut t = Term::new(10, 20);
        let before = t.dirty_epoch;
        t.input('a');
        assert_ne!(t.dirty_epoch, before);
    }

    #[test]
    fn dirty_epoch_unchanged_on_pure_cursor_move() {
        let mut t = Term::new(10, 20);
        let before = t.dirty_epoch;
        t.move_forward(3);
        t.move_backward(1);
        // Cursor moves don't change visible cells — pure repaint
        // hint should not fire on these.
        assert_eq!(t.dirty_epoch, before);
    }

    #[test]
    fn sgr_bold_red_writes_styled_cell() {
        let mut t = Term::new(5, 10);
        t.terminal_attribute(Attr::Bold);
        t.terminal_attribute(Attr::Foreground(vte::ansi::Color::Named(
            vte::ansi::NamedColor::Red,
        )));
        t.input('A');
        let cell = &t.grid.rows[0].cells[0];
        assert_eq!(cell.ch, 'A');
        assert_eq!(cell.fg, Color::Named(NamedColor::Red));
        assert!(cell.flags.contains(Flags::BOLD));
    }

    #[test]
    fn sgr_reset_clears_flags_and_fg() {
        let mut t = Term::new(5, 10);
        t.terminal_attribute(Attr::Bold);
        t.terminal_attribute(Attr::Italic);
        t.terminal_attribute(Attr::Reset);
        t.input('B');
        let cell = &t.grid.rows[0].cells[0];
        assert!(cell.flags.is_empty());
        assert_eq!(cell.fg, Color::Named(NamedColor::Foreground));
    }

    #[test]
    fn line_wrap_defers_until_next_input() {
        let mut t = Term::new(3, 4);
        // Fill the row exactly.
        for c in "abcd".chars() {
            t.input(c);
        }
        assert_eq!(t.grid.cursor.col, 3);
        assert!(t.grid.cursor.input_needs_wrap);
        // Next char triggers the wrap.
        t.input('e');
        assert_eq!(t.grid.cursor.row, 1);
        assert_eq!(t.grid.cursor.col, 1);
        assert_eq!(t.grid.rows[1].cells[0].ch, 'e');
    }

    #[test]
    fn clear_line_right_clears_only_from_cursor() {
        let mut t = Term::new(3, 5);
        for c in "abcde".chars() {
            t.input(c);
        }
        t.goto(0, 2);
        t.clear_line(LineClearMode::Right);
        assert_eq!(t.grid.rows[0].cells[0].ch, 'a');
        assert_eq!(t.grid.rows[0].cells[1].ch, 'b');
        assert_eq!(t.grid.rows[0].cells[2].ch, ' ');
        assert_eq!(t.grid.rows[0].cells[4].ch, ' ');
    }

    #[test]
    fn clear_screen_all_resets_grid() {
        let mut t = Term::new(3, 5);
        for c in "abcde".chars() {
            t.input(c);
        }
        t.clear_screen(ClearMode::All);
        for row in &t.grid.rows {
            for cell in &row.cells {
                assert_eq!(cell.ch, ' ');
            }
        }
    }

    #[test]
    fn save_restore_cursor_roundtrips() {
        let mut t = Term::new(10, 20);
        t.goto(3, 7);
        t.save_cursor();
        t.goto(0, 0);
        t.restore_cursor();
        assert_eq!(t.grid.cursor.row, 3);
        assert_eq!(t.grid.cursor.col, 7);
    }

    #[test]
    fn delete_lines_does_not_pollute_scrollback() {
        let mut t = Term::new(5, 5);
        for _ in 0..5 {
            for c in "xxxx".chars() {
                t.input(c);
            }
            if t.grid.cursor.row + 1 < t.grid.visible_rows {
                t.carriage_return();
                let _ = t.linefeed();
            }
        }
        let before = t.scrollback.len();
        t.goto(0, 0);
        t.delete_lines(2);
        assert_eq!(t.scrollback.len(), before);
    }

    #[test]
    fn insert_blank_lines_does_not_pollute_scrollback() {
        let mut t = Term::new(5, 5);
        for _ in 0..5 {
            for c in "xxxx".chars() {
                t.input(c);
            }
            if t.grid.cursor.row + 1 < t.grid.visible_rows {
                t.carriage_return();
                let _ = t.linefeed();
            }
        }
        let before = t.scrollback.len();
        t.goto(0, 0);
        t.insert_blank_lines(2);
        assert_eq!(t.scrollback.len(), before);
    }
}

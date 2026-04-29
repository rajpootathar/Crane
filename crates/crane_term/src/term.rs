//! Terminal core: glues grid + scrollback + mode + cursor and
//! implements the [`Handler`] trait. This is where the
//! TUI-scrollback fix lives — see [`Term::linefeed`].

use crate::grid::Grid;
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
}

impl Term {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            grid: Grid::new(rows, cols),
            scrollback: Scrollback::default(),
            mode: TermMode::default(),
            in_sync_frame: false,
        }
    }

    /// Resize the viewport. Scrollback rows are widened/narrowed to
    /// match so painting stored history at the new width is one
    /// memcpy per row.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let template = self.grid.cursor.template.clone();
        self.grid.resize(rows, cols);
        self.scrollback.resize_columns(cols, &template);
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
        // Take the row at the top of the region.
        let evicted = std::mem::replace(
            &mut self.grid.rows[region.start],
            Row::new(self.grid.columns, &self.grid.cursor.template),
        );
        // Only the main screen feeds scrollback. Alt screen never
        // pushes; that's enforced one level up in [`AltScreen`] by
        // bypassing this path.
        if !self.mode.contains(TermMode::ALT_SCREEN) {
            self.scrollback.push(evicted);
        }
        // Slide the rest of the region up.
        for r in region.start..region.end.saturating_sub(1) {
            self.grid.rows.swap(r, r + 1);
        }
        // Reset the new bottom row to the template.
        let bottom = region.end.saturating_sub(1);
        if let Some(row) = self.grid.rows.get_mut(bottom) {
            row.reset(&self.grid.cursor.template);
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
        if self.grid.cursor_at_scroll_bottom() {
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
        }
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
        // v1: write the character at the cursor and step right.
        // Auto-wrap, wide-char handling, and zero-width grapheme
        // accumulation are wired up in subsequent work — gated on
        // `LINE_WRAP` and the `WIDE_CHAR` / `WIDE_CHAR_SPACER`
        // flags. This intentionally minimal impl is enough to
        // exercise the linefeed routing in unit tests.
        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col_idx = self.grid.cursor.col.min(self.grid.columns - 1);
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            if let Some(cell) = row.cells.get_mut(col_idx) {
                cell.ch = c;
                cell.fg = self.grid.cursor.template.fg;
                cell.bg = self.grid.cursor.template.bg;
                cell.flags = self.grid.cursor.template.flags;
            }
            row.mark_touched(col_idx);
        }
        self.grid.cursor.col = self.grid.cursor.col.saturating_add(1);
        if self.grid.cursor.col >= self.grid.columns {
            self.grid.cursor.col = self.grid.columns - 1;
            self.grid.cursor.input_needs_wrap = true;
        }
    }

    fn backspace(&mut self) {
        if self.grid.cursor.col > 0 {
            self.grid.cursor.col -= 1;
        }
        self.grid.cursor.input_needs_wrap = false;
    }

    fn move_up(&mut self, n: usize) {
        self.grid.cursor.row = self.grid.cursor.row.saturating_sub(n);
    }

    fn move_down(&mut self, n: usize) {
        let last = self.grid.visible_rows.saturating_sub(1);
        self.grid.cursor.row = self.grid.cursor.row.saturating_add(n).min(last);
    }

    fn goto(&mut self, line: usize, col: usize) {
        self.grid.cursor.row = line.min(self.grid.visible_rows.saturating_sub(1));
        self.grid.cursor.col = col.min(self.grid.columns.saturating_sub(1));
        self.grid.cursor.input_needs_wrap = false;
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
        }
    }

    fn set_mode(&mut self, mode: vte::ansi::Mode) {
        if let vte::ansi::Mode::Named(_n) = mode {
            // Plain (non-private) modes are mostly insert / line-
            // feed flavor toggles; left as a stub until we wire up
            // the matching Handler trait methods.
        }
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
                vte::ansi::NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.mode |= TermMode::ALT_SCREEN | TermMode::ALT_SCREEN_1049;
                }
                vte::ansi::NamedPrivateMode::SyncUpdate => {
                    self.mode |= TermMode::SYNC_OUTPUT;
                    self.in_sync_frame = true;
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
                vte::ansi::NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.mode -= TermMode::ALT_SCREEN | TermMode::ALT_SCREEN_1049;
                }
                vte::ansi::NamedPrivateMode::SyncUpdate => {
                    self.mode -= TermMode::SYNC_OUTPUT;
                    self.in_sync_frame = false;
                }
                _ => {}
            }
        }
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
        // Simulate prior streaming output that filled the screen.
        for _ in 0..24 {
            t.input('x');
            t.carriage_return();
            // Move cursor explicitly rather than scrolling — the
            // test setup avoids hitting the eviction path here.
            if t.grid.cursor.row + 1 < t.grid.visible_rows {
                t.move_down(1);
            }
        }
        let scrollback_before = t.scrollback.len();
        // Now redraw: cursor up 5, then LF five times stepping back
        // through the rewritten region. Mimics how Ink-based TUIs
        // repaint their UI block.
        t.move_up(5);
        for _ in 0..5 {
            let delta = t.linefeed();
            assert_eq!(delta, ScrollDelta::Zero);
        }
        assert_eq!(t.scrollback.len(), scrollback_before);
    }
}

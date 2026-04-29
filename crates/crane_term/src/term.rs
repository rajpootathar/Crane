//! Terminal core: glues grid + scrollback + mode + cursor and
//! implements the [`Handler`] trait. This is where the
//! TUI-scrollback fix lives — see [`Term::linefeed`].

use crate::cell::{Color, Flags};
use crate::grid::{Cursor, Grid};
use crate::handler::{Handler, ProcessorInput, ScrollDelta};
use crate::index::{Column, Line, Point};
use crate::mode::TermMode;
use crate::row::Row;
use crate::scrollback::Scrollback;
use crate::selection::Selection;

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
    /// Active mouse selection (drag / double-click / triple-click).
    /// Populated by view.rs's input handlers; cleared on click.
    pub selection: Option<Selection>,
    /// Outbound bytes the parser produced as replies to PTY queries
    /// (DSR, DA, title acks, etc.). Drained by the PTY reader thread
    /// via [`Term::take_pty_replies`] after each parse pass.
    pty_replies: Vec<u8>,
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
            selection: None,
            pty_replies: Vec::new(),
        }
    }

    /// Drain accumulated outbound bytes (DSR / DA / title-ack
    /// replies) so the PTY reader thread can write them back to
    /// the master fd. Returns an empty Vec when nothing is queued.
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pty_replies)
    }

    /// Append bytes to the outbound reply queue. Used by the
    /// Handler impl when the parser triggers a query response.
    fn reply(&mut self, bytes: &[u8]) {
        self.pty_replies.extend_from_slice(bytes);
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

    /// Convenience query — `true` when `mode` is set in the bag.
    pub fn mode_contains(&self, mode: TermMode) -> bool {
        self.mode.contains(mode)
    }

    pub fn is_alt_screen(&self) -> bool {
        self.mode.contains(TermMode::ALT_SCREEN)
    }

    pub fn is_app_cursor(&self) -> bool {
        self.mode.contains(TermMode::APP_CURSOR)
    }

    pub fn is_bracketed_paste(&self) -> bool {
        self.mode.contains(TermMode::BRACKETED_PASTE)
    }

    /// Number of rows the user has scrolled up into history. `0`
    /// means the live screen is showing.
    pub fn display_offset(&self) -> usize {
        self.grid.display_offset
    }

    /// Adjust the display offset by `delta`. Positive `delta`
    /// scrolls upward into scrollback; negative scrolls back toward
    /// the live screen. Clamped to `[0, scrollback.len()]`.
    pub fn scroll_display(&mut self, delta: i32) {
        let max = self.scrollback.len();
        let new = if delta >= 0 {
            self.grid
                .display_offset
                .saturating_add(delta as usize)
                .min(max)
        } else {
            self.grid.display_offset.saturating_sub((-delta) as usize)
        };
        if new != self.grid.display_offset {
            self.grid.display_offset = new;
            self.mark_dirty();
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.grid.display_offset != 0 {
            self.grid.display_offset = 0;
            self.mark_dirty();
        }
    }

    /// Iterator over every cell currently presentable, with each
    /// cell paired to its `(line, column)` Point. Includes
    /// scrollback rows when `display_offset > 0`. Live viewport
    /// rows occupy lines `0..visible_rows`; scrollback rows live
    /// at negative line indices, with `-1` being the most recent
    /// row evicted off the top of the live viewport.
    ///
    /// Used by the renderer to walk the visible area in row-major
    /// order without poking into Grid / Scrollback internals.
    pub fn renderable_content(&self) -> RenderableContent<'_> {
        let cursor_line = self.grid.cursor.row as i32 - self.grid.display_offset as i32;
        RenderableContent {
            term: self,
            cursor: RenderableCursor {
                point: Point::new(Line(cursor_line), Column(self.grid.cursor.col)),
                visible: self.mode.contains(TermMode::SHOW_CURSOR),
            },
            display_offset: self.grid.display_offset,
            selection_range: self.selection.as_ref().map(|s| s.to_range()),
            row: 0,
            col: 0,
        }
    }

    /// Plain-text snapshot: every scrollback row, then every
    /// visible row, joined with CRLF, trailing empties trimmed.
    /// Used for session save (which can't replay raw PTY bytes
    /// because shell prompts use absolute-positioning escapes
    /// that are width-baked).
    pub fn snapshot_text(&self) -> String {
        let cap = self.scrollback.len() + self.grid.rows.len();
        let mut out: Vec<String> = Vec::with_capacity(cap);
        for row in self.scrollback.iter() {
            out.push(row_to_text(row));
        }
        for row in self.grid.rows.iter() {
            out.push(row_to_text(row));
        }
        while out.last().is_some_and(|r| r.is_empty()) {
            out.pop();
        }
        out.join("\r\n")
    }

    /// Evict the row at the top of the active scroll region into
    /// scrollback and shift the rest up by one. The new bottom row
    /// is reset against the cursor template. Called only by
    /// [`Term::linefeed`] when the cursor sits at scroll-region
    /// bottom — that's the single chokepoint for scrollback writes.
    ///
    /// **Sync-frame guard**: when `in_sync_frame` is true, the
    /// evicted row is dropped instead of preserved. Inside a
    /// `?2026h ... ?2026l` block, a TUI is repainting its own
    /// region; the rows that fall off the top during the replay
    /// are intermediate state, not real history. Without this
    /// guard, every Ink-style redraw whose last LF lands at the
    /// screen bottom pushes one duplicate row into scrollback —
    /// the exact "duplicate Claude Code splash" artifact tracked
    /// in CLAUDE.md.
    fn scroll_up_one(&mut self) {
        let region = self.grid.scroll_region.clone();
        if region.is_empty() {
            return;
        }
        let evicted = std::mem::replace(
            &mut self.grid.rows[region.start],
            Row::new(self.grid.columns, &self.grid.cursor.template),
        );
        if !self.mode.contains(TermMode::ALT_SCREEN) && !self.in_sync_frame {
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
        use unicode_width::UnicodeWidthChar;
        // Width 0: zero-width / combining mark. Stack onto the
        // previous cell instead of advancing the cursor.
        let width = UnicodeWidthChar::width(c).unwrap_or(1);
        if width == 0 {
            let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
            let col = self
                .grid
                .cursor
                .col
                .saturating_sub(1)
                .min(self.grid.columns.saturating_sub(1));
            if let Some(row) = self.grid.rows.get_mut(row_idx) {
                if let Some(cell) = row.cells.get_mut(col) {
                    cell.push_zero_width(c);
                }
                row.mark_touched(col);
            }
            self.mark_dirty();
            return;
        }

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
        let is_wide = width >= 2;
        // Wide char that would land its second column past the
        // right margin: instead of straddling, wrap first so both
        // halves stay on the next row.
        if is_wide && col_idx + 1 >= self.grid.columns {
            if self.mode.contains(TermMode::LINE_WRAP) {
                self.carriage_return();
                let _ = self.linefeed();
            } else {
                // No wrap mode: clamp to last column with a normal
                // narrow paint to avoid a stray spacer overwriting
                // a real cell off-screen.
                if let Some(row) = self.grid.rows.get_mut(row_idx) {
                    if let Some(cell) = row.cells.get_mut(self.grid.columns - 1) {
                        cell.ch = c;
                        cell.fg = template.fg;
                        cell.bg = template.bg;
                        cell.flags = template.flags;
                        cell.extra = None;
                    }
                }
                self.grid.cursor.input_needs_wrap = true;
                self.mark_dirty();
                return;
            }
        }

        let row_idx = self.grid.cursor.row.min(self.grid.rows.len() - 1);
        let col_idx = self.grid.cursor.col.min(self.grid.columns - 1);
        if let Some(row) = self.grid.rows.get_mut(row_idx) {
            if let Some(cell) = row.cells.get_mut(col_idx) {
                cell.ch = c;
                cell.fg = template.fg;
                cell.bg = template.bg;
                cell.flags = template.flags;
                if is_wide {
                    cell.flags.insert(Flags::WIDE_CHAR);
                }
                cell.extra = None;
            }
            if is_wide {
                if let Some(spacer) = row.cells.get_mut(col_idx + 1) {
                    spacer.ch = ' ';
                    spacer.fg = template.fg;
                    spacer.bg = template.bg;
                    spacer.flags = template.flags;
                    spacer.flags.insert(Flags::WIDE_CHAR_SPACER);
                    spacer.extra = None;
                }
                row.mark_touched(col_idx + 1);
            } else {
                row.mark_touched(col_idx);
            }
        }
        let advance = if is_wide { 2 } else { 1 };
        if self.grid.cursor.col + advance >= self.grid.columns {
            self.grid.cursor.col = self.grid.columns - 1;
            self.grid.cursor.input_needs_wrap = true;
        } else {
            self.grid.cursor.col += advance;
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

    fn device_status(&mut self, n: usize) {
        // CSI 6n: cursor position report — `\e[<row>;<col>R` with
        // 1-based indices. CSI 5n: ready report — `\e[0n`. Only the
        // two DEC-standard queries are answered here; anything else
        // is dropped.
        match n {
            5 => self.reply(b"\x1b[0n"),
            6 => {
                let row = self.grid.cursor.row + 1;
                let col = self.grid.cursor.col + 1;
                let s = format!("\x1b[{};{}R", row, col);
                self.reply(s.as_bytes());
            }
            _ => {}
        }
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        // Primary DA: `\e[?6c` advertises VT102. Secondary DA
        // (intermediate `>`): `\e[>0;0;0c` reports terminal type 0
        // / firmware 0. Tertiary DA (`=`): not supported, ignored.
        // Most TUIs only check the primary form; matching alacritty
        // behavior is close enough.
        match intermediate {
            None => self.reply(b"\x1b[?6c"),
            Some('>') => self.reply(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }

    fn set_sync_frame(&mut self, active: bool) {
        self.in_sync_frame = active;
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
    fn wide_char_occupies_two_columns_with_spacer() {
        let mut t = Term::new(3, 10);
        // CJK ideograph — full-width.
        t.input('文');
        let row = &t.grid.rows[0];
        assert_eq!(row.cells[0].ch, '文');
        assert!(row.cells[0].flags.contains(Flags::WIDE_CHAR));
        assert!(row.cells[1].flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(t.grid.cursor.col, 2);
    }

    #[test]
    fn zero_width_combining_mark_stacks_onto_previous_cell() {
        let mut t = Term::new(3, 10);
        t.input('e');
        // Combining acute accent (U+0301).
        t.input('\u{0301}');
        let cell = &t.grid.rows[0].cells[0];
        assert_eq!(cell.ch, 'e');
        assert!(cell.extra.is_some());
        assert_eq!(cell.extra.as_ref().unwrap().zero_width, vec!['\u{0301}']);
        // Cursor stayed on column 1 — no advance for the combiner.
        assert_eq!(t.grid.cursor.col, 1);
    }

    #[test]
    fn snapshot_text_reads_visible_grid() {
        let mut t = Term::new(3, 10);
        for c in "hello".chars() {
            t.input(c);
        }
        t.carriage_return();
        let _ = t.linefeed();
        for c in "world".chars() {
            t.input(c);
        }
        let s = t.snapshot_text();
        assert!(s.starts_with("hello"));
        assert!(s.contains("world"));
    }

    #[test]
    fn scroll_display_clamps_to_scrollback_size() {
        let mut t = Term::new(2, 5);
        t.scroll_display(10);
        // No scrollback yet — clamps to 0.
        assert_eq!(t.display_offset(), 0);
        // Generate scrollback by overflowing the grid bottom.
        for _ in 0..5 {
            t.goto(1, 0);
            let _ = t.linefeed();
        }
        assert!(t.scrollback.len() > 0);
        t.scroll_display(2);
        assert_eq!(t.display_offset(), 2);
        t.scroll_to_bottom();
        assert_eq!(t.display_offset(), 0);
    }

    #[test]
    fn device_status_5_replies_ready() {
        let mut t = Term::new(5, 10);
        t.device_status(5);
        assert_eq!(t.take_pty_replies(), b"\x1b[0n");
    }

    #[test]
    fn device_status_6_replies_cursor_position() {
        let mut t = Term::new(5, 10);
        t.goto(2, 4);
        t.device_status(6);
        // 1-based; row 2 col 4 → "\e[3;5R".
        assert_eq!(t.take_pty_replies(), b"\x1b[3;5R");
    }

    #[test]
    fn identify_terminal_replies_vt102() {
        let mut t = Term::new(5, 10);
        t.identify_terminal(None);
        assert_eq!(t.take_pty_replies(), b"\x1b[?6c");
    }

    #[test]
    fn renderable_content_walks_visible_cells() {
        let mut t = Term::new(2, 3);
        t.input('a');
        t.input('b');
        t.input('c');
        let cells: Vec<_> = t.renderable_content().collect();
        assert_eq!(cells.len(), 6); // 2 rows × 3 cols
        assert_eq!(cells[0].point.line.0, 0);
        assert_eq!(cells[0].point.column.0, 0);
        assert_eq!(cells[0].cell.ch, 'a');
        assert_eq!(cells[2].cell.ch, 'c');
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

/// Where a renderable cell lives: viewport (live grid row index)
/// or scrollback (negative line index relative to the live grid).
#[derive(Clone, Copy, Debug)]
pub struct RenderableCursor {
    pub point: Point,
    pub visible: bool,
}

/// Iterator returned by [`Term::renderable_content`]. Walks the
/// visible viewport row-major, sourcing rows from scrollback while
/// the user has scrolled up and from the live grid below the
/// scrollback portion.
pub struct RenderableContent<'a> {
    term: &'a Term,
    pub cursor: RenderableCursor,
    pub display_offset: usize,
    pub selection_range: Option<crate::selection::SelectionRange>,
    row: usize,
    col: usize,
}

/// One element of [`RenderableContent`]. Mirrors alacritty's
/// shape so the renderer in `src/terminal/view.rs` can swap to
/// crane_term with minimal rewriting.
#[derive(Clone, Debug)]
pub struct RenderableCell<'a> {
    pub point: Point,
    pub cell: &'a crate::cell::Cell,
}

impl<'a> Iterator for RenderableContent<'a> {
    type Item = RenderableCell<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let cols = self.term.grid.columns;
        let visible_rows = self.term.grid.visible_rows;
        loop {
            if self.col >= cols {
                self.col = 0;
                self.row += 1;
            }
            if self.row >= visible_rows {
                return None;
            }
            // Map presentation row → either scrollback (when the
            // user has scrolled up) or live grid row.
            //
            // `display_offset` rows of scrollback show at the top
            // of the viewport. Their line numbers go negative so
            // selection / cursor math can address the same point
            // without a separate "scrollback row" coordinate.
            let line: i32 = self.row as i32 - self.display_offset as i32;
            let cell = if line >= 0 {
                self.term.grid.cell_at(line as usize, self.col)
            } else {
                // -1 is the most recent scrollback row. Index from
                // the back of the deque.
                let from_back = (-line) as usize;
                let idx = self
                    .term
                    .scrollback
                    .len()
                    .checked_sub(from_back);
                idx.and_then(|i| {
                    self.term
                        .scrollback
                        .iter()
                        .nth(i)
                        .and_then(|r| r.cells.get(self.col))
                })
            };
            let col_idx = self.col;
            self.col += 1;
            if let Some(cell) = cell {
                return Some(RenderableCell {
                    point: Point::new(Line(line), Column(col_idx)),
                    cell,
                });
            }
            // Out-of-history hole — render nothing for this cell
            // (just advance). The renderer fills empty space with
            // the theme background, so a None mid-iter means the
            // viewport row is shorter than the live grid (only
            // happens when scrollback is shallower than
            // display_offset, which is clamped by scroll_display
            // to never happen in normal flow).
        }
    }
}

/// Convert one row to plain text. `occ` bounds the scan so empty
/// tail cells don't get emitted as trailing spaces. Wide-char
/// spacers are skipped — their glyph is owned by the preceding
/// `WIDE_CHAR` cell. Trailing whitespace is trimmed.
fn row_to_text(row: &crate::row::Row) -> String {
    let bound = row.occ.min(row.cells.len());
    let mut s = String::with_capacity(bound);
    for cell in row.cells.iter().take(bound) {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let ch = cell.ch;
        s.push(if ch == '\0' { ' ' } else { ch });
    }
    while s.ends_with(' ') {
        s.pop();
    }
    s
}

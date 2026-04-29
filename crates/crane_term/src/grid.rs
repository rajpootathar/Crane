//! Live viewport grid.
//!
//! Holds the visible rows, the cursor, and the active scroll region.
//! Methods here are the *mechanics* of grid mutation; the policy
//! decisions (what hits scrollback, what just moves the cursor) live
//! one level up in [`crate::term::Term`].

use crate::cell::Cell;
use crate::row::Row;
use std::ops::Range;

/// Cursor state for the live viewport. SGR foreground / background /
/// flag bits the parser is currently writing get cached here so each
/// `input(c)` call doesn't re-derive them.
#[derive(Clone, Debug)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    /// One past the last column we wrote. `LINE_WRAP` mode uses
    /// this to defer the wrap until the *next* `input` arrives —
    /// matches xterm semantics.
    pub input_needs_wrap: bool,
    /// Active SGR foreground.
    pub template: Cell,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            input_needs_wrap: false,
            template: Cell::default(),
        }
    }
}

#[derive(Debug)]
pub struct Grid {
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    /// Inclusive top, exclusive bottom — rows the scroll-affecting
    /// operations are confined to. Defaults to the full grid.
    pub scroll_region: Range<usize>,
    pub columns: usize,
    pub visible_rows: usize,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        let template = Cell::default();
        let row_vec = (0..rows).map(|_| Row::new(cols, &template)).collect();
        Self {
            rows: row_vec,
            cursor: Cursor::default(),
            scroll_region: 0..rows,
            columns: cols,
            visible_rows: rows,
        }
    }

    /// Resize the viewport. Reflow is intentionally simple in v1 —
    /// rows are padded / truncated to the new width and the visible
    /// row count grows / shrinks from the bottom. Reflow that keeps
    /// wrapped content aligned across reshapes is v2 work.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let template = self.cursor.template.clone();
        for r in self.rows.iter_mut() {
            r.resize(cols, &template);
        }
        if rows > self.rows.len() {
            for _ in self.rows.len()..rows {
                self.rows.push(Row::new(cols, &template));
            }
        } else {
            self.rows.truncate(rows);
        }
        self.columns = cols;
        self.visible_rows = rows;
        self.scroll_region = 0..rows;
        if self.cursor.row >= rows {
            self.cursor.row = rows.saturating_sub(1);
        }
        if self.cursor.col >= cols {
            self.cursor.col = cols.saturating_sub(1);
        }
    }

    /// Bottom row index of the active scroll region (inclusive).
    pub fn scroll_bottom(&self) -> usize {
        self.scroll_region.end.saturating_sub(1)
    }

    /// True when the cursor sits at the bottom of the active scroll
    /// region. The fix for the TUI scrollback bug pivots on this:
    /// `linefeed` only evicts a row to scrollback when this is true.
    pub fn cursor_at_scroll_bottom(&self) -> bool {
        self.cursor.row == self.scroll_bottom()
    }
}

//! Capped FIFO of evicted rows.
//!
//! v1 storage is a simple `VecDeque<Row>`. A flat-storage layout
//! (UTF-8 byte buffer + interval maps for attributes) would be a v2
//! memory optimization for very long histories — skipped here in
//! favor of straightforward per-row storage that maps cleanly to
//! the renderer's per-row painter loop.

use crate::row::Row;
use std::collections::VecDeque;

/// Default cap. Large enough for typical interactive sessions.
pub const DEFAULT_MAX_ROWS: usize = 10_000;

#[derive(Debug)]
pub struct Scrollback {
    rows: VecDeque<Row>,
    max_rows: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_MAX_ROWS)
    }
}

impl Scrollback {
    pub fn with_capacity(max_rows: usize) -> Self {
        Self {
            rows: VecDeque::with_capacity(max_rows.min(1024)),
            max_rows,
        }
    }

    /// Push a row onto the bottom (most recent) end. Evicts the
    /// oldest row when at capacity.
    pub fn push(&mut self, row: Row) {
        if self.rows.len() >= self.max_rows {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    /// Resize each retained row to `cols` columns. Called when the
    /// terminal viewport resizes — keeps stored rows wide enough
    /// for the new viewport without losing content.
    pub fn resize_columns(&mut self, cols: usize, template: &crate::cell::Cell) {
        for r in self.rows.iter_mut() {
            r.resize(cols, template);
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }
}

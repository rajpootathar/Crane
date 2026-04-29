//! Row of cells with a dirty upper bound.
//!
//! `occ` is the largest column index the row has ever been touched
//! at, plus one. Cells from `occ..len` are guaranteed to equal the
//! template that the row was reset against, so dirty iteration and
//! redraw can stop at `occ` and skip the empty tail.

use crate::cell::Cell;

#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Upper bound on touched columns. `cells[occ..]` is template-
    /// equal. Reset by [`Row::reset`].
    pub occ: usize,
}

impl Row {
    pub fn new(columns: usize, template: &Cell) -> Self {
        Self {
            cells: vec![template.clone(); columns],
            occ: 0,
        }
    }

    /// Reset every column to `template` and clear the dirty bound.
    /// Used when scrolling pulls a row back into view.
    pub fn reset(&mut self, template: &Cell) {
        for c in self.cells.iter_mut().take(self.occ) {
            *c = template.clone();
        }
        self.occ = 0;
    }

    /// Mark the row as dirty up to (and including) column `col`.
    /// Should be called by every code path that mutates a cell in
    /// place — keeps `occ` honest.
    pub fn mark_touched(&mut self, col: usize) {
        let bound = col.saturating_add(1).min(self.cells.len());
        if bound > self.occ {
            self.occ = bound;
        }
    }

    /// Resize the row to `cols` columns, padding with `template` on
    /// growth. Used when the terminal viewport resizes.
    pub fn resize(&mut self, cols: usize, template: &Cell) {
        if cols > self.cells.len() {
            self.cells.resize(cols, template.clone());
        } else if cols < self.cells.len() {
            self.cells.truncate(cols);
            if self.occ > cols {
                self.occ = cols;
            }
        }
    }
}

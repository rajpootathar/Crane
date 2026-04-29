//! Grid coordinate types.
//!
//! Mirrors alacritty's `index` module shape so the renderer in
//! `src/terminal/view.rs` can address grid cells with the same
//! pattern. `Line` is signed because rows in scrollback live at
//! negative indices relative to the live viewport.

use std::ops::{Add, Sub};

/// A grid row index. Live viewport rows are `[0, screen_lines)`;
/// scrollback rows live at `-1, -2, ...` so the renderer can address
/// the whole presentable grid (scrollback + viewport) with a single
/// signed iteration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Line(pub i32);

/// A grid column index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Column(pub usize);

impl Add<usize> for Column {
    type Output = Column;
    fn add(self, rhs: usize) -> Column {
        Column(self.0 + rhs)
    }
}

impl Sub<usize> for Column {
    type Output = Column;
    fn sub(self, rhs: usize) -> Column {
        Column(self.0.saturating_sub(rhs))
    }
}

impl Add<i32> for Line {
    type Output = Line;
    fn add(self, rhs: i32) -> Line {
        Line(self.0 + rhs)
    }
}

impl Sub<i32> for Line {
    type Output = Line;
    fn sub(self, rhs: i32) -> Line {
        Line(self.0 - rhs)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: Line,
    pub column: Column,
}

impl Point {
    pub fn new(line: Line, column: Column) -> Self {
        Self { line, column }
    }
}

/// Which side of a column a click landed on. Selection drag uses
/// this so anchoring against the right side of the previous cell
/// renders the same selection rectangle as anchoring against the
/// left side of the next cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

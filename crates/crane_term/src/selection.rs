//! Mouse selection state.
//!
//! Anchor + active end with a side hint, `update()` drag,
//! `to_range()` materialization, point-in-range containment via
//! `SelectionRange`. Block selection (column-constrained) is a
//! separate kind so dragging through TUI sidebar dividers doesn't
//! pull adjacent column text along with the intended selection.

use crate::index::{Point, Side};

#[cfg(test)]
use crate::index::Line;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionType {
    /// Word-wrap-aware range from anchor to active end.
    Simple,
    /// Column-constrained rectangle.
    Block,
    /// Word-boundary expanded — set by double-click.
    Semantic,
    /// Whole-line expanded — set by triple-click.
    Lines,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionAnchor {
    pub point: Point,
    pub side: Side,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub kind: SelectionType,
    pub anchor: SelectionAnchor,
    pub active: SelectionAnchor,
}

impl Selection {
    pub fn new(kind: SelectionType, point: Point, side: Side) -> Self {
        let a = SelectionAnchor { point, side };
        Self {
            kind,
            anchor: a,
            active: a,
        }
    }

    /// Drag — move the active end to a new (point, side) pair.
    pub fn update(&mut self, point: Point, side: Side) {
        self.active = SelectionAnchor { point, side };
    }

    /// True when the selection has zero coverage (anchor == active
    /// on the same side).
    pub fn is_empty(&self) -> bool {
        self.anchor.point == self.active.point && self.anchor.side == self.active.side
    }

    /// Materialize the inclusive start/end pair, normalized so that
    /// `start <= end` regardless of which way the user dragged.
    pub fn to_range(&self) -> SelectionRange {
        let (mut start, mut end) = if self.anchor.point <= self.active.point {
            (self.anchor, self.active)
        } else {
            (self.active, self.anchor)
        };
        // Side hint: `Left` on the start anchor means the click
        // landed on the left half of the start column, so the
        // selection includes that column. `Right` on the end anchor
        // means the click landed on the right half of the end
        // column, so the selection includes the end column too.
        if start.side == Side::Right {
            // Start clicked on the right half — column NOT included;
            // bump start one column to the right.
            start.point.column.0 = start.point.column.0.saturating_add(1);
        }
        if end.side == Side::Left {
            // End clicked on the left half — column NOT included;
            // pull end back one column.
            end.point.column.0 = end.point.column.0.saturating_sub(1);
        }
        SelectionRange {
            start: start.point,
            end: end.point,
            is_block: matches!(self.kind, SelectionType::Block),
        }
    }
}

/// Materialized selection rectangle for the renderer.
#[derive(Clone, Copy, Debug)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

impl SelectionRange {
    /// True when `point` falls inside this range. For non-block
    /// selections the test is row-major: any column on intermediate
    /// rows is covered, with start/end rows clipped at the anchor
    /// columns. For block selections the test clips both row and
    /// column to the rectangle.
    pub fn contains(&self, point: Point) -> bool {
        if point.line < self.start.line || point.line > self.end.line {
            return false;
        }
        if self.is_block {
            let lo = self.start.column.0.min(self.end.column.0);
            let hi = self.start.column.0.max(self.end.column.0);
            return point.column.0 >= lo && point.column.0 <= hi;
        }
        if point.line == self.start.line && point.line == self.end.line {
            // Single-row selection — clip both bounds.
            point.column >= self.start.column && point.column <= self.end.column
        } else if point.line == self.start.line {
            point.column >= self.start.column
        } else if point.line == self.end.line {
            point.column <= self.end.column
        } else {
            true
        }
    }
}

/// Helper used by the `Lines` selection kind. Expands an anchor to
/// cover the full row.
pub fn expand_to_line(point: Point, columns: usize) -> SelectionRange {
    SelectionRange {
        start: Point::new(point.line, crate::index::Column(0)),
        end: Point::new(point.line, crate::index::Column(columns.saturating_sub(1))),
        is_block: false,
    }
}

/// Helper used by the `Semantic` (double-click) selection kind.
/// Walks the row left and right from the click point until a
/// non-word character is hit. ASCII alphanumerics + `_` are word
/// chars; tweak as needed when extending to more locales.
pub fn expand_to_word<F>(point: Point, columns: usize, char_at: F) -> SelectionRange
where
    F: Fn(usize) -> char,
{
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut left = point.column.0;
    while left > 0 && is_word(char_at(left.saturating_sub(1))) {
        left -= 1;
    }
    let mut right = point.column.0;
    while right + 1 < columns && is_word(char_at(right + 1)) {
        right += 1;
    }
    SelectionRange {
        start: Point::new(point.line, crate::index::Column(left)),
        end: Point::new(point.line, crate::index::Column(right)),
        is_block: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Column;

    fn pt(line: i32, col: usize) -> Point {
        Point::new(Line(line), Column(col))
    }

    #[test]
    fn drag_normalizes_start_before_end() {
        // Drag left-to-right: anchor (0,2) → active (0,5).
        let mut sel = Selection::new(SelectionType::Simple, pt(0, 2), Side::Left);
        sel.update(pt(0, 5), Side::Right);
        let r = sel.to_range();
        assert_eq!(r.start.column.0, 2);
        assert_eq!(r.end.column.0, 5);
    }

    #[test]
    fn drag_backward_still_normalizes() {
        // Drag right-to-left: anchor (0,5) → active (0,2).
        let mut sel = Selection::new(SelectionType::Simple, pt(0, 5), Side::Right);
        sel.update(pt(0, 2), Side::Left);
        let r = sel.to_range();
        assert!(r.start <= r.end);
    }

    #[test]
    fn contains_covers_intermediate_rows() {
        let r = SelectionRange {
            start: pt(0, 5),
            end: pt(2, 3),
            is_block: false,
        };
        assert!(r.contains(pt(1, 0)));
        assert!(r.contains(pt(1, 100)));
        assert!(r.contains(pt(0, 5)));
        assert!(!r.contains(pt(0, 4)));
        assert!(r.contains(pt(2, 3)));
        assert!(!r.contains(pt(2, 4)));
    }

    #[test]
    fn block_selection_clips_columns_per_row() {
        let r = SelectionRange {
            start: pt(0, 2),
            end: pt(3, 5),
            is_block: true,
        };
        assert!(r.contains(pt(1, 2)));
        assert!(r.contains(pt(1, 5)));
        assert!(!r.contains(pt(1, 1)));
        assert!(!r.contains(pt(1, 6)));
    }
}

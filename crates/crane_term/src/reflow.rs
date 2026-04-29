//! Resize reflow.
//!
//! Without reflow, resizing a terminal leaves rows truncated (on
//! shrink) or padded with template cells (on grow). Multi-resize
//! sequences therefore garble historic content. With reflow:
//!
//! 1. Adjacent physical rows joined by [`Flags::WRAPLINE`] on the
//!    last cell are walked as one **logical line**.
//! 2. Each logical line is materialized as a flat sequence of
//!    cells (visible characters only — trailing template padding
//!    is dropped).
//! 3. The flat sequence is re-wrapped at `new_cols`. Physical
//!    rows are emitted; all but the last carry `WRAPLINE` on
//!    their last cell.
//! 4. New rows replace the old grid + scrollback, with the
//!    cursor's logical position translated to a new physical
//!    `(row, col)` so typed text lands in the same place.
//!
//! Wide chars: a `WIDE_CHAR` cell + its `WIDE_CHAR_SPACER` cell
//! are treated as one logical glyph occupying two columns. The
//! pair is never split across a wrap boundary.
//!
//! Scope: this v1 only reflows the **live grid**. Scrollback rows
//! pass through with a column-only resize (truncate or pad). A
//! v2 could collect logical lines across the scrollback / live
//! boundary too, but the v1 covers the user-visible bug — typing
//! at the cursor after a resize lands where the user expects.

use crate::cell::{Cell, Flags};
use crate::grid::{Cursor, Grid};
use crate::row::Row;

/// One logical line: a flat sequence of cells with the cursor's
/// position recorded if it falls inside this line.
struct LogicalLine {
    cells: Vec<Cell>,
    /// `Some(idx)` when the cursor's column inside the original
    /// physical layout maps to this line's `idx`-th cell. `None`
    /// when the cursor isn't in this line.
    cursor_at: Option<usize>,
}

/// Re-wrap the live grid to `new_cols × new_rows`. Returns the
/// updated cursor and the rows in render order; callers replace
/// the grid + cursor with these.
pub fn reflow_grid(
    rows: &[Row],
    cursor: &Cursor,
    new_cols: usize,
    new_rows: usize,
    template: &Cell,
) -> ReflowResult {
    let lines = collect_logical_lines(rows, cursor);
    let (mut wrapped, new_cursor_pos) = rewrap_lines(&lines, new_cols, template);

    // Pad to `new_rows` if the rewrap produced fewer; truncate
    // from the top (push to "overflow") if it produced more —
    // those overflow rows go to scrollback.
    let mut overflow: Vec<Row> = Vec::new();
    while wrapped.len() > new_rows {
        overflow.push(wrapped.remove(0));
    }
    while wrapped.len() < new_rows {
        wrapped.push(Row::new(new_cols, template));
    }

    // Clamp the cursor to the new dimensions. `new_cursor_pos`
    // already accounts for the rewrap; we only adjust here for the
    // case where the cursor's logical line landed in the overflow.
    let (mut cur_row, cur_col) = new_cursor_pos.unwrap_or((0, 0));
    let overflow_len = overflow.len();
    if cur_row >= overflow_len {
        cur_row -= overflow_len;
    } else {
        cur_row = 0;
    }
    let cur_row = cur_row.min(new_rows.saturating_sub(1));
    let cur_col = cur_col.min(new_cols.saturating_sub(1));

    ReflowResult {
        rows: wrapped,
        overflow_to_scrollback: overflow,
        cursor_row: cur_row,
        cursor_col: cur_col,
    }
}

pub struct ReflowResult {
    /// Reflowed live rows, exactly `new_rows` long.
    pub rows: Vec<Row>,
    /// Rows that fell off the top during reflow — push these to
    /// scrollback in order (oldest first).
    pub overflow_to_scrollback: Vec<Row>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

/// Walk physical rows, joining via `WRAPLINE` on the last cell of
/// the previous row. Records the cursor's flat position inside
/// the line that contains it.
fn collect_logical_lines(rows: &[Row], cursor: &Cursor) -> Vec<LogicalLine> {
    let mut lines: Vec<LogicalLine> = Vec::new();
    let mut current = LogicalLine {
        cells: Vec::new(),
        cursor_at: None,
    };
    let mut continued = false;
    for (row_idx, row) in rows.iter().enumerate() {
        // Materialize visible cells: walk up to `occ`, but trim
        // template-equivalent trailing cells so a half-empty row
        // doesn't leak its padding into the next wrap.
        let bound = row.occ.min(row.cells.len());
        let mut row_cells: Vec<Cell> = row.cells.iter().take(bound).cloned().collect();
        // Strip trailing whitespace cells UNLESS this row wraps —
        // a wrapped row ends at the right margin by definition,
        // so its tail isn't padding.
        let last_wraps = row
            .cells
            .last()
            .map(|c| c.flags.contains(Flags::WRAPLINE))
            .unwrap_or(false);
        if !last_wraps {
            while row_cells
                .last()
                .map(|c| c.ch == ' ' && c.flags.is_empty())
                .unwrap_or(false)
            {
                row_cells.pop();
            }
        }
        // Cursor mapping: if the cursor's row equals this physical
        // row, the cursor's flat position is current.cells.len() +
        // cursor.col (clamped to the actual material length).
        if cursor.row == row_idx {
            let flat = current.cells.len() + cursor.col;
            current.cursor_at = Some(flat);
        }
        current.cells.extend(row_cells);

        if last_wraps {
            // Continue: next row joins this logical line.
            continued = true;
        } else {
            lines.push(std::mem::replace(
                &mut current,
                LogicalLine {
                    cells: Vec::new(),
                    cursor_at: None,
                },
            ));
            continued = false;
        }
    }
    if continued || !current.cells.is_empty() || current.cursor_at.is_some() {
        lines.push(current);
    }
    // Trim trailing empty logical lines that didn't carry the
    // cursor — they're "unused future rows" of the live grid, not
    // intentional blank lines in scrolled-up output. Without this,
    // a single content row + empty trailing rows generates
    // (content_row + N empty) logical lines, which can produce
    // more reflowed rows than the new viewport holds.
    while let Some(last) = lines.last() {
        if last.cells.is_empty() && last.cursor_at.is_none() {
            lines.pop();
        } else {
            break;
        }
    }
    lines
}

/// Re-wrap each logical line to `new_cols`-wide physical rows.
/// Returns the new rows and the cursor's `(row_idx, col)` position
/// in the new layout, or `None` if no logical line had the cursor.
fn rewrap_lines(
    lines: &[LogicalLine],
    new_cols: usize,
    template: &Cell,
) -> (Vec<Row>, Option<(usize, usize)>) {
    let mut out: Vec<Row> = Vec::new();
    let mut cursor_pos: Option<(usize, usize)> = None;

    for line in lines {
        if line.cells.is_empty() {
            // Empty line — emit one empty row.
            let row = Row::new(new_cols, template);
            if line.cursor_at == Some(0) {
                cursor_pos = Some((out.len(), 0));
            }
            out.push(row);
            continue;
        }
        let mut idx = 0;
        let mut first_in_line = true;
        while idx < line.cells.len() {
            // Decide how many logical cells fit in one physical
            // row at new_cols width. Wide-char pairs (WIDE_CHAR +
            // WIDE_CHAR_SPACER) count as 2 cols and are never
            // split.
            let mut cols_used = 0usize;
            let chunk_start = idx;
            while idx < line.cells.len() {
                let cell = &line.cells[idx];
                let glyph_cols =
                    if cell.flags.contains(Flags::WIDE_CHAR) {
                        2
                    } else if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        // Spacer should be paired with its WIDE_CHAR
                        // immediately before it. If we already
                        // consumed the WIDE_CHAR, just skip the
                        // spacer (it's already accounted for).
                        idx += 1;
                        continue;
                    } else {
                        1
                    };
                if cols_used + glyph_cols > new_cols {
                    break;
                }
                cols_used += glyph_cols;
                idx += 1;
                // For wide chars, also consume the spacer.
                if cell.flags.contains(Flags::WIDE_CHAR)
                    && idx < line.cells.len()
                    && line.cells[idx].flags.contains(Flags::WIDE_CHAR_SPACER)
                {
                    idx += 1;
                }
            }

            let mut row = Row::new(new_cols, template);
            // Copy the chunk into row.cells[0..]. Zero-width
            // graphemes ride on cell.extra so a plain clone is
            // enough.
            for (i, cell) in line.cells[chunk_start..idx].iter().enumerate() {
                if i < new_cols {
                    row.cells[i] = cell.clone();
                }
            }
            row.occ = idx - chunk_start;

            // If this isn't the last chunk for the line, mark
            // wrap continuation on the last cell.
            let line_continues = idx < line.cells.len();
            if line_continues {
                if let Some(last) = row.cells.last_mut() {
                    last.flags.insert(Flags::WRAPLINE);
                }
            } else {
                if let Some(last) = row.cells.last_mut() {
                    last.flags.remove(Flags::WRAPLINE);
                }
            }

            // Cursor mapping: if the cursor's flat position falls
            // inside this chunk, record its physical (row, col).
            if first_in_line && cursor_pos.is_none() {
                if let Some(flat) = line.cursor_at {
                    if flat >= chunk_start && flat <= idx {
                        let col_offset = flat - chunk_start;
                        cursor_pos = Some((out.len(), col_offset.min(new_cols - 1)));
                    }
                }
            } else if cursor_pos.is_none() {
                if let Some(flat) = line.cursor_at {
                    if flat >= chunk_start && flat <= idx {
                        let col_offset = flat - chunk_start;
                        cursor_pos = Some((out.len(), col_offset.min(new_cols - 1)));
                    }
                }
            }
            first_in_line = false;

            out.push(row);
        }
    }

    (out, cursor_pos)
}

#[allow(dead_code)]
pub(crate) fn debug_grid(grid: &Grid) -> String {
    let mut s = String::new();
    for (r, row) in grid.rows.iter().enumerate() {
        s.push_str(&format!("row {}: ", r));
        let bound = row.occ.min(row.cells.len());
        for cell in row.cells.iter().take(bound) {
            s.push(if cell.ch == '\0' { ' ' } else { cell.ch });
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;

    #[test]
    fn no_wrap_lines_passthrough() {
        // 3 rows × 10 cols, content that fits.
        let template = Cell::default();
        let mut rows = vec![Row::new(10, &template); 3];
        let put = |row: &mut Row, s: &str| {
            for (i, c) in s.chars().enumerate() {
                row.cells[i].ch = c;
            }
            row.occ = s.chars().count();
        };
        put(&mut rows[0], "hello");
        put(&mut rows[1], "world");
        let cursor = Cursor::default();
        let result = reflow_grid(&rows, &cursor, 10, 3, &template);
        let r0: String = result.rows[0]
            .cells
            .iter()
            .take(result.rows[0].occ)
            .map(|c| c.ch)
            .collect();
        assert_eq!(r0, "hello");
        let r1: String = result.rows[1]
            .cells
            .iter()
            .take(result.rows[1].occ)
            .map(|c| c.ch)
            .collect();
        assert_eq!(r1, "world");
    }

    #[test]
    fn wrapped_line_unwraps_when_widening() {
        // Original: cols=10, "hello worl" wraps to "d" on next row.
        let template = Cell::default();
        let mut rows = vec![Row::new(10, &template); 3];
        for (i, c) in "hello worl".chars().enumerate() {
            rows[0].cells[i].ch = c;
        }
        rows[0].occ = 10;
        // Mark row 0 as wrapping.
        rows[0].cells[9].flags.insert(Flags::WRAPLINE);
        rows[1].cells[0].ch = 'd';
        rows[1].occ = 1;
        let cursor = Cursor::default();

        // Reflow to cols=20: should fit on one row.
        let result = reflow_grid(&rows, &cursor, 20, 3, &template);
        let r0: String = result.rows[0]
            .cells
            .iter()
            .take(result.rows[0].occ)
            .map(|c| c.ch)
            .collect();
        assert_eq!(r0, "hello world");
        // Row 0 should NOT have WRAPLINE anymore — line fits.
        assert!(!result.rows[0].cells.last().unwrap().flags.contains(Flags::WRAPLINE));
    }

    /// Multi-resize cycle should also fix up scrollback. Type a
    /// long line, force it into scrollback (by filling rows
    /// below), shrink, grow — content in scrollback should
    /// survive the cycle without being truncated.
    #[test]
    fn scrollback_reflows_through_multi_resize() {
        use crate::term::Term;
        use crate::Processor;

        let mut t = Term::new(3, 30);
        let mut p = Processor::new();
        // Type a long line + enough LFs to push it into scrollback.
        p.parse_bytes(&mut t, b"hello world this is a long line\r\n");
        p.parse_bytes(&mut t, b"second line\r\n");
        p.parse_bytes(&mut t, b"third line\r\n");
        p.parse_bytes(&mut t, b"fourth line\r\n");

        // Cycle: 30 -> 15 -> 50.
        t.resize(3, 15);
        t.resize(3, 50);

        // The original first line should still be reconstructable
        // by walking scrollback + grid by WRAPLINE chains.
        let mut all_text = String::new();
        for row in t.scrollback.iter() {
            for cell in row.cells.iter().take(row.occ) {
                if !cell.flags.contains(crate::Flags::WIDE_CHAR_SPACER) {
                    all_text.push(cell.ch);
                }
            }
            // Newline boundary unless the last cell wraps.
            let wraps = row
                .cells
                .last()
                .map(|c| c.flags.contains(crate::Flags::WRAPLINE))
                .unwrap_or(false);
            if !wraps {
                all_text.push('\n');
            }
        }
        assert!(
            all_text.contains("hello world this is a long line"),
            "scrollback lost original line. got: {:?}",
            all_text
        );
    }

    /// Reproduce the user-reported pattern: type a line at one
    /// width, shrink, grow, shrink, grow. Content should not get
    /// silently truncated through the cycle — the line should
    /// re-flow to fit each width.
    #[test]
    fn shrink_grow_cycle_preserves_content() {
        use crate::term::Term;
        use crate::Processor;

        let mut t = Term::new(5, 30);
        let mut p = Processor::new();
        // Type one long line that will need to wrap at narrower widths.
        let line = b"hello world this is a long line";
        p.parse_bytes(&mut t, line);

        // Cycle: 30 -> 15 -> 30. Content should still show the
        // full line, just possibly wrapped.
        t.resize(5, 15);
        t.resize(5, 30);

        // After the cycle, content should still be present —
        // either on one row (if it fits) or wrapped, but not
        // truncated. Walk the WRAPLINE chain from row 0 and
        // reconstruct the full logical line.
        let mut full = String::new();
        let mut row_idx = 0;
        loop {
            let row = &t.grid.rows[row_idx];
            let bound = row.occ.min(row.cells.len());
            for cell in row.cells.iter().take(bound) {
                if !cell.flags.contains(crate::Flags::WIDE_CHAR_SPACER) {
                    full.push(cell.ch);
                }
            }
            let wraps = row
                .cells
                .last()
                .map(|c| c.flags.contains(crate::Flags::WRAPLINE))
                .unwrap_or(false);
            if !wraps || row_idx + 1 >= t.grid.rows.len() {
                break;
            }
            row_idx += 1;
        }
        assert!(
            full.contains("hello world this is a long line"),
            "content lost across resize cycle. got: {:?}",
            full
        );
    }

    #[test]
    fn long_line_wraps_when_narrowing() {
        // Original: cols=20, "hello world this is" on one row.
        let template = Cell::default();
        let mut rows = vec![Row::new(20, &template); 3];
        let s = "hello world this is";
        for (i, c) in s.chars().enumerate() {
            rows[0].cells[i].ch = c;
        }
        rows[0].occ = s.len();
        let cursor = Cursor::default();

        // Reflow to cols=10: should wrap.
        let result = reflow_grid(&rows, &cursor, 10, 3, &template);
        let r0: String = result.rows[0]
            .cells
            .iter()
            .take(result.rows[0].occ)
            .map(|c| c.ch)
            .collect();
        assert_eq!(r0, "hello worl");
        let r1: String = result.rows[1]
            .cells
            .iter()
            .take(result.rows[1].occ)
            .map(|c| c.ch)
            .collect();
        assert_eq!(r1, "d this is");
        // Row 0 should now have WRAPLINE set.
        assert!(result.rows[0].cells[9].flags.contains(Flags::WRAPLINE));
    }
}

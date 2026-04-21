//! Per-frame grid snapshot probe.
//!
//! Opt-in via `CRANE_GRID_SNAP=1`. When on, each terminal render
//! frame writes a JSONL record to `~/.crane/grid-snap-<pid>.jsonl`
//! describing the grid *after* all incoming PTY bytes for that frame
//! have been applied:
//!
//! - `t`    — ms since process start
//! - `cur`  — `[line, col]` (line is grid-absolute, col is 0-based)
//! - `off`  — `display_offset`; 0 when viewing live, positive when
//!            user is scrolled up into history
//! - `hist` — current `history_size`; compared across frames it tells
//!            us whether a redraw pushed a row into scrollback
//! - `rows` — every row in the visible viewport, top→bottom, as a
//!            `String` with trailing whitespace preserved so we can
//!            see what actually landed where
//! - `tophist` — the most recent 8 rows of scrollback (the rows that
//!            would be just above the first visible row if the user
//!            scrolled up one step). Lets us tell "duplicate visible"
//!            apart from "duplicate got promoted to history".
//!
//! Records are written only when `hist` changes or the last row of
//! `rows` differs from the previous frame, to keep the log small
//! enough to read as text.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use std::fs::File;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::Mutex;
use std::time::Instant;

static STATE: OnceLock<Mutex<Option<State>>> = OnceLock::new();

struct State {
    file: File,
    start: Instant,
    // Keyed by (term_ptr, cols) so each terminal pane has its own
    // change-gate and pane tag. Pane id is assigned on first snap.
    panes: std::collections::HashMap<(usize, usize), PaneState>,
    next_pane_id: u32,
}

struct PaneState {
    id: u32,
    last_hist: usize,
    last_off: i32,
    last_rows_hash: u64,
}

fn try_open() -> Option<State> {
    if std::env::var("CRANE_GRID_SNAP").ok().as_deref() != Some("1") {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::PathBuf::from(format!("{home}/.crane"));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("grid-snap-{}.jsonl", std::process::id()));
    eprintln!("[crane] grid snap enabled → {}", path.display());
    File::create(&path).ok().map(|f| State {
        file: f,
        start: Instant::now(),
        panes: std::collections::HashMap::new(),
        next_pane_id: 0,
    })
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn row_to_string<L: EventListener>(term: &Term<L>, line: i32, cols: usize) -> String {
    let mut s = String::with_capacity(cols);
    let grid = term.grid();
    for c in 0..cols {
        let cell = &grid[Point::new(Line(line), Column(c))];
        let ch = cell.c;
        s.push(if ch == '\0' { ' ' } else { ch });
    }
    // Right-trim spaces but keep tabs/other visible chars to preserve layout
    let trimmed_end = s.trim_end_matches(' ');
    trimmed_end.to_string()
}

pub fn snap_if_enabled<L: EventListener>(
    term: &Term<L>,
    cursor: (usize, i32),
    display_offset: i32,
    history: usize,
) {
    let mutex = STATE.get_or_init(|| Mutex::new(try_open()));
    let mut guard = match mutex.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    let grid = term.grid();
    let cols = grid.columns();
    let screen_lines = grid.screen_lines() as i32;

    let mut visible: Vec<String> = Vec::with_capacity(screen_lines as usize);
    for line in 0..screen_lines {
        visible.push(row_to_string(term, line, cols));
    }
    // Per-pane change gate. Key by (term_ptr, cols) so a reflow
    // (cols changed) or a pane recreation gets its own state.
    let term_ptr = (term as *const Term<L>) as usize;
    let key = (term_ptr, cols);
    let rows_hash = {
        use std::hash::{Hasher, Hash};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for r in &visible {
            r.hash(&mut h);
        }
        h.finish()
    };
    let (pane_id, is_new) = match state.panes.get_mut(&key) {
        Some(p) => {
            // Gate: fire on history change, offset change, or row-hash
            // change. This catches scroll events and mid-viewport
            // redraws that the old bottom-row gate missed.
            if history == p.last_hist && display_offset == p.last_off && rows_hash == p.last_rows_hash {
                return;
            }
            p.last_hist = history;
            p.last_off = display_offset;
            p.last_rows_hash = rows_hash;
            (p.id, false)
        }
        None => {
            let id = state.next_pane_id;
            state.next_pane_id += 1;
            state.panes.insert(key, PaneState {
                id,
                last_hist: history,
                last_off: display_offset,
                last_rows_hash: rows_hash,
            });
            (id, true)
        }
    };

    // Capture more scrollback when the user is scrolled up — lets us
    // see what they're actually looking at vs. what's live. 32 rows
    // upstream of the first visible line covers a typical screenful.
    let hist_rows_wanted: i32 = if display_offset > 0 { 32 } else { 8 };
    let mut top_history: Vec<String> = Vec::with_capacity(hist_rows_wanted as usize);
    let take = hist_rows_wanted.min(history as i32);
    for off in 1..=take {
        top_history.push(row_to_string(term, -off, cols));
    }

    let t_ms = state.start.elapsed().as_millis();
    let rows_json = visible
        .iter()
        .map(|r| escape_json_str(r))
        .collect::<Vec<_>>()
        .join(",");
    let tophist_json = top_history
        .iter()
        .map(|r| escape_json_str(r))
        .collect::<Vec<_>>()
        .join(",");
    let line = format!(
        "{{\"t\":{t_ms},\"pane\":{pane_id},\"new\":{is_new},\"cols\":{cols},\"cur\":[{},{}],\"off\":{display_offset},\"hist\":{history},\"rows\":[{rows_json}],\"tophist\":[{tophist_json}]}}\n",
        cursor.1, cursor.0,
    );
    let _ = state.file.write_all(line.as_bytes());
}

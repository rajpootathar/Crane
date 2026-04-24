use crate::terminal::Terminal;
use super::grid_snap;
use crate::theme;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Vec2};

/// One URL detected in a row of the visible grid. `col_end` is
/// exclusive. Used by the renderer to draw a hover-underline and to
/// resolve a click back to the URL string.
struct UrlHit {
    col_start: usize,
    col_end: usize,
    url: String,
}

/// One local path detected in a row of the visible grid. Only paths
/// that resolve to something on disk are kept — path detection without
/// the existence check would underline every dotted identifier in the
/// output. `line` / `col` carry an optional `:LINE[:COL]` suffix parsed
/// from compiler-style references; unused at click time today because
/// `open(1)` has no line argument, but recorded so a future in-app
/// Files pane hookup can jump straight to the referenced location.
struct PathHit {
    col_start: usize,
    col_end: usize,
    path: std::path::PathBuf,
    #[allow(dead_code)]
    line: Option<u32>,
    #[allow(dead_code)]
    col: Option<u32>,
}

/// Scan a row of plain text for `http://` / `https://` URLs. Stops at
/// whitespace and trims a small set of trailing punctuation that's
/// almost never part of the URL itself (`.,;:!?)]}>"' `). Conservative
/// on purpose — false negatives are fine, false positives that swallow
/// trailing prose punctuation are user-visible breakage.
fn scan_urls(row: &str) -> Vec<UrlHit> {
    let mut hits = Vec::new();
    let chars: Vec<char> = row.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let starts_http = i + 7 <= n
            && chars[i..i + 7].iter().collect::<String>().eq_ignore_ascii_case("http://");
        let starts_https = i + 8 <= n
            && chars[i..i + 8].iter().collect::<String>().eq_ignore_ascii_case("https://");
        if !(starts_http || starts_https) {
            i += 1;
            continue;
        }
        // Don't treat `xhttp://` mid-word as a URL — require a
        // word-boundary just before. Fine to be strict here; false
        // negatives are recoverable, false positives aren't.
        if i > 0 {
            let prev = chars[i - 1];
            if prev.is_alphanumeric() || prev == '/' || prev == '.' {
                i += 1;
                continue;
            }
        }
        let mut end = i;
        while end < n {
            let c = chars[end];
            if c.is_whitespace() || c == '\0' || (c as u32) < 0x20 {
                break;
            }
            end += 1;
        }
        // Trim trailing punctuation that's likely the surrounding
        // sentence, not part of the URL.
        while end > i {
            let c = chars[end - 1];
            if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '>' | '"' | '\'') {
                end -= 1;
            } else {
                break;
            }
        }
        if end > i + 8 {
            let url: String = chars[i..end].iter().collect();
            hits.push(UrlHit {
                col_start: i,
                col_end: end,
                url,
            });
        }
        i = end.max(i + 1);
    }
    hits
}

/// Split a token into its path part and optional `:LINE[:COL]` suffix.
/// Accepts `path:N` and `path:N:M` where N/M are all digits; anything
/// else falls through as a plain path with no line info. Windows drive
/// letters aren't supported (unix-only codebase), so single-char
/// leading segments aren't a concern.
fn split_line_col(s: &str) -> (&str, Option<u32>, Option<u32>) {
    if let Some(c1) = s.rfind(':') {
        let tail = &s[c1 + 1..];
        let head = &s[..c1];
        if let Ok(n1) = tail.parse::<u32>() {
            if let Some(c2) = head.rfind(':') {
                let mid = &head[c2 + 1..];
                let head2 = &head[..c2];
                if let Ok(n2) = mid.parse::<u32>() {
                    return (head2, Some(n2), Some(n1));
                }
            }
            return (head, Some(n1), None);
        }
    }
    (s, None, None)
}

/// True when `s` looks like a bare filename (`main.rs`, `README.md`).
/// The caller also accepts tokens that contain `/` or start with `~`,
/// so this only needs to catch the no-separator case — reject dotted
/// identifiers like `v1.2.3` or `Self.method` by requiring the extension
/// to be short ASCII alphanumerics.
fn looks_like_file(s: &str) -> bool {
    let Some(dot) = s.rfind('.') else {
        return false;
    };
    let ext = &s[dot + 1..];
    if ext.is_empty() || ext.len() > 8 {
        return false;
    }
    ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Resolve a terminal-emitted path token against the pane's cwd. `~`
/// and `~/…` expand via `$HOME`; absolute tokens pass through; anything
/// else is treated as relative to `cwd`. We don't canonicalize — that
/// would flatten symlinks the user clicked on purpose.
fn resolve_path(token: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    if token == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home);
    }
    if let Some(rest) = token.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    let p = std::path::Path::new(token);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Scan a row of plain text for references to paths that exist on
/// disk. Deliberately aggressive on the syntactic side (any
/// whitespace-separated token that contains `/`, a tilde prefix, or a
/// plausible extension) and then filtered by `Path::exists()` — the
/// stat check is the load-bearing part. URLs are skipped so the URL
/// scanner stays the authority for those.
fn scan_paths(row: &str, cwd: &std::path::Path) -> Vec<PathHit> {
    let mut hits = Vec::new();
    let chars: Vec<char> = row.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        while i < n && (chars[i].is_whitespace() || chars[i] == '\0') {
            i += 1;
        }
        let start = i;
        while i < n && !chars[i].is_whitespace() && chars[i] != '\0' {
            i += 1;
        }
        if start == i {
            break;
        }
        let mut ts = start;
        let mut te = i;
        while ts < te && matches!(chars[ts], '(' | '[' | '{' | '<' | '"' | '\'') {
            ts += 1;
        }
        while te > ts
            && matches!(
                chars[te - 1],
                '.' | ',' | ';' | '!' | '?' | ')' | ']' | '}' | '>' | '"' | '\''
            )
        {
            te -= 1;
        }
        if te - ts < 2 {
            continue;
        }
        let token: String = chars[ts..te].iter().collect();
        let lower = token.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("file://")
        {
            continue;
        }
        let (base, line_no, col_no) = split_line_col(&token);
        if !(base.contains('/') || base.starts_with('~') || looks_like_file(base)) {
            continue;
        }
        let resolved = resolve_path(base, cwd);
        if !resolved.exists() {
            continue;
        }
        let base_chars = base.chars().count();
        hits.push(PathHit {
            col_start: ts,
            col_end: ts + base_chars,
            path: resolved,
            line: line_no,
            col: col_no,
        });
    }
    hits
}

/// Hand a path off to the OS to open in its default app. Matches the
/// `reveal_in_file_manager` pattern in `ui/projects.rs` — spawn so we
/// don't block the UI thread, ignore the child handle (fire-and-forget).
fn open_in_default_app(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
}

fn term_bg() -> Color32 {
    theme::current().terminal_bg.to_color32()
}
fn term_fg() -> Color32 {
    theme::current().terminal_fg.to_color32()
}
fn selection_bg() -> Color32 {
    // Prefer the theme's dedicated `selection` field if set. Custom
    // themes may omit it (serde default = Rgb(0,0,0)) — in that case
    // fall back to the historical accent-at-~28%-alpha derivation so
    // old theme files keep working without modification.
    let t = theme::current();
    let s = t.selection;
    if s.r == 0 && s.g == 0 && s.b == 0 {
        let a = t.accent;
        Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 72)
    } else {
        s.to_color32()
    }
}

fn point_in_selection(point: Point, range: &alacritty_terminal::selection::SelectionRange) -> bool {
    if range.is_block {
        point.line >= range.start.line
            && point.line <= range.end.line
            && point.column >= range.start.column
            && point.column <= range.end.column
    } else if point.line < range.start.line || point.line > range.end.line {
        false
    } else if range.start.line == range.end.line {
        point.column >= range.start.column && point.column <= range.end.column
    } else if point.line == range.start.line {
        point.column >= range.start.column
    } else if point.line == range.end.line {
        point.column <= range.end.column
    } else {
        true
    }
}

/// Column contains a vertical box-drawing char on at least 60% of
/// visible rows — treat as a real TUI separator.
fn is_separator_column<T>(term: &alacritty_terminal::Term<T>, col: usize, rows: usize) -> bool {
    use alacritty_terminal::index::{Column, Line, Point};
    if rows == 0 {
        return false;
    }
    let mut hits = 0usize;
    for r in 0..rows {
        let p = Point::new(Line(r as i32), Column(col));
        let c = term.grid()[p].c;
        // U+2502 │, U+2503 ┃, U+2551 ║, plus a few common double/heavy variants
        if matches!(c, '│' | '┃' | '║' | '╎' | '╏' | '╽' | '╿') {
            hits += 1;
        }
    }
    hits * 5 >= rows * 3
}

fn is_inside_vertical_separators<T>(
    term: &alacritty_terminal::Term<T>,
    start_col: usize,
    rows: usize,
) -> bool {
    use alacritty_terminal::grid::Dimensions;
    let total_cols = term.columns();
    let has_left = (0..start_col).any(|c| is_separator_column(term, c, rows));
    let has_right = (start_col + 1..total_cols).any(|c| is_separator_column(term, c, rows));
    has_left && has_right
}

fn pixel_to_point(
    pos: Pos2,
    origin: Pos2,
    cell_w: f32,
    cell_h: f32,
    cols: usize,
    rows: usize,
    display_offset: usize,
) -> (Point, Side) {
    let rel_x = (pos.x - origin.x).max(0.0);
    let rel_y = (pos.y - origin.y).max(0.0);
    let col_f = rel_x / cell_w;
    let line_f = rel_y / cell_h;
    let col = (col_f.floor() as usize).min(cols.saturating_sub(1));
    let viewport_line = (line_f.floor() as usize).min(rows.saturating_sub(1));
    // Alacritty's Selection wants grid-absolute Line: negative into
    // scrollback, 0..screen_lines-1 for the current screen. At
    // display_offset=0 the viewport IS the current screen; as the
    // user scrolls up each display_offset step shifts what's visible
    // by one row into history.
    let grid_line = viewport_line as i32 - display_offset as i32;
    let side = if col_f - col_f.floor() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (Point::new(Line(grid_line), Column(col)), side)
}

pub fn render_terminal(ui: &mut egui::Ui, terminal: &mut Terminal, font_size: f32, has_focus: bool) {
    let font_id = FontId::new(font_size, FontFamily::Monospace);
    // Measure the stride egui actually uses when it lays out a galley,
    // not the bare glyph advance. `glyph_width('M')` differs from the
    // per-char step of a laid-out galley by a fraction of a pixel —
    // enough to drift the cursor onto the previous cell after ~25
    // columns of typed text. Laying out a 32-char string of 'M' and
    // dividing by 32 gives the real stride that `painter.galley` will
    // step by, so cursor math matches exactly.
    let cell_h = ui.fonts_mut(|f| f.row_height(&font_id));
    let cell_w = {
        let mut job = LayoutJob::default();
        job.append(
            "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM",
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                ..Default::default()
            },
        );
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        galley.rect.width() / 32.0
    };

    let available = ui.available_size();
    let cols = ((available.x / cell_w).floor() as usize).max(20);
    let rows = ((available.y / cell_h).floor() as usize).max(5);
    terminal.resize(cols, rows);
    // Flush any VT replies alacritty's parser queued (CSI 6n cursor
    // position, DSR, etc.). See WakeListener comment for why these
    // are queued rather than written synchronously.
    terminal.flush_pty_replies();

    let (response, painter) = ui.allocate_painter(
        Vec2::new(cols as f32 * cell_w, rows as f32 * cell_h),
        Sense::click_and_drag().union(Sense::focusable_noninteractive()),
    );
    let origin = response.rect.min;

    let bg_theme = term_bg();
    painter.rect_filled(response.rect, 0.0, bg_theme);

    // I-beam over the terminal so it feels like selectable text.
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // Scrollback: mouse wheel → alacritty Scroll::Delta. Positive
    // delta is upward in egui (history); alacritty scrolls up into
    // history on positive delta, so the sign passes through.
    //
    // Alacritty's grid is row-granular — `Scroll::Delta(1)` jumps the
    // viewport by a whole cell_h every commit, which feels like the
    // 16-px stutter the user sees vs. egui's pixel-smooth ScrollArea
    // in Files. We close the gap by accumulating the wheel into
    // `scroll_carry` (in fractional-row units) and applying the
    // sub-row remainder as a pixel offset on the painted rows below.
    // Whole-row crossings still get committed to alacritty so the
    // grid actually advances; the carry persists between frames so
    // the view stays where the user left it.
    if response.hovered() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 0.01 {
            let (disp, hist) = {
                let g = terminal.term.lock();
                (g.grid().display_offset(), g.history_size())
            };
            let mut carry = terminal.scroll_carry.lock();
            *carry += wheel / cell_h;
            // Clamp at the scroll boundaries so the pixel offset can't
            // shift content past the live screen at the bottom or past
            // the oldest history row at the top — without this the
            // sub-row offset would tear the view off into an empty
            // sliver that never fills.
            if disp == 0 {
                *carry = carry.max(0.0);
            }
            if disp >= hist {
                *carry = carry.min(0.0);
            }
            let lines = carry.trunc() as i32;
            if lines != 0 {
                *carry -= lines as f32;
                terminal.term.lock().scroll_display(Scroll::Delta(lines));
            }
        }
    }
    // Sub-row offset (px) applied to every painted row + the cursor so
    // motion between commits tracks the trackpad 1:1 instead of
    // snapping to row boundaries.
    let scroll_pixel_offset = *terminal.scroll_carry.lock() * cell_h;

    // Drag: plain range select. pixel_to_point needs the current
    // display_offset so clicks on scrollback content resolve to the
    // right (negative) grid line rather than landing on the current
    // screen.
    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos() {
            let mut guard = terminal.term.lock();
            let off = guard.grid().display_offset();
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows, off);
            // Ghostty-style column-aware selection: if the start cell
            // sits between two columns that contain vertical
            // box-drawing characters on most visible rows (i.e. the TUI
            // has a real vertical separator on either side, like Ink's
            // sidebar divider in llm-party / lazygit / k9s), promote
            // the selection to Block mode so dragging down one column
            // doesn't drag the neighboring column's text along.
            let kind = if is_inside_vertical_separators(
                &guard,
                point.column.0,
                rows,
            ) {
                SelectionType::Block
            } else {
                SelectionType::Simple
            };
            guard.selection = Some(Selection::new(kind, point, side));
        }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos() {
            let mut guard = terminal.term.lock();
            let off = guard.grid().display_offset();
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows, off);
            if let Some(sel) = guard.selection.as_mut() {
                sel.update(point, side);
            }
        }

    // Clicks: 1 → clear, 2 → word (Semantic), 3 → line (Lines),
    // Shift+click → extend existing selection to click point.
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos() {
            let off = terminal.term.lock().grid().display_offset();
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows, off);
            let shift_held = ui.input(|i| i.modifiers.shift);
            let now = std::time::Instant::now();
            let is_multi = terminal
                .last_click
                .map(|(t, line, col)| {
                    now.duration_since(t) < std::time::Duration::from_millis(500)
                        && line == point.line.0
                        && col == point.column.0
                })
                .unwrap_or(false);
            terminal.click_count = if is_multi { terminal.click_count + 1 } else { 1 };
            terminal.last_click = Some((now, point.line.0, point.column.0));

            let mut guard = terminal.term.lock();
            if shift_held && guard.selection.is_some() {
                if let Some(sel) = guard.selection.as_mut() {
                    sel.update(point, side);
                }
            } else {
                match terminal.click_count {
                    2 => {
                        guard.selection =
                            Some(Selection::new(SelectionType::Semantic, point, Side::Left));
                    }
                    3 => {
                        guard.selection =
                            Some(Selection::new(SelectionType::Lines, point, Side::Left));
                    }
                    _ => {
                        guard.selection = None;
                    }
                }
            }
        }

    let snapshot = {
        let guard = terminal.term.lock();
        let content = guard.renderable_content();
        let offset = content.display_offset as i32;
        let cursor = (
            content.cursor.point.column.0,
            content.cursor.point.line.0 + offset,
        );
        let selection = content.selection;
        let cells: Vec<_> = content
            .display_iter
            .map(|item| (item.point, item.cell.clone()))
            .collect();
        // history_size lives on the Dimensions trait (Grid impls it).
        let history = guard.history_size();
        // Opt-in grid snapshot probe. When `CRANE_GRID_SNAP=1` is set
        // in the parent env, dump the full visible grid + the top 8
        // history rows + cursor coords to a JSONL log each time the
        // grid content changed since the previous frame. Meant for
        // diagnosing the Claude Code duplicate-prompt artifact: row
        // text is captured exactly as alacritty stores it, so we can
        // tell whether a given "duplicate" row is in live viewport or
        // has been promoted into scrollback history.
        grid_snap::snap_if_enabled(&guard, cursor, offset, history);
        // Build a set of trimmed text for the bottom ~12 live rows so we
        // can dedup TUI redraw artifacts out of scrollback at paint
        // time. Reading direct from the grid (rather than display_iter,
        // which follows display_offset) always gives the currently
        // live bottom rows regardless of scroll position.
        let grid = guard.grid();
        let screen_lines = grid.screen_lines() as i32;
        let grid_cols = grid.columns();
        let mut live_bottom: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let probe_from = (screen_lines - 12).max(0);
        for l in probe_from..screen_lines {
            let mut s = String::with_capacity(grid_cols);
            for c in 0..grid_cols {
                let ch = grid[Point::new(Line(l), Column(c))].c;
                s.push(if ch == '\0' { ' ' } else { ch });
            }
            let trimmed = s.trim_end_matches(' ').to_string();
            if trimmed.chars().filter(|c| !c.is_whitespace()).count() >= 3 {
                live_bottom.insert(trimmed);
            }
        }
        // Also pull in rows the PTY reader recorded as "scrolled into
        // history during a `?2026h..?2026l` sync block". Those are Ink
        // redraw leftovers — content the TUI has since overwritten,
        // but the LF-scroll pushed a copy into history. The current-
        // bottom snapshot above only catches redraw artifacts that
        // still match what's on screen right now; this covers older
        // frames whose content has since rolled off the live region.
        // Locked briefly, copied into the same set so the paint-side
        // dedup stays a single HashSet lookup.
        {
            let gt = terminal.ghost_texts.lock();
            for t in gt.iter() {
                live_bottom.insert(t.clone());
            }
        }
        (cells, cursor, selection, offset, history, live_bottom)
    };
    let (cells, (cursor_col, cursor_line), selection, display_offset, history_size, live_bottom) = snapshot;

    // Group cells by line, then batch each line into a single LayoutJob
    // grouped by contiguous runs of same (fg, bg, flags). This cuts paint
    // calls from one-per-cell (~4800 for 120×40) down to a small handful
    // per row (~3–10), and hands the font layout off to egui once per row
    // instead of once per glyph.
    let cols_count = cols;
    let mut by_row: std::collections::BTreeMap<i32, Vec<(usize, alacritty_terminal::term::cell::Cell, bool)>> =
        std::collections::BTreeMap::new();
    for (point, cell) in cells {
        // alacritty yields display_iter items in grid-absolute line
        // coordinates, which go negative into history when the user has
        // scrolled up. Translate to viewport-local (0..screen_lines) by
        // adding the current display offset.
        let viewport_line = point.line.0 + display_offset;
        if viewport_line < 0 || viewport_line as usize >= rows {
            continue;
        }
        let in_selection = selection
            .map(|sel| point_in_selection(point, &sel))
            .unwrap_or(false);
        by_row
            .entry(viewport_line)
            .or_default()
            .push((point.column.0, cell, in_selection));
    }

    // Render-time scrollback dedup: when the user has scrolled up into
    // history (display_offset > 0), drop any viewport row whose original
    // grid line is in history (< 0) AND whose text matches a live
    // bottom row. This hides the Claude-Code-style duplicate prompt
    // artifact without mutating grid state.
    if display_offset > 0 && !live_bottom.is_empty() {
        let dedup_rows: Vec<i32> = by_row
            .iter()
            .filter_map(|(viewport_line, cells)| {
                let original_line = *viewport_line - display_offset;
                if original_line >= 0 {
                    return None;
                }
                let mut chars: Vec<(usize, char)> =
                    cells.iter().map(|(c, cell, _)| (*c, cell.c)).collect();
                chars.sort_by_key(|(c, _)| *c);
                let mut s = String::with_capacity(chars.len());
                for (_, ch) in chars {
                    s.push(if ch == '\0' { ' ' } else { ch });
                }
                let trimmed = s.trim_end_matches(' ').to_string();
                if live_bottom.contains(&trimmed) {
                    Some(*viewport_line)
                } else {
                    None
                }
            })
            .collect();
        for l in dedup_rows {
            by_row.remove(&l);
        }
    }

    // URL + path scan over the visible rows. We build each row's text
    // from the already-assembled cells (cheaper than a second grid walk
    // and respects scrollback offset + dedup). Both maps are keyed by
    // viewport_line so lookups during paint / hit-test map directly.
    // URL hits take priority when ranges overlap — a URL is a strictly
    // more specific match than "token with a dot".
    let col_stride_for_scan = cell_w.max(1.0);
    let mut urls_by_line: std::collections::HashMap<i32, Vec<UrlHit>> =
        std::collections::HashMap::new();
    let mut paths_by_line: std::collections::HashMap<i32, Vec<PathHit>> =
        std::collections::HashMap::new();
    let pane_cwd = terminal.cwd.clone();
    for (line, cells) in by_row.iter() {
        let mut by_col = vec![' '; cols_count];
        for (c, cell, _) in cells {
            if *c < cols_count {
                let ch = match cell.c {
                    '\0' | '\n' | '\r' | '\t' => ' ',
                    c => c,
                };
                by_col[*c] = ch;
            }
        }
        let row_text: String = by_col.into_iter().collect();
        let u_hits = scan_urls(&row_text);
        if !u_hits.is_empty() {
            urls_by_line.insert(*line, u_hits);
        }
        let p_hits = scan_paths(&row_text, &pane_cwd);
        if !p_hits.is_empty() {
            paths_by_line.insert(*line, p_hits);
        }
    }

    /// What's under the pointer: a URL (click opens in default browser)
    /// or a local path (click opens in default app). Borrowed from the
    /// hit maps so we don't clone the path on every hover frame.
    enum HoveredKind<'a> {
        Url(&'a str),
        Path(&'a std::path::Path),
    }

    // Resolve whatever's under the pointer — URL first, then path.
    // Mapping inverts `row_y = origin.y + line*cell_h + offset` so the
    // hover cell tracks the sub-row scroll offset.
    let hovered_hit: Option<(i32, usize, usize, HoveredKind<'_>)> =
        response.hover_pos().and_then(|pos| {
            if !response.rect.contains(pos) {
                return None;
            }
            let rel_x = pos.x - origin.x;
            let rel_y = pos.y - origin.y - scroll_pixel_offset;
            if rel_x < 0.0 || rel_y < 0.0 {
                return None;
            }
            let line = (rel_y / cell_h).floor() as i32;
            let col = (rel_x / col_stride_for_scan).floor() as usize;
            if let Some(hits) = urls_by_line.get(&line)
                && let Some(h) = hits
                    .iter()
                    .find(|h| col >= h.col_start && col < h.col_end)
            {
                return Some((line, h.col_start, h.col_end, HoveredKind::Url(&h.url)));
            }
            if let Some(hits) = paths_by_line.get(&line)
                && let Some(h) = hits
                    .iter()
                    .find(|h| col >= h.col_start && col < h.col_end)
            {
                return Some((line, h.col_start, h.col_end, HoveredKind::Path(&h.path)));
            }
            None
        });

    if hovered_hit.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Plain click (no drag) on a hovered URL or path → hand it straight
    // to the OS default handler. `response.clicked()` is false on drags,
    // so text selection is unaffected.
    if response.clicked()
        && let Some((_, _, _, kind)) = &hovered_hit
    {
        match kind {
            HoveredKind::Url(url) => {
                let _ = webbrowser::open(url);
            }
            HoveredKind::Path(path) => {
                open_in_default_app(path);
            }
        }
    }

    let fallback_fg = term_fg();
    // Per-run pinning: each style run is painted at run_start_col *
    // col_stride, so egui's galley advance only accumulates WITHIN a
    // run, not across the whole row. Use the raw (unrounded) cell_w so
    // text and cursor share the same stride — rounding here introduces
    // a sub-pixel gap per column that shows up as a visible gap between
    // the last prompt char and the cursor on wider widths.
    let col_stride = cell_w.max(1.0);
    for (line, mut row_cells) in by_row {
        row_cells.sort_by_key(|(c, _, _)| *c);
        let row_y = (origin.y + line as f32 * cell_h + scroll_pixel_offset).round();
        let row_x = origin.x.round();

        // Paint each style run as its own galley pinned to
        // `row_x + run_start_col * col_stride`. This guarantees
        // text columns match cursor column exactly regardless of how
        // egui's font layout accumulates per-glyph advance.
        let mut cur_fg: Option<Color32> = None;
        let mut cur_bg: Option<Color32> = None;
        let mut cur_underline = false;
        let mut buf = String::new();
        let mut run_start_col: usize = 0;

        let flush = |buf: &mut String,
                         run_start_col: usize,
                         fg: Option<Color32>,
                         bg: Option<Color32>,
                         underline: bool,
                         ui: &mut egui::Ui| {
            if buf.is_empty() {
                return;
            }
            let color = fg.unwrap_or(fallback_fg);
            let bg_visible = bg.filter(|&b| b != bg_theme);
            let stroke = if underline {
                egui::Stroke::new(1.0, color)
            } else {
                egui::Stroke::NONE
            };
            let char_cols = buf.chars().count();
            let run_x = row_x + run_start_col as f32 * col_stride;
            // Paint cell background across the full run rect BEFORE
            // drawing the glyphs. egui's TextFormat::background only
            // fills behind the glyph path, so space-only cells in a
            // highlighted row lose the bar (visible in nvitop row
            // selection, TUI dividers, etc.). A rect_filled spanning
            // `char_cols * col_stride × cell_h` restores the full bar.
            if let Some(bg_color) = bg_visible {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        Pos2::new(run_x, row_y),
                        Vec2::new(char_cols as f32 * col_stride, cell_h),
                    ),
                    0.0,
                    bg_color,
                );
            }
            let mut job = LayoutJob::default();
            job.append(
                buf,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    background: Color32::TRANSPARENT,
                    underline: stroke,
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(Pos2::new(run_x, row_y), galley, fallback_fg);
            buf.clear();
        };

        // Walk columns strictly 0..cols_count, pulling the cell for
        // each column from `row_cells`. This keeps buf's character
        // count === visual column, which is the invariant that was
        // being violated (resized grids occasionally emit display_iter
        // cells with col values that no longer align to the current
        // viewport, leading to packed/misaligned text). Row_cells was
        // already sorted ascending above, so we walk both in lockstep.
        let mut idx = 0;
        let default_cell = alacritty_terminal::term::cell::Cell::default();
        for col in 0..cols_count {
            while idx < row_cells.len() && row_cells[idx].0 < col {
                idx += 1;
            }
            let (cell, in_selection) = if idx < row_cells.len() && row_cells[idx].0 == col {
                (&row_cells[idx].1, row_cells[idx].2)
            } else {
                (&default_cell, false)
            };
            // Wide-char second cell: alacritty emits a WIDE_CHAR on
            // col N and a WIDE_CHAR_SPACER on col N+1 (CJK, emoji,
            // Nerd Font icons marked wide). We MUST contribute
            // something at col N+1 — if we `continue` here, `buf`
            // ends up one char short per spacer, left-shifting every
            // cell right of the wide char by one cell_w. Emit a space
            // with the same style so the visible spacing stays
            // 1-cell-per-column.
            let is_wide_spacer = cell.flags.contains(CellFlags::WIDE_CHAR_SPACER);
            let mut fg = color_to_egui(cell.fg, true);
            let mut bg = color_to_egui(cell.bg, false);
            // SGR 7 (reverse video) — TUIs like nvitop / htop use this
            // to highlight the selected row. alacritty tags cells with
            // CellFlags::INVERSE; the renderer must swap fg and bg at
            // paint time. Without this the row looks unhighlighted.
            if cell.flags.contains(CellFlags::INVERSE) {
                // If bg was the default (terminal bg), swapping gives us
                // the theme bg as the text color — unreadable. Use the
                // fallback_fg (theme text color) in that case so the
                // inverted text stays visible against its new bg.
                let new_bg = if fg == bg_theme { fallback_fg } else { fg };
                let new_fg = if bg == bg_theme { bg_theme } else { bg };
                fg = new_fg;
                bg = new_bg;
            }
            if in_selection {
                bg = selection_bg();
            }
            let underline = cell.flags.contains(CellFlags::UNDERLINE);
            if Some(fg) != cur_fg || Some(bg) != cur_bg || underline != cur_underline {
                flush(&mut buf, run_start_col, cur_fg, cur_bg, cur_underline, ui);
                run_start_col = col;
                cur_fg = Some(fg);
                cur_bg = Some(bg);
                cur_underline = underline;
            }
            let ch = if is_wide_spacer {
                ' '
            } else {
                match cell.c {
                    '\0' | '\n' | '\r' | '\t' => ' ',
                    c => c,
                }
            };
            // Batched runs advance chars at egui's internal per-glyph
            // stride, not `col_stride`. For plain ASCII those match (all
            // glyphs are the same width as 'M'), but a non-ASCII cell
            // (wide-char second half, Nerd-Font icons, □/▎/tofu fallback)
            // renders via font-fallback with a different advance and
            // shifts every subsequent char in the run. The cursor uses
            // `col_stride * cursor_col`, so typed text lands at a
            // different column than the cursor block. Fix: flush the run
            // around the odd glyph and paint it in its own single-char
            // galley pinned to `col * col_stride`, so only that one cell
            // is potentially off (visually) while grid alignment resumes
            // at col+1.
            if ch.is_ascii() {
                buf.push(ch);
            } else {
                flush(&mut buf, run_start_col, cur_fg, cur_bg, cur_underline, ui);
                let mut tmp = String::new();
                tmp.push(ch);
                flush(&mut tmp, col, cur_fg, cur_bg, cur_underline, ui);
                run_start_col = col + 1;
            }
        }
        flush(&mut buf, run_start_col, cur_fg, cur_bg, cur_underline, ui);
    }

    // Hover underline for the URL / path under the pointer. Drawn
    // AFTER the row paint so it sits on top of the glyph row and
    // doesn't get overwritten. Painted as a single line_segment rather
    // than baked into the run TextFormat, so the existing run/style
    // logic stays untouched and a hit spanning multiple style runs
    // still gets one continuous underline.
    if let Some((line, col_start, col_end, _)) = &hovered_hit {
        let y = (origin.y + (*line as f32 + 1.0) * cell_h + scroll_pixel_offset - 1.0).round();
        let x0 = (origin.x + *col_start as f32 * col_stride).round();
        let x1 = (origin.x + *col_end as f32 * col_stride).round();
        painter.line_segment(
            [Pos2::new(x0, y), Pos2::new(x1, y)],
            egui::Stroke::new(1.0, fallback_fg),
        );
    }

    // Snap cursor to integer pixels so it aligns with char cells. Subpixel
    // drift accumulates on long lines and makes the cursor look "off by
    // one" vs where the next character will print.
    let cx = origin.x.round() + cursor_col as f32 * col_stride;
    let cy = (origin.y + cursor_line as f32 * cell_h + scroll_pixel_offset).round();
    let cw = col_stride;
    let ch = cell_h.round();
    let cursor_color = {
        let c = theme::current().terminal_fg;
        Color32::from_rgba_unmultiplied(c.r, c.g, c.b, 130)
    };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(cw, ch)),
        0.0,
        cursor_color,
    );

    // Scrollbar — right-edge thumb whose height reflects the visible
    // viewport's share of (history + viewport), and whose y reflects
    // the current display_offset. Drag scrolls; no scrollbar drawn
    // when there's no history yet.
    let total = history_size + rows;
    if history_size > 0 && total > rows {
        let track_w = 6.0;
        let track_rect = Rect::from_min_max(
            Pos2::new(response.rect.max.x - track_w, response.rect.min.y),
            Pos2::new(response.rect.max.x, response.rect.max.y),
        );
        let thumb_h = (track_rect.height() * rows as f32 / total as f32).max(20.0);
        // display_offset = 0 → thumb at bottom; display_offset = history
        // → thumb at top. The scrollable thumb range is
        // (track_height - thumb_h).
        let scrollable = (track_rect.height() - thumb_h).max(1.0);
        let y_from_top =
            scrollable * (1.0 - display_offset as f32 / history_size as f32);
        let thumb_rect = Rect::from_min_size(
            Pos2::new(track_rect.min.x, track_rect.min.y + y_from_top),
            Vec2::new(track_w, thumb_h),
        );
        let t = theme::current();
        let track_col = Color32::from_rgba_unmultiplied(255, 255, 255, 8);
        painter.rect_filled(track_rect, 3.0, track_col);
        let scroll_id = ui.id().with("terminal_scrollbar");
        let thumb_resp = ui.interact(thumb_rect, scroll_id, egui::Sense::drag());
        let thumb_col = if thumb_resp.dragged() {
            t.accent.to_color32()
        } else if thumb_resp.hovered() {
            Color32::from_rgba_unmultiplied(255, 255, 255, 90)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 55)
        };
        painter.rect_filled(thumb_rect, 3.0, thumb_col);
        if thumb_resp.hovered() || thumb_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }
        if thumb_resp.dragged() {
            let dy = thumb_resp.drag_delta().y;
            // Drag down → positive dy → scroll toward newer content
            // (decrease display_offset). One thumb-pixel equals
            // `history / scrollable` history-lines.
            let lines_per_px = history_size as f32 / scrollable;
            let delta_lines = -(dy * lines_per_px).round() as i32;
            if delta_lines != 0 {
                terminal
                    .term
                    .lock()
                    .scroll_display(Scroll::Delta(delta_lines));
            }
        }
    }

    if !has_focus {
        return;
    }
    // True when another egui widget (e.g. tab-rename TextEdit) owns
    // keyboard focus. We still want terminal-level command shortcuts
    // (Cmd+K, Cmd+A, Copy, Paste) to work globally — only the raw-key
    // fall-through that writes to the PTY must skip in this case.
    let other_widget_focused = ui.memory(|m| m.focused().is_some());

    let mut copy_text: Option<String> = None;
    let mut paste_text: Option<String> = None;
    let mut clear_requested = false;
    // When a modal overlay is open the parent ui is disabled — skip
    // all keyboard/paste input routing so key events don't leak into
    // the PTY through the backdrop.
    let input_enabled = ui.is_enabled();
    // Image paste: on macOS an NSEvent local monitor (mac_keys.rs)
    // catches Cmd+V before winit sees it, reads NSPasteboard for
    // image data, writes it to a temp PNG, and enqueues the path.
    // Drain here so it flows into the active terminal as a normal
    // bracketed paste. egui-winit's Event::Paste path can't be used:
    // it calls arboard.get() for text only and returns early on
    // image clipboards without pushing any event.
    #[cfg(target_os = "macos")]
    if input_enabled && !other_widget_focused {
        let mut paths = crate::mac_keys::drain_pending_image_paths();
        if let Some(p) = paths.pop() {
            paste_text = Some(p);
        }
    }
    // Tell the NSEvent monitor that the terminal has focus so it will
    // swallow Shift+Tab (CSI Z). egui's focus navigator eats the key
    // before our handler runs in-frame, so we intercept at the OS
    // level. See `mac_keys.rs::set_terminal_focused`.
    #[cfg(target_os = "macos")]
    crate::mac_keys::set_terminal_focused(input_enabled && !other_widget_focused);

    // Drain and write any Shift+Tab presses the NSEvent monitor caught.
    #[cfg(target_os = "macos")]
    if input_enabled && !other_widget_focused {
        let count = crate::mac_keys::drain_pending_shift_tab();
        for _ in 0..count {
            terminal.write_input(b"\x1b[Z");
        }
        let tab_count = crate::mac_keys::drain_pending_tab();
        for _ in 0..tab_count {
            terminal.write_input(b"\t");
        }
    }

    // Plain Tab still goes through the normal event path (egui doesn't
    // eat plain Tab the way it eats Shift+Tab), handled in the main
    // key-event loop below via `named_key_bytes`.
    if input_enabled { ui.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Copy => {
                    if other_widget_focused {
                        continue;
                    }
                    let guard = terminal.term.lock();
                    if let Some(t) = guard.selection_to_string()
                        && !t.is_empty() {
                            // Trim trailing whitespace per line — TUIs
                            // right-pad cells to a fixed width with
                            // spaces, so a plain cell-range copy drags
                            // that padding into the clipboard along
                            // with the real text. iTerm2 / WezTerm /
                            // Terminal.app all trim per-row on copy,
                            // which is what makes "just drag and copy"
                            // feel right in TUIs like llm-party.
                            let trimmed: String = t
                                .split('\n')
                                .map(|line| line.trim_end_matches([' ', '\t']))
                                .collect::<Vec<_>>()
                                .join("\n");
                            copy_text = Some(trimmed);
                        }
                }
                egui::Event::Key {
                    key: egui::Key::K,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.mac_cmd || modifiers.command => {
                    if other_widget_focused {
                        continue;
                    }
                    // Queue; actual work happens after the input
                    // closure unlocks Context. Driving the ANSI parser
                    // inside `ui.input` used to deadlock because
                    // alacritty's WakeListener calls
                    // ctx.request_repaint() on certain escape events,
                    // and that call takes a Context write lock while
                    // our ui.input closure still holds its read lock.
                    clear_requested = true;
                }
                egui::Event::Key {
                    key: egui::Key::A,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.mac_cmd || modifiers.command => {
                    if other_widget_focused {
                        continue;
                    }
                    let mut guard = terminal.term.lock();
                    let start = Point::new(Line(0), Column(0));
                    let end = Point::new(
                        Line(rows.saturating_sub(1) as i32),
                        Column(cols.saturating_sub(1)),
                    );
                    let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
                    sel.update(end, Side::Right);
                    guard.selection = Some(sel);
                }
                egui::Event::Paste(text) => {
                    // Another widget (tab-rename TextEdit, find bar, etc.)
                    // owns focus — don't also paste into the PTY.
                    if other_widget_focused {
                        continue;
                    }
                    if !text.is_empty() {
                        paste_text = Some(text.clone());
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Another widget (rename TextEdit, branch-picker
                    // filter, etc.) owns keyboard focus — swallow the
                    // key so it doesn't also echo into the PTY.
                    if other_widget_focused {
                        continue;
                    }
                    if modifiers.ctrl
                        && let Some(letter) = key_letter(*key) {
                            terminal.write_input(&[letter - b'a' + 1]);
                            continue;
                        }
                    if modifiers.mac_cmd || modifiers.command {
                        // Image paste for Cmd+V is handled by
                        // mac_keys.rs's NSEvent monitor, whose queue
                        // is drained above. All other Cmd+key combos
                        // are swallowed so they don't echo to the PTY.
                        continue;
                    }
                    // Alt/Option + arrow: emit word-navigation sequences
                    // most shells expect (bash, zsh, fish all read ESC b / f
                    // for word back / forward). Also covers Alt + letter as
                    // generic "ESC + <char>".
                    if modifiers.alt {
                        match *key {
                            egui::Key::ArrowLeft => {
                                terminal.write_input(b"\x1bb");
                                continue;
                            }
                            egui::Key::ArrowRight => {
                                terminal.write_input(b"\x1bf");
                                continue;
                            }
                            egui::Key::Backspace => {
                                // Alt+Backspace → delete previous word.
                                terminal.write_input(b"\x1b\x7f");
                                continue;
                            }
                            _ => {
                                if let Some(letter) = key_letter(*key) {
                                    terminal.write_input(&[0x1b, letter]);
                                    continue;
                                }
                            }
                        }
                    }
                    let app_cursor = terminal
                        .term
                        .lock()
                        .mode()
                        .contains(alacritty_terminal::term::TermMode::APP_CURSOR);
                    if let Some(bytes) = named_key_bytes(*key, app_cursor) {
                        terminal.write_input(&bytes);
                    }
                }
                egui::Event::Text(text) => {
                    // Don't echo into the PTY while another widget
                    // (tab-rename TextEdit, find bar, etc.) owns focus.
                    // Without this guard, every typed char ends up in
                    // both the rename box AND the terminal.
                    if other_widget_focused {
                        continue;
                    }
                    terminal.write_input(text.as_bytes());
                }
                _ => {}
            }
        }
    }); }
    // Safe to drive scroll_display now that ui.input's read lock on
    // Context has released. write_input accumulates into a flag —
    // drain it here so typing snaps the viewport back to the live
    // screen without racing the alacritty listener → request_repaint
    // → Context write-lock path.
    terminal.flush_scroll_to_bottom();
    if let Some(t) = copy_text {
        ui.ctx().copy_text(t);
    }
    if let Some(t) = paste_text {
        // Only wrap in bracketed-paste markers when the running shell
        // / TUI has actually asked for it (DECSET 2004 — alacritty
        // tracks this as TermMode::BRACKETED_PASTE). If we wrap
        // unconditionally, shells/apps that haven't enabled the mode
        // see "200~…201~" as literal command text.
        let bracketed = terminal
            .term
            .lock()
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        if bracketed {
            let mut bytes = Vec::with_capacity(t.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(t.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            terminal.write_input(&bytes);
        } else {
            terminal.write_input(t.as_bytes());
        }
    }
    if clear_requested {
        // Two regimes, distinguished by whether a foreground process is
        // running in the PTY:
        //
        // 1. Bare shell prompt — full clear: cursor home + erase
        //    display + erase scrollback, then Ctrl+L to the PTY so zsh
        //    / bash repaint the prompt at row 0. This is what the user
        //    actually expects from Cmd+K and matches Terminal.app.
        //
        // 2. Foreground TUI (vim, claude, htop, …) — scrollback only
        //    (`\x1b[3J`). A full clear would home the cursor and wipe
        //    the alt-screen widget; the TUI's next write would then
        //    land at (0,0) instead of where it left off, leaving its
        //    UI broken until a manual redraw. Match iTerm2's "Clear
        //    Buffer" semantics here.
        let tui_active = terminal.has_foreground_process();
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        {
            let mut guard = terminal.term.lock();
            if tui_active {
                processor.advance(&mut *guard, b"\x1b[3J");
            } else {
                processor.advance(&mut *guard, b"\x1b[H\x1b[2J\x1b[3J");
            }
            guard.scroll_display(Scroll::Bottom);
        }
        if !tui_active {
            terminal.write_input(b"\x0c");
        }
        terminal.history.lock().clear();
    }
}

fn color_to_egui(color: TermColor, is_fg: bool) -> Color32 {
    match color {
        TermColor::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        TermColor::Indexed(idx) => {
            let (r, g, b) = palette(idx);
            Color32::from_rgb(r, g, b)
        }
        TermColor::Named(named) => match named {
            NamedColor::Foreground => term_fg(),
            NamedColor::Background => term_bg(),
            NamedColor::Cursor => term_fg(),
            other => {
                let idx = other as u16;
                if idx < 16 {
                    let (r, g, b) = palette(idx as u8);
                    Color32::from_rgb(r, g, b)
                } else if is_fg {
                    term_fg()
                } else {
                    term_bg()
                }
            }
        },
    }
}

fn palette(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => (0x1a, 0x1c, 0x28),
        1 => (0xcc, 0x55, 0x55),
        2 => (0x44, 0xaa, 0x99),
        3 => (0xe8, 0x92, 0x2a),
        4 => (0x5a, 0x7a, 0xbf),
        5 => (0xaa, 0x66, 0xcc),
        6 => (0x55, 0xaa, 0xaa),
        7 => (0xb0, 0xb4, 0xc0),
        8 => (0x4a, 0x4c, 0x5a),
        9 => (0xff, 0x66, 0x66),
        10 => (0x55, 0xcc, 0xbb),
        11 => (0xff, 0xaa, 0x44),
        12 => (0x77, 0x99, 0xdd),
        13 => (0xcc, 0x77, 0xdd),
        14 => (0x77, 0xcc, 0xcc),
        15 => (0xdd, 0xdd, 0xee),
        16..=231 => {
            let i = idx - 16;
            let r = (i / 36) * 51;
            let g = ((i % 36) / 6) * 51;
            let b = (i % 6) * 51;
            (r, g, b)
        }
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn key_letter(key: egui::Key) -> Option<u8> {
    use egui::Key;
    match key {
        Key::A => Some(b'a'),
        Key::B => Some(b'b'),
        Key::C => Some(b'c'),
        Key::D => Some(b'd'),
        Key::E => Some(b'e'),
        Key::F => Some(b'f'),
        Key::G => Some(b'g'),
        Key::H => Some(b'h'),
        Key::I => Some(b'i'),
        Key::J => Some(b'j'),
        Key::K => Some(b'k'),
        Key::L => Some(b'l'),
        Key::M => Some(b'm'),
        Key::N => Some(b'n'),
        Key::O => Some(b'o'),
        Key::P => Some(b'p'),
        Key::Q => Some(b'q'),
        Key::R => Some(b'r'),
        Key::S => Some(b's'),
        Key::T => Some(b't'),
        Key::U => Some(b'u'),
        Key::V => Some(b'v'),
        Key::W => Some(b'w'),
        Key::X => Some(b'x'),
        Key::Y => Some(b'y'),
        Key::Z => Some(b'z'),
        _ => None,
    }
}

fn named_key_bytes(key: egui::Key, app_cursor: bool) -> Option<Vec<u8>> {
    use egui::Key;
    // DECCKM (ESC [ ? 1 h) switches arrow/home/end to SS3 prefixes
    // (\x1bO...) instead of CSI (\x1b[...). curses-based TUIs such as
    // nvitop / htop enable it and expect SS3 — without this branch
    // arrow-key row selection silently does nothing.
    match key {
        Key::Enter => Some(b"\r".to_vec()),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Backspace => Some(vec![0x7f]),
        Key::Escape => Some(vec![0x1b]),
        Key::ArrowUp if app_cursor => Some(b"\x1bOA".to_vec()),
        Key::ArrowDown if app_cursor => Some(b"\x1bOB".to_vec()),
        Key::ArrowRight if app_cursor => Some(b"\x1bOC".to_vec()),
        Key::ArrowLeft if app_cursor => Some(b"\x1bOD".to_vec()),
        Key::ArrowUp => Some(b"\x1b[A".to_vec()),
        Key::ArrowDown => Some(b"\x1b[B".to_vec()),
        Key::ArrowRight => Some(b"\x1b[C".to_vec()),
        Key::ArrowLeft => Some(b"\x1b[D".to_vec()),
        Key::Home if app_cursor => Some(b"\x1bOH".to_vec()),
        Key::End if app_cursor => Some(b"\x1bOF".to_vec()),
        Key::Home => Some(b"\x1b[H".to_vec()),
        Key::End => Some(b"\x1b[F".to_vec()),
        Key::PageUp => Some(b"\x1b[5~".to_vec()),
        Key::PageDown => Some(b"\x1b[6~".to_vec()),
        Key::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}


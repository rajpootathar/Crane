//! TerminalView — a native warpui `View` that owns a `TerminalController`,
//! snapshots the grid each frame into a `GridElement`, and routes key
//! input to the PTY via an `EventHandler`.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

// ── URL scanning ─────────────────────────────────────────────────────────────

/// One HTTP/HTTPS URL detected in a row of the visible grid. `col_end` is
/// exclusive. Ported from `src/terminal/view.rs:15-96`.
struct UrlHit {
    col_start: usize,
    col_end: usize,
    url: String,
}

/// Scan a row of plain text for `http://` / `https://` URLs. Stops at
/// whitespace and trims trailing punctuation that is almost never part of the
/// URL itself. Ported verbatim from `src/terminal/view.rs:43-96`.
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
        // Require a word-boundary before the scheme to avoid mid-word matches.
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
        // Trim trailing sentence punctuation.
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
            hits.push(UrlHit { col_start: i, col_end: end, url });
        }
        i = end.max(i + 1);
    }
    hits
}

/// One local file path detected in a row of the visible grid. Only paths that
/// resolve to something on disk are kept — path detection without the existence
/// check would underline every dotted identifier in the output. `line` / `col`
/// carry an optional `:LINE[:COL]` suffix parsed from compiler-style references
/// (rustc / tsc / Claude Code). `path` is already resolved against the pane cwd.
/// Ported from `src/terminal/view.rs:28-36,166-224`.
struct PathHit {
    col_start: usize,
    col_end: usize,
    path: std::path::PathBuf,
    line: Option<u32>,
    col: Option<u32>,
}

/// Split a token into its path part and optional `:LINE[:COL]` suffix. Accepts
/// `path:N` and `path:N:M` where N/M are all digits; anything else falls through
/// as a plain path with no line info. Ported from `src/terminal/view.rs:98-118`.
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

/// True when `s` looks like a bare filename (`main.rs`, `README.md`). The caller
/// also accepts tokens that contain `/` or start with `~`, so this only needs to
/// catch the no-separator case — reject dotted identifiers like `v1.2.3` or
/// `Self.method` by requiring a short ASCII-alphanumeric extension. Ported from
/// `src/terminal/view.rs:124-137`.
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

/// Resolve a terminal-emitted path token against the pane's cwd. `~` / `~/…`
/// expand via `$HOME`; absolute tokens pass through; anything else is relative to
/// `cwd`. Not canonicalized (would flatten symlinks the user clicked on purpose).
/// Ported from `src/terminal/view.rs:141-158`.
fn resolve_path(token: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    if token == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    let p = std::path::Path::new(token);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Scan a row of plain text for references to paths that exist on disk.
/// Deliberately aggressive syntactically (any whitespace-separated token that
/// contains `/`, a tilde prefix, or a plausible extension) then filtered by
/// `Path::exists()` — the stat check is the load-bearing part. URLs are skipped
/// so the URL scanner stays the authority for those. Ported from
/// `src/terminal/view.rs:166-224`.
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

// ─────────────────────────────────────────────────────────────────────────────

use crane_term::index::{Column as TermColumn, Line as TermLine, Point as TermPoint, Side};
use crane_term::selection::{expand_to_line, expand_to_word, Selection, SelectionAnchor, SelectionType};
use crane_term::{Flags, TermMode};

use warpui::elements::{
    DispatchEventResult, Element, EventHandler, Expanded, Flex, ParentElement,
};
use warpui::fonts::FamilyId;
use warpui::keymap::Keystroke;
use warpui::r#async::SpawnedLocalStream;
use warpui::{
    AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::warpui::color;
use crate::warpui::controller::{TerminalController, Wake};
use crate::warpui::grid_element::{GridCell, GridElement, MouseSelPhase};
use crate::warpui::input::keystroke_to_pty_bytes;


/// Ring the macOS system alert sound (the classic "beep"). `NSBeep` is a free
/// AppKit C function; AppKit is already linked via objc2-app-kit, so no new
/// dependency is needed. No-op on non-macOS targets.
#[cfg(target_os = "macos")]
fn system_beep() {
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSBeep();
    }
    // Safe: NSBeep takes no arguments and is callable from the main thread,
    // which is where render() runs.
    unsafe { NSBeep() }
}

/// True when `col` looks like a TUI vertical divider: a box-drawing vertical
/// bar occupies it on ≥60% of the visible rows. Ported verbatim from old
/// `src/terminal/view.rs:271-287`.
fn is_separator_column(term: &crane_term::Term, col: usize, rows: usize) -> bool {
    if rows == 0 {
        return false;
    }
    let mut hits = 0usize;
    for r in 0..rows {
        let c = term.grid.cell_at(r, col).map(|c| c.ch).unwrap_or(' ');
        if matches!(c, '│' | '┃' | '║' | '╎' | '╏' | '╽' | '╿') {
            hits += 1;
        }
    }
    hits * 5 >= rows * 3
}

/// True when the start cell sits between a vertical separator on its left and
/// one on its right (i.e. inside a bordered TUI column). Ported verbatim from
/// old `src/terminal/view.rs:289-294`.
fn is_inside_vertical_separators(term: &crane_term::Term, start_col: usize, rows: usize) -> bool {
    let total_cols = term.grid.columns;
    let has_left = (0..start_col).any(|c| is_separator_column(term, c, rows));
    let has_right = (start_col + 1..total_cols).any(|c| is_separator_column(term, c, rows));
    has_left && has_right
}

pub struct TerminalView {
    font_family: FamilyId,
    controller: Rc<RefCell<TerminalController>>,
    /// Cols/rows that fit the pane, written by GridElement::layout and
    /// applied here on the next frame (decouples &mut resize from the
    /// immutable layout/paint borrow).
    desired: Rc<StdCell<Option<(usize, usize)>>>,
    /// Project cwd requested by a sidebar click; render respawns the
    /// terminal here when it differs from `current_cwd`.
    requested_cwd: Rc<RefCell<Option<std::path::PathBuf>>>,
    current_cwd: RefCell<Option<std::path::PathBuf>>,
    /// Repaint waker, reused when respawning the controller.
    wake: Wake,
    /// Fractional scrollback position in LINES (0 = live/bottom), kept across
    /// scroll events so trackpad sub-line deltas accumulate — Warp's approach:
    /// the position itself carries the fraction; we truncate to integer rows only
    /// when calling `scroll_display`.
    scroll_pos: Rc<StdCell<f32>>,
    /// Fractional line accumulator for mouse/alt-screen forwarding (SGR events /
    /// PageUp-Down), which are discrete and can't take sub-line deltas.
    page_accum: Rc<StdCell<f32>>,
    /// Persisted drag state for the scrollbar thumb (element is rebuilt each frame).
    scrollbar_drag: Rc<StdCell<bool>>,
    /// Persisted drag state for mouse text selection (element is rebuilt each frame).
    sel_dragging: Rc<StdCell<bool>>,
    /// Last mouse-down instant + viewport position for consecutive-click detection.
    last_click: Rc<RefCell<Option<(std::time::Instant, usize, usize)>>>,
    /// Consecutive click count (1 = simple, 2 = word, 3+ = line).
    click_count: Rc<StdCell<u32>>,
    _repaint: SpawnedLocalStream,
    /// Hovered URL span: (row, col_start, col_end). Persists across per-frame
    /// rebuilds so GridElement can draw the underline between MouseMoved events.
    url_hover: Rc<StdCell<Option<(usize, usize, usize)>>>,
    /// Link target that was pressed at the last LeftMouseDown (click-without-drag
    /// detection). URL or resolved file path.
    link_pressed: Rc<RefCell<Option<crate::warpui::grid_element::LinkTarget>>>,
    /// Whether LeftMouseDragged fired since the last LeftMouseDown.
    url_did_drag: Rc<StdCell<bool>>,
    /// The (project_idx, worktree_idx, tab_id) this terminal currently lives in,
    /// synced by the shell from its authoritative `layouts` map. Attached to the
    /// `TermNotification` / `TermBell` this view dispatches so the shell can flag
    /// attention on the *source* tab (not the active one). `None` until the shell
    /// first syncs it.
    owner_key: Rc<StdCell<Option<(usize, usize, usize)>>>,
}

impl TerminalView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: Wake = Arc::new(move || {
            let _ = tx.try_send(());
        });
        Self::new_with(ctx, Rc::new(RefCell::new(None)), wake, rx)
    }

    /// Like `new`, but driven by a shared `requested_cwd` the shell sets, plus
    /// a shared `wake`/`rx` so the SHELL can also ping a repaint (e.g. when a
    /// tab click changes the cwd — the terminal respawns immediately instead of
    /// waiting for the next PTY byte).
    pub fn new_with(
        ctx: &mut ViewContext<Self>,
        requested_cwd: Rc<RefCell<Option<std::path::PathBuf>>>,
        wake: Wake,
        rx: async_channel::Receiver<()>,
    ) -> Self {
        let font_family = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| crate::warpui::bundled_fonts::mono(cache));
        ctx.focus_self();

        // Spawn directly in the initial requested cwd (avoids the
        // spawn-in-$HOME-then-respawn double start).
        let initial = requested_cwd.borrow().clone();
        let controller = TerminalController::new(80, 24, initial.as_deref(), wake.clone())
            .expect("spawn terminal");
        // Reader-thread wake -> repaint. Also the drain point for OSC 9 / OSC 777
        // desktop notifications the reader thread buffered on the controller: each
        // is forwarded to the shell as a `CraneShellAction::TermNotification` (the
        // shell renders the toast — not here). The dispatch is attributed to this
        // TerminalView's view_id, so the shell can map it back to the source pane.
        let repaint = ctx.spawn_stream_local(
            rx,
            |this, _item, ctx| {
                let source = this.owner_key.get();
                let notes = this.controller.borrow().take_notifications();
                for n in notes {
                    ctx.dispatch_typed_action(
                        &crate::warpui::shell::CraneShellAction::TermNotification {
                            body: n.body,
                            urgent: n.urgent,
                            source,
                        },
                    );
                }
                // Bell attention path: a background terminal ringing BEL is a
                // legit "wants attention" signal. Drained via the dedicated
                // `bell_notify` latch (NOT the audible-bell latch the paint path
                // owns, so the system beep is untouched). The shell only pulses
                // when `source` isn't the active tab.
                if this.controller.borrow().take_bell_notify() {
                    ctx.dispatch_typed_action(
                        &crate::warpui::shell::CraneShellAction::TermBell { source },
                    );
                }
                ctx.notify();
            },
            |_this, _ctx| {},
        );

        Self {
            font_family,
            controller: Rc::new(RefCell::new(controller)),
            desired: Rc::new(StdCell::new(None)),
            requested_cwd,
            current_cwd: RefCell::new(initial),
            wake,
            scroll_pos: Rc::new(StdCell::new(0.0)),
            page_accum: Rc::new(StdCell::new(0.0)),
            scrollbar_drag: Rc::new(StdCell::new(false)),
            sel_dragging: Rc::new(StdCell::new(false)),
            last_click: Rc::new(RefCell::new(None)),
            click_count: Rc::new(StdCell::new(0)),
            _repaint: repaint,
            url_hover: Rc::new(StdCell::new(None)),
            link_pressed: Rc::new(RefCell::new(None)),
            url_did_drag: Rc::new(StdCell::new(false)),
            owner_key: Rc::new(StdCell::new(None)),
        }
    }

    /// A cloned handle to this terminal's owner-tab cell. The shell sets it from
    /// its `layouts` map so dispatched notifications carry the right source tab.
    pub fn owner_cell(&self) -> Rc<StdCell<Option<(usize, usize, usize)>>> {
        self.owner_key.clone()
    }

    /// Restore a terminal from a persisted session: spawn in `cwd`, then replay
    /// the saved ANSI scrollback so it comes back looking as it did.
    pub fn new_restore(
        ctx: &mut ViewContext<Self>,
        cwd: std::path::PathBuf,
        history: String,
    ) -> Self {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: Wake = Arc::new(move || {
            let _ = tx.try_send(());
        });
        let view = Self::new_with(ctx, Rc::new(RefCell::new(Some(cwd))), wake, rx);
        view.controller.borrow().replay(&history);
        view
    }

    /// ANSI snapshot of the scrollback + grid, for session persistence.
    pub fn snapshot(&self) -> String {
        self.controller.borrow().snapshot()
    }

    /// The terminal's spawn directory (persisted for restore).
    pub fn cwd(&self) -> std::path::PathBuf {
        self.controller.borrow().cwd.clone()
    }

    /// True when a foreground program (alt-screen TUI: vim, htop, less, …) owns
    /// the viewport — a proxy for "a process is running" used by the quit /
    /// close-pane confirmation modals. See `TerminalController::has_foreground_process`.
    pub fn has_foreground_process(&self) -> bool {
        self.controller.borrow().has_foreground_process()
    }

    /// The terminal's OSC-0/OSC-2 window title, if a program set one. Used by
    /// the shell to label a terminal Tab with the running command / cwd.
    pub fn title(&self) -> Option<String> {
        self.controller.borrow().title()
    }

    /// Copy the current terminal text selection to a string. Returns `None` when
    /// there is no selection or it covers no characters.
    pub fn copy_selection(&self) -> Option<String> {
        self.controller.borrow().term.lock().selection_to_string()
    }

    /// Select the entire grid: a Simple selection from (Line 0, Col 0) to
    /// (Line rows-1, Col cols-1). Mirrors old `src/terminal/view.rs:1453-1461`
    /// (Cmd+A). Called by the shell for the focused terminal pane.
    pub fn select_all(&self) {
        let ctrl = self.controller.borrow();
        let mut t = ctrl.term.lock();
        let rows = t.grid.visible_rows;
        let cols = t.grid.columns;
        let start = TermPoint::new(TermLine(0), TermColumn(0));
        let end = TermPoint::new(
            TermLine(rows.saturating_sub(1) as i32),
            TermColumn(cols.saturating_sub(1)),
        );
        let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
        sel.update(end, Side::Right);
        t.selection = Some(sel);
        drop(t);
        (self.wake)();
    }
}

impl Entity for TerminalView {
    type Event = ();
}

impl View for TerminalView {
    fn ui_name() -> &'static str {
        "TerminalView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        // Respawn the terminal in a newly-selected project directory.
        {
            let req = self.requested_cwd.borrow().clone();
            if req != *self.current_cwd.borrow() {
                if let Some(path) = req.as_ref() {
                    if let Ok(c) =
                        TerminalController::new(80, 24, Some(path.as_path()), self.wake.clone())
                    {
                        *self.controller.borrow_mut() = c;
                    }
                }
                *self.current_cwd.borrow_mut() = req;
            }
        }

        // Apply a resize requested by the previous frame's layout pass.
        if let Some((c, r)) = self.desired.get() {
            let mut ctrl = self.controller.borrow_mut();
            if ctrl.cols != c || ctrl.rows != r {
                ctrl.resize(c, r);
            }
        }

        // Snapshot the viewport (scrollback-aware) into owned cells.
        let default_fg = color::default_fg();
        let default_bg = color::default_bg();
        let (cells, rows, cols, cursor, sel_range, disp_off, cursor_style) = {
            let ctrl = self.controller.borrow();
            let t = ctrl.term.lock();
            let cols = t.grid.columns;
            let rows = t.grid.visible_rows;
            let cursor_style = t.cursor_style();
            let blank = GridCell {
                ch: ' ',
                fg: default_fg,
                bg: default_bg,
                is_wide: false,
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                hidden: false,
                strikethrough: false,
            };
            let mut cells = vec![blank; rows * cols];

            // Drive from renderable_content() so scrollback (display_offset)
            // is honored; viewport_row = point.line + display_offset.
            let rc = t.renderable_content();
            let display_offset = rc.display_offset as i32;
            let cursor_pt = rc.cursor.point;
            let cursor_visible = rc.cursor.visible;
            for rcell in rc {
                let vr = rcell.point.line.0 + display_offset;
                if vr < 0 || vr as usize >= rows {
                    continue;
                }
                let col = rcell.point.column.0;
                if col >= cols {
                    continue;
                }
                let cell = rcell.cell;
                let mut fg = color::term_color_to_coloru(cell.fg, true);
                let mut bg = color::term_color_to_coloru(cell.bg, false);
                if cell.flags.contains(Flags::INVERSE) {
                    // Default-aware swap so inverted text stays readable
                    // against the theme bg (mirrors view.rs::color_to_egui).
                    let swapped_bg = if fg == default_bg { default_fg } else { fg };
                    let swapped_fg = if bg == default_bg { default_bg } else { bg };
                    fg = swapped_fg;
                    bg = swapped_bg;
                }
                cells[vr as usize * cols + col] = GridCell {
                    ch: cell.ch,
                    fg,
                    bg,
                    is_wide: cell.flags.contains(Flags::WIDE_CHAR),
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.contains(Flags::UNDERLINE),
                    dim: cell.flags.contains(Flags::DIM),
                    hidden: cell.flags.contains(Flags::HIDDEN),
                    strikethrough: cell.flags.contains(Flags::STRIKEOUT),
                };
            }

            let cursor = if cursor_visible {
                let cr = cursor_pt.line.0 + display_offset;
                let cc = cursor_pt.column.0;
                if cr >= 0 && (cr as usize) < rows && cc < cols {
                    Some((cr as usize, cc))
                } else {
                    None
                }
            } else {
                None
            };

            let sel_range = t.selection.as_ref().map(|s| s.to_range());
            let disp_off = t.grid.display_offset as i32;

            (cells, rows, cols, cursor, sel_range, disp_off, cursor_style)
        };

        // Ring the system bell if a BEL arrived since the last frame. Drained
        // unconditionally (even off-macOS) so it can't re-trigger; the audible
        // chime is macOS-only (NSBeep).
        if self.controller.borrow().take_bell() {
            #[cfg(target_os = "macos")]
            system_beep();
        }

        // Scan visible rows for clickable links: HTTP/HTTPS URLs and on-disk file
        // paths (absolute / `~` / repo-relative, optional `:LINE[:COL]`). Paths
        // resolve against the terminal's cwd and are kept only when they exist.
        // URLs win on overlap (a URL is a strictly more specific match than a
        // token-with-a-dot), so a path hit overlapping a URL span is dropped.
        use crate::warpui::grid_element::{LinkSpan, LinkTarget};
        let link_spans: Vec<LinkSpan> = {
            let cwd = self.controller.borrow().cwd.clone();
            let mut spans = Vec::new();
            for r in 0..rows {
                let row_text: String = (0..cols).map(|c| cells[r * cols + c].ch).collect();
                let mut url_ranges: Vec<(usize, usize)> = Vec::new();
                for hit in scan_urls(&row_text) {
                    url_ranges.push((hit.col_start, hit.col_end));
                    spans.push(LinkSpan {
                        row: r,
                        col_start: hit.col_start,
                        col_end: hit.col_end,
                        target: LinkTarget::Url(hit.url),
                    });
                }
                for ph in scan_paths(&row_text, &cwd) {
                    let overlaps_url = url_ranges
                        .iter()
                        .any(|&(s, e)| ph.col_start < e && s < ph.col_end);
                    if overlaps_url {
                        continue;
                    }
                    spans.push(LinkSpan {
                        row: r,
                        col_start: ph.col_start,
                        col_end: ph.col_end,
                        target: LinkTarget::File {
                            path: ph.path,
                            line: ph.line,
                            col: ph.col,
                        },
                    });
                }
            }
            spans
        };

        // Build the mouse-selection callback. Captures Rc-cloned state from the
        // view so it survives the per-frame element rebuild.
        let sel_ctrl = self.controller.clone();
        let sel_wake = self.wake.clone();
        let last_click = self.last_click.clone();
        let click_count = self.click_count.clone();
        let grid_cols = cols;
        let grid_rows = rows;
        let mouse_sel_cb: Rc<dyn Fn(MouseSelPhase, usize, usize, Side, bool)> =
            Rc::new(move |phase, vrow, vcol, side, shift| {
                match phase {
                    MouseSelPhase::Down => {
                        // Consecutive-click detection (double = word, triple = line).
                        // 500ms window matches old `view.rs:918`.
                        let now = std::time::Instant::now();
                        let count = {
                            let mut last = last_click.borrow_mut();
                            let prev = click_count.get();
                            let new_count = match *last {
                                Some((t, pr, pc))
                                    if now.duration_since(t)
                                        < std::time::Duration::from_millis(500)
                                        && pr == vrow
                                        && pc == vcol =>
                                {
                                    prev + 1
                                }
                                _ => 1,
                            };
                            *last = Some((now, vrow, vcol));
                            click_count.set(new_count);
                            new_count
                        };

                        let ctrl = sel_ctrl.borrow();
                        let mut t = ctrl.term.lock();
                        let disp = t.grid.display_offset as i32;
                        let term_line = vrow as i32 - disp;
                        let pt =
                            TermPoint::new(TermLine(term_line), TermColumn(vcol.min(grid_cols.saturating_sub(1))));

                        // Shift+click extends an existing selection to the click
                        // point instead of starting a fresh one (old `view.rs:927`).
                        if shift && t.selection.is_some() {
                            if let Some(sel) = t.selection.as_mut() {
                                sel.update(pt, side);
                            }
                            drop(t);
                            (sel_wake)();
                            return;
                        }

                        let sel = if count >= 3 {
                            // Triple click: select the whole line.
                            let range = expand_to_line(pt, grid_cols);
                            Selection {
                                kind: SelectionType::Lines,
                                anchor: SelectionAnchor {
                                    point: range.start,
                                    side: Side::Left,
                                },
                                active: SelectionAnchor {
                                    point: range.end,
                                    side: Side::Right,
                                },
                            }
                        } else if count == 2
                            && term_line >= 0
                            && (term_line as usize) < grid_rows
                        {
                            // Double click: expand to the word under the cursor.
                            let row_idx = term_line as usize;
                            let range = expand_to_word(pt, grid_cols, |c| {
                                t.grid
                                    .cell_at(row_idx, c)
                                    .map(|cell| cell.ch)
                                    .unwrap_or(' ')
                            });
                            Selection {
                                kind: SelectionType::Semantic,
                                anchor: SelectionAnchor {
                                    point: range.start,
                                    side: Side::Left,
                                },
                                active: SelectionAnchor {
                                    point: range.end,
                                    side: Side::Right,
                                },
                            }
                        } else {
                            // Single click: start a drag selection. If the start
                            // cell sits between two TUI vertical separators
                            // (lazygit/k9s column divider), promote to Block so
                            // dragging one column stays rectangular
                            // (old `view.rs:886-895`).
                            let kind = if is_inside_vertical_separators(
                                &t,
                                pt.column.0,
                                grid_rows,
                            ) {
                                SelectionType::Block
                            } else {
                                SelectionType::Simple
                            };
                            Selection::new(kind, pt, side)
                        };
                        t.selection = Some(sel);
                        drop(t);
                        (sel_wake)();
                    }
                    MouseSelPhase::Drag => {
                        let ctrl = sel_ctrl.borrow();
                        let mut t = ctrl.term.lock();
                        let disp = t.grid.display_offset as i32;
                        let term_line = vrow as i32 - disp;
                        let pt = TermPoint::new(
                            TermLine(term_line),
                            TermColumn(vcol.min(grid_cols.saturating_sub(1))),
                        );
                        if let Some(ref mut sel) = t.selection {
                            sel.update(pt, side);
                        }
                        drop(t);
                        (sel_wake)();
                    }
                    MouseSelPhase::Up => {
                        // Clear the selection when the click produced no drag range.
                        let ctrl = sel_ctrl.borrow();
                        let mut t = ctrl.term.lock();
                        if t.selection.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
                            t.selection = None;
                        }
                        drop(t);
                        (sel_wake)();
                    }
                }
            });

        // Mouse-reporting mode: when a TUI owns the mouse, forward SGR clicks
        // rather than starting a text selection. Same mode set the scroll path
        // already recognises (click / drag / motion).
        let mouse_on = {
            let ctrl = self.controller.borrow();
            let t = ctrl.term.lock();
            t.mode_contains(TermMode::MOUSE_REPORT_CLICK)
                || t.mode_contains(TermMode::MOUSE_DRAG)
                || t.mode_contains(TermMode::MOUSE_MOTION)
        };
        let mouse_report_cb: Option<Rc<dyn Fn(bool, usize, usize)>> = if mouse_on {
            let ctrl = self.controller.clone();
            Some(Rc::new(move |press: bool, col: usize, row: usize| {
                // SGR: `\x1b[<0;COL;ROW M` press / `... m` release, 1-based.
                let tail = if press { 'M' } else { 'm' };
                let seq = format!("\x1b[<0;{col};{row}{tail}");
                ctrl.borrow().write_input(seq.as_bytes());
            }))
        } else {
            None
        };

        let grid = GridElement::new(
            rows,
            cols,
            cells,
            cursor,
            self.font_family,
            crate::warpui::fontsize::base(),
            color::default_bg(),
            color::cursor_color(),
            self.desired.clone(),
        )
        .with_selection(sel_range, disp_off)
        .with_cursor_style(cursor_style.shape, cursor_style.blink)
        .on_mouse_report(mouse_report_cb)
        .on_mouse_select(self.sel_dragging.clone(), mouse_sel_cb)
        .with_link_spans(
            link_spans,
            self.url_hover.clone(),
            self.link_pressed.clone(),
            self.url_did_drag.clone(),
        );

        // Scrollbar metrics from crane_term (rows, not pixels). In alt-screen
        // (vim/less/htop) there's no scrollback and the app owns its own
        // viewport, so — like real terminals — show NO thumb (total == viewport).
        let (sb_len, sb_disp_off, alt) = {
            let ctrl = self.controller.borrow();
            let term = ctrl.term.lock();
            (
                term.scrollback_len(),
                term.display_offset(),
                term.is_alt_screen(),
            )
        };
        let (total, top) = if alt {
            (rows, 0)
        } else {
            (sb_len + rows, sb_len.saturating_sub(sb_disp_off))
        };
        let mut scrollbar_el =
            crate::warpui::scrollbar_element::LineScrollbar::new(
                total,
                rows,
                top,
                crate::warpui::theme::border(),
            );
        // Draggable thumb on the main screen (scrollback). In alt-screen there's
        // nothing to drag (the app owns its viewport), so leave it display-only.
        if !alt && sb_len > 0 {
            let ctrl = self.controller.clone();
            let wake = self.wake.clone();
            let sb = sb_len;
            let on_scroll: std::rc::Rc<dyn Fn(f32)> = std::rc::Rc::new(move |frac: f32| {
                // frac 0.0 = top (oldest, max offset), 1.0 = bottom (live, offset 0).
                let target = ((1.0 - frac) * sb as f32).round().clamp(0.0, sb as f32) as usize;
                let c = ctrl.borrow();
                let cur = c.term.lock().display_offset();
                let delta = target as i32 - cur as i32;
                if delta != 0 {
                    c.term.lock().scroll_display(delta);
                    (wake)();
                }
            });
            scrollbar_el = scrollbar_el.draggable(self.scrollbar_drag.clone(), on_scroll);
        }
        let scrollbar = scrollbar_el.finish();

        let scroll_ctrl = self.controller.clone();
        let scroll_wake = self.wake.clone();
        let scroll_pos = self.scroll_pos.clone();
        let page_accum = self.page_accum.clone();
        // Faithful port of Warp's terminal scroll (block_list_element::scroll_internal):
        //   precise (trackpad):  delta_lines = delta.y() / cell_height   (fractional)
        //   non-precise (wheel):  delta_lines = delta.y()                (already lines)
        // NO x40 (that's only the generic Scrollable wrapper, which the terminal
        // bypasses). Positive delta.y() = scroll up. Warp keeps scroll_top as
        // fractional lines across events; we mirror that by keeping `scroll_pos`
        // (fractional display_offset) and truncating to integer rows on apply.
        let cell_h = crate::warpui::fontsize::base() * 1.2;
        let scroll_cb: std::rc::Rc<dyn Fn(f32, bool)> = std::rc::Rc::new(move |dy: f32, precise: bool| {
            let delta_lines = if precise { dy / cell_h } else { dy };
            let ctrl = scroll_ctrl.borrow();
            let (alt, mouse, max, cur) = {
                let t = ctrl.term.lock();
                let mouse = t.mode_contains(TermMode::MOUSE_REPORT_CLICK)
                    || t.mode_contains(TermMode::MOUSE_DRAG)
                    || t.mode_contains(TermMode::MOUSE_MOTION);
                (t.is_alt_screen(), mouse, t.scrollback_len(), t.display_offset())
            };
            if mouse {
                // Mouse-aware app: forward SGR wheel events, one per whole line.
                let acc = page_accum.get() + delta_lines;
                let lines = acc.trunc() as i32;
                page_accum.set(acc - lines as f32);
                if lines != 0 {
                    let btn = if lines > 0 { 64 } else { 65 };
                    let mut seq = String::new();
                    for _ in 0..lines.unsigned_abs().min(8) {
                        seq.push_str(&format!("\x1b[<{btn};1;1M"));
                    }
                    ctrl.write_input(seq.as_bytes());
                }
                return;
            }
            if alt {
                // Alt-screen app without mouse (less/man/vim): one PageUp/Down
                // per ~8 accumulated lines (it only understands page keys).
                const LINES_PER_PAGE: f32 = 8.0;
                let acc = page_accum.get() + delta_lines;
                let pages = (acc / LINES_PER_PAGE).trunc() as i32;
                page_accum.set(acc - pages as f32 * LINES_PER_PAGE);
                if pages != 0 {
                    let key: &[u8] = if pages > 0 { b"\x1b[5~" } else { b"\x1b[6~" };
                    for _ in 0..pages.unsigned_abs().min(2) {
                        ctrl.write_input(key);
                    }
                }
                return;
            }
            // Main screen: fractional scrollback position (Warp's f64 scroll_top).
            // display_offset: 0 = live/bottom, `max` = fully scrolled up. Positive
            // delta_lines scrolls up -> increases display_offset.
            let cur_f = cur as f32;
            // Resync if the terminal moved the offset itself (typing snaps to bottom).
            if (scroll_pos.get() - cur_f).abs() >= 1.0 {
                scroll_pos.set(cur_f);
            }
            let pos = (scroll_pos.get() + delta_lines).clamp(0.0, max as f32);
            scroll_pos.set(pos);
            let delta_rows = pos.round() as i32 - cur as i32;
            if delta_rows != 0 {
                ctrl.term.lock().scroll_display(delta_rows);
                (scroll_wake)();
            }
        });
        let term_body = EventHandler::new(grid.on_scroll(scroll_cb).finish())
            // ALL key handling is routed by the SHELL to the focused pane (the
            // shell knows which pane is active; warpui per-view focus proved
            // unreliable across panes). So just bubble keys up.
            .on_keydown(move |_ctx, _app, _ks: &Keystroke| DispatchEventResult::PropagateToParent)
            .finish();
        Flex::row()
            .with_child(Expanded::new(1.0, term_body).finish())
            .with_child(scrollbar)
            .finish()
    }
}

impl TerminalView {
    /// Write a keystroke to THIS terminal's PTY (called by the shell for the
    /// focused pane).
    pub fn write_keystroke(&self, ks: &Keystroke) {
        let ctrl = self.controller.borrow();
        if !ctrl.is_alive() {
            return;
        }
        let app_cursor = ctrl.term.lock().is_app_cursor();
        if let Some(bytes) = keystroke_to_pty_bytes(ks, app_cursor) {
            ctrl.write_input(&bytes);
        }
    }

    /// Paste text into THIS terminal (bracketed when the app requested it).
    pub fn paste_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let ctrl = self.controller.borrow();
        let bracketed = ctrl.term.lock().is_bracketed_paste();
        let bytes = if bracketed {
            let mut b = b"\x1b[200~".to_vec();
            b.extend_from_slice(text.as_bytes());
            b.extend_from_slice(b"\x1b[201~");
            b
        } else {
            text.as_bytes().to_vec()
        };
        ctrl.write_input(&bytes);
    }

    /// Clear THIS terminal — two-regime Cmd+K clear:
    /// • Alt-screen / TUI active: erase scrollback only (`\x1b[3J`).
    /// • Bare shell: cursor home + erase display + erase scrollback + Ctrl+L.
    pub fn clear_screen(&self) {
        self.controller.borrow().clear_screen_two_regime();
    }
}

#[derive(Debug, Clone)]
pub enum TerminalViewAction {
    /// Cmd+V — paste clipboard text (bracketed when the app requested it).
    Paste,
    /// Cmd+K — clear the screen (Ctrl+L: shell clears + redraws prompt).
    Clear,
}

impl TypedActionView for TerminalView {
    type Action = TerminalViewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            TerminalViewAction::Paste => {
                let text = ctx.clipboard().read().plain_text;
                if text.is_empty() {
                    return;
                }
                let ctrl = self.controller.borrow();
                let bracketed = ctrl.term.lock().is_bracketed_paste();
                let bytes = if bracketed {
                    let mut b = b"\x1b[200~".to_vec();
                    b.extend_from_slice(text.as_bytes());
                    b.extend_from_slice(b"\x1b[201~");
                    b
                } else {
                    text.into_bytes()
                };
                ctrl.write_input(&bytes);
            }
            TerminalViewAction::Clear => {
                self.controller.borrow().clear_screen_two_regime();
            }
        }
    }
}

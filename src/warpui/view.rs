//! TerminalView — a native warpui `View` that owns a `TerminalController`,
//! snapshots the grid each frame into a `GridElement`, and routes key
//! input to the PTY via an `EventHandler`.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

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

const FONT_SIZE: f32 = 14.0;

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
            .update(ctx, |cache, _| cache.load_system_font("Menlo").expect("load Menlo"));
        ctx.focus_self();

        // Spawn directly in the initial requested cwd (avoids the
        // spawn-in-$HOME-then-respawn double start).
        let initial = requested_cwd.borrow().clone();
        let controller = TerminalController::new(80, 24, initial.as_deref(), wake.clone())
            .expect("spawn terminal");
        let repaint =
            ctx.spawn_stream_local(rx, |_this, _item, ctx| ctx.notify(), |_this, _ctx| {});

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
        }
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

    /// Copy the current terminal text selection to a string. Returns `None` when
    /// there is no selection or it covers no characters.
    pub fn copy_selection(&self) -> Option<String> {
        self.controller.borrow().term.lock().selection_to_string()
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
        let (cells, rows, cols, cursor, sel_range, disp_off) = {
            let ctrl = self.controller.borrow();
            let t = ctrl.term.lock();
            let cols = t.grid.columns;
            let rows = t.grid.visible_rows;
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

            (cells, rows, cols, cursor, sel_range, disp_off)
        };

        // Build the mouse-selection callback. Captures Rc-cloned state from the
        // view so it survives the per-frame element rebuild.
        let sel_ctrl = self.controller.clone();
        let sel_wake = self.wake.clone();
        let last_click = self.last_click.clone();
        let click_count = self.click_count.clone();
        let grid_cols = cols;
        let grid_rows = rows;
        let mouse_sel_cb: Rc<dyn Fn(MouseSelPhase, usize, usize, Side)> =
            Rc::new(move |phase, vrow, vcol, side| {
                match phase {
                    MouseSelPhase::Down => {
                        // Consecutive-click detection (double = word, triple = line).
                        let now = std::time::Instant::now();
                        let count = {
                            let mut last = last_click.borrow_mut();
                            let prev = click_count.get();
                            let new_count = match *last {
                                Some((t, pr, pc))
                                    if now.duration_since(t)
                                        < std::time::Duration::from_millis(350)
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
                            // Single click: start a simple drag selection.
                            Selection::new(SelectionType::Simple, pt, side)
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

        let grid = GridElement::new(
            rows,
            cols,
            cells,
            cursor,
            self.font_family,
            FONT_SIZE,
            color::default_bg(),
            color::cursor_color(),
            self.desired.clone(),
        )
        .with_selection(sel_range, disp_off)
        .on_mouse_select(self.sel_dragging.clone(), mouse_sel_cb);

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
        const CELL_H: f32 = FONT_SIZE * 1.2;
        let scroll_cb: std::rc::Rc<dyn Fn(f32, bool)> = std::rc::Rc::new(move |dy: f32, precise: bool| {
            let delta_lines = if precise { dy / CELL_H } else { dy };
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

    /// Clear THIS terminal (Ctrl+L — shell clears + redraws prompt).
    pub fn clear_screen(&self) {
        self.controller.borrow().write_input(b"\x0c");
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
                self.controller.borrow().write_input(b"\x0c");
            }
        }
    }
}

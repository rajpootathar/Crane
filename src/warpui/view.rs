//! TerminalView — a native warpui `View` that owns a `TerminalController`,
//! snapshots the grid each frame into a `GridElement`, and routes key
//! input to the PTY via an `EventHandler`.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crane_term::Flags;

use warpui::elements::{DispatchEventResult, Element, EventHandler};
use warpui::fonts::FamilyId;
use warpui::keymap::Keystroke;
use warpui::r#async::SpawnedLocalStream;
use warpui::{
    AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::warpui::color;
use crate::warpui::controller::{TerminalController, Wake};
use crate::warpui::grid_element::{GridCell, GridElement};
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
            _repaint: repaint,
        }
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
        let (cells, rows, cols, cursor) = {
            let ctrl = self.controller.borrow();
            let t = ctrl.term.lock();
            let cols = t.grid.columns;
            let rows = t.grid.visible_rows;
            let blank = GridCell {
                ch: ' ',
                fg: default_fg,
                bg: default_bg,
                is_wide: false,
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
            (cells, rows, cols, cursor)
        };

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
        );

        let scroll_ctrl = self.controller.clone();
        let scroll_wake = self.wake.clone();
        EventHandler::new(grid.finish())
            .on_scroll_wheel(move |_ctx, _app, delta, _mods| {
                // Scroll up into scrollback (positive display_offset).
                let lines = (delta.y() / 10.0).round() as i32;
                if lines != 0 {
                    scroll_ctrl.borrow().term.lock().scroll_display(lines);
                    (scroll_wake)();
                }
                DispatchEventResult::StopPropagation
            })
            // ALL key handling is routed by the SHELL to the focused pane (the
            // shell knows which pane is active; warpui per-view focus proved
            // unreliable across panes). So just bubble keys up.
            .on_keydown(move |_ctx, _app, _ks: &Keystroke| DispatchEventResult::PropagateToParent)
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

//! Custom warpui Element that paints a terminal grid cell-by-cell:
//! per-cell background quad + glyph, plus a block cursor. Full repaint
//! from a cell snapshot every frame — no damage tracking, so it cannot
//! "drop" rows the way the egui grid did.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Process-lifetime epoch used to drive the cursor blink phase. A single shared
/// clock keeps every terminal's cursor blinking in unison.
fn blink_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

use crane_term::index::{Column as TermColumn, Line as TermLine, Point as TermPoint, Side};
use crane_term::selection::SelectionRange;
use crane_term::CursorShape;
use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties, Style, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint};

/// Phase of a mouse selection gesture.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MouseSelPhase {
    Down,
    Drag,
    Up,
}

/// What a clickable span in the terminal grid points at. A `Url` opens in the
/// system browser; a `File` is routed to the shell (Crane's editor) at the
/// optional `:LINE[:COL]` the CLI printed.
#[derive(Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File {
        path: std::path::PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// One clickable span (URL or file path) in the visible grid. `col_end` is
/// exclusive. Built by the View each frame; hover/click machinery is shared
/// between the two target kinds.
#[derive(Clone)]
pub struct LinkSpan {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub target: LinkTarget,
}

#[derive(Clone, Copy)]
pub struct GridCell {
    pub ch: char,
    pub fg: ColorU,
    pub bg: ColorU,
    /// True for the leading column of a double-width (CJK/emoji) glyph;
    /// the trailing WIDE_CHAR_SPACER column is a blank cell.
    pub is_wide: bool,
    /// SGR bold (weight: bold font variant).
    pub bold: bool,
    /// SGR italic (style: italic font variant).
    pub italic: bool,
    /// SGR underline (draw 1px line under glyph).
    pub underline: bool,
    /// SGR dim / faint — blend the glyph ~38% toward its background (see the
    /// paint path); readable on light and dark themes.
    pub dim: bool,
    /// SGR hidden / conceal (suppress glyph rendering).
    pub hidden: bool,
    /// SGR strikethrough (draw 1px line through cell mid-height).
    pub strikethrough: bool,
}

pub struct GridElement {
    rows: usize,
    cols: usize,
    cells: Vec<GridCell>, // rows*cols, row-major
    cursor: Option<(usize, usize)>, // (row, col), viewport-relative
    font_family: FamilyId,
    font_size: f32,
    line_height_ratio: f32,
    default_bg: ColorU,
    cursor_color: ColorU,
    /// DECSCUSR cursor shape (Block / Underline / Beam) — controls the cursor
    /// geometry drawn in paint().
    cursor_shape: CursorShape,
    /// DECSCUSR blink bit — when true, the cursor alpha oscillates on a ~1s
    /// period (500ms on / 500ms off); when false it is steady.
    cursor_blink: bool,
    /// Written in layout() with the cols/rows that fit the available
    /// space; the View reads this next frame to drive PTY/grid resize.
    desired: Rc<StdCell<Option<(usize, usize)>>>,
    /// Repaint waker fired when `desired` CHANGES in layout(). The resize is a
    /// two-frame dance (layout writes `desired`; the next render applies it), so
    /// a one-shot size change — a split/close that adds or removes a sibling
    /// pane — would leave the grid stale until the next incidental event. Waking
    /// on the change guarantees the follow-up frame that applies it.
    resize_wake: Option<crate::warpui::controller::Wake>,
    size: Option<Vector2F>,
    origin: Option<Point>,
    /// Paint-time origin in window coords, used in dispatch_event for hit-testing.
    origin_vec: Option<Vector2F>,
    cell_w: f32,
    cell_h: f32,
    /// Scroll-wheel callback: `(delta_y_points, precise)`. `precise` = trackpad
    /// (pixel-smooth), else mouse wheel (line steps). `None` = no scrolling.
    scroll_cb: Option<Rc<dyn Fn(f32, bool)>>,
    /// Active selection range to highlight, plus the display_offset used to
    /// convert viewport rows to terminal line numbers.
    selection: Option<SelectionRange>,
    display_offset: i32,
    /// Mouse selection callback: `(phase, viewport_row, col, side, shift)`.
    /// `shift` is the Shift-modifier state at a Down event (used to extend an
    /// existing selection); it is always `false` for Drag/Up.
    mouse_sel_cb: Option<Rc<dyn Fn(MouseSelPhase, usize, usize, Side, bool)>>,
    /// Shared drag state (persisted by the owning View across per-frame rebuilds).
    mouse_dragging: Rc<StdCell<bool>>,
    /// Clickable link spans (URLs + file paths) detected in the visible grid
    /// rows (built by the View each frame).
    link_spans: Vec<LinkSpan>,
    /// Which span is currently hovered: (row, col_start, col_end). Persisted
    /// across rebuilds so the underline is visible between MouseMoved events.
    url_hover: Rc<StdCell<Option<(usize, usize, usize)>>>,
    /// Link target recorded at the last LeftMouseDown (click-without-drag detection).
    link_pressed: Rc<RefCell<Option<LinkTarget>>>,
    /// True if LeftMouseDragged fired since the last LeftMouseDown. Shared so
    /// the View can also inspect it (e.g. suppress copy on drag-release).
    url_did_drag: Rc<StdCell<bool>>,
    /// When Some, the terminal has a mouse-reporting mode active: a
    /// LeftMouseDown/Up forwards an SGR mouse press/release (so ranger/mc/
    /// vim-mouse work) INSTEAD of starting a text selection. The callback
    /// receives `(press, col_1based, row_1based)`.
    mouse_report_cb: Option<Rc<dyn Fn(bool, usize, usize)>>,
}

impl GridElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rows: usize,
        cols: usize,
        cells: Vec<GridCell>,
        cursor: Option<(usize, usize)>,
        font_family: FamilyId,
        font_size: f32,
        default_bg: ColorU,
        cursor_color: ColorU,
        desired: Rc<StdCell<Option<(usize, usize)>>>,
    ) -> Self {
        Self {
            rows,
            cols,
            cells,
            cursor,
            font_family,
            font_size,
            line_height_ratio: 1.2,
            default_bg,
            cursor_color,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            desired,
            resize_wake: None,
            size: None,
            origin: None,
            origin_vec: None,
            cell_w: font_size * 0.6,
            cell_h: font_size * 1.2,
            scroll_cb: None,
            selection: None,
            display_offset: 0,
            mouse_sel_cb: None,
            mouse_dragging: Rc::new(StdCell::new(false)),
            link_spans: Vec::new(),
            url_hover: Rc::new(StdCell::new(None)),
            link_pressed: Rc::new(RefCell::new(None)),
            url_did_drag: Rc::new(StdCell::new(false)),
            mouse_report_cb: None,
        }
    }

    /// Attach a scroll-wheel handler that receives `(delta_y_points, precise)`.
    pub fn on_scroll(mut self, cb: Rc<dyn Fn(f32, bool)>) -> Self {
        self.scroll_cb = Some(cb);
        self
    }

    /// Repaint waker fired when the fitted grid size changes in layout() — so a
    /// one-shot resize (split/close) self-completes its two-frame apply.
    pub fn with_resize_wake(mut self, wake: crate::warpui::controller::Wake) -> Self {
        self.resize_wake = Some(wake);
        self
    }

    /// Set the DECSCUSR cursor shape + blink used when painting the cursor.
    pub fn with_cursor_style(mut self, shape: CursorShape, blink: bool) -> Self {
        self.cursor_shape = shape;
        self.cursor_blink = blink;
        self
    }

    /// Attach an SGR mouse-report callback. When set (i.e. a mouse-reporting
    /// mode is active), left clicks forward SGR press/release to the PTY instead
    /// of starting a text selection. `None` keeps the default selection path.
    pub fn on_mouse_report(mut self, cb: Option<Rc<dyn Fn(bool, usize, usize)>>) -> Self {
        self.mouse_report_cb = cb;
        self
    }

    /// Attach a selection range to highlight. `display_offset` converts viewport
    /// rows to terminal line numbers: `term_line = viewport_row - display_offset`.
    pub fn with_selection(
        mut self,
        sel: Option<SelectionRange>,
        display_offset: i32,
    ) -> Self {
        self.selection = sel;
        self.display_offset = display_offset;
        self
    }

    /// Attach a mouse-selection callback and persist the drag state across
    /// per-frame rebuilds. The callback receives `(phase, viewport_row, col, side)`.
    pub fn on_mouse_select(
        mut self,
        dragging: Rc<StdCell<bool>>,
        cb: Rc<dyn Fn(MouseSelPhase, usize, usize, Side, bool)>,
    ) -> Self {
        self.mouse_dragging = dragging;
        self.mouse_sel_cb = Some(cb);
        self
    }

    /// Attach clickable link spans (URLs + file paths) computed by the View,
    /// plus shared hover/press/drag state that persists across per-frame rebuilds.
    pub fn with_link_spans(
        mut self,
        spans: Vec<LinkSpan>,
        hover: Rc<StdCell<Option<(usize, usize, usize)>>>,
        pressed: Rc<RefCell<Option<LinkTarget>>>,
        did_drag: Rc<StdCell<bool>>,
    ) -> Self {
        self.link_spans = spans;
        self.url_hover = hover;
        self.link_pressed = pressed;
        self.url_did_drag = did_drag;
        self
    }
}

impl Element for GridElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let fc = app.font_cache();
        let font = fc.select_font(self.font_family, Properties::default());
        self.cell_w = fc
            .glyph_for_char(font, 'M', false)
            .and_then(|(gid, mfont)| fc.glyph_advance(mfont, self.font_size, gid).ok())
            .map(|a| a.x())
            .unwrap_or(self.font_size * 0.6);
        self.cell_h = fc.line_height(self.font_size, self.line_height_ratio);

        // How many cells fit the available space -> next-frame resize.
        let max = constraint.max;
        let fit_cols = (max.x() / self.cell_w).floor().max(1.0) as usize;
        let fit_rows = (max.y() / self.cell_h).floor().max(1.0) as usize;
        let fit = Some((fit_cols, fit_rows));
        if self.desired.get() != fit {
            self.desired.set(fit);
            // The fitted size changed since the last layout — the View applies
            // it on the NEXT render, so request that frame (a split/close is a
            // one-shot change with no follow-up event of its own).
            if let Some(wake) = &self.resize_wake {
                wake();
            }
        }

        // Occupy the available area (so the pane fills the window).
        let size = constraint.apply(max);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        let fc = app.font_cache();
        let font = fc.select_font(self.font_family, Properties::default());
        let ascent = fc.ascent(font, self.font_size);
        let descent = fc.descent(font, self.font_size); // negative
        let text_height = ascent - descent;
        let baseline = ((self.cell_h - text_height) * 0.5) + ascent;
        // Underline sits part-way into the descent region (font underline
        // metric); clamp so tiny/large fonts still get a sensible offset.
        let underline_off = ((-descent) * 0.5).clamp(1.0, 3.0);
        let (cw, ch) = (self.cell_w, self.cell_h);

        // 1) Whole-grid background + the single hit rect.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::Solid(self.default_bg));

        // 2) Per-cell backgrounds (only where != default).
        for r in 0..self.rows {
            for c in 0..self.cols {
                let cell = self.cells[r * self.cols + c];
                if cell.bg != self.default_bg {
                    let x = origin.x() + c as f32 * cw;
                    let y = origin.y() + r as f32 * ch;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(cw, ch)))
                        .with_background(Fill::Solid(cell.bg));
                }
            }
        }

        // 3) Selection highlight (over cell backgrounds, under cursor and glyphs).
        if let Some(sel) = self.selection {
            let sel_color = crate::warpui::theme::selection();
            let disp = self.display_offset;
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let term_line = r as i32 - disp;
                    let pt = TermPoint::new(TermLine(term_line), TermColumn(c));
                    if sel.contains(pt) {
                        let x = origin.x() + c as f32 * cw;
                        let y = origin.y() + r as f32 * ch;
                        ctx.scene
                            .draw_rect_without_hit_recording(
                                RectF::new(vec2f(x, y), vec2f(cw, ch)),
                            )
                            .with_background(Fill::Solid(sel_color));
                    }
                }
            }
        }

        // 4) Glyphs + decorations (underline, strikethrough).
        for r in 0..self.rows {
            for c in 0..self.cols {
                let cell = self.cells[r * self.cols + c];

                // SGR hidden: suppress the glyph entirely.
                if cell.hidden {
                    continue;
                }

                // Resolve fg color: the glyph keeps its own fg (the cursor is a
                // translucent overlay painted *after* this pass — no inversion).
                let mut fg = cell.fg;
                if cell.dim {
                    // SGR faint: de-emphasise by blending the glyph a fixed
                    // fraction toward its OWN background, opaquely. The old
                    // code halved the alpha, which composites ~50% toward
                    // whatever is painted behind — fine on a dark bg, but on a
                    // light theme that fades text toward white until it is
                    // unreadable. A 38% RGB blend keeps enough of the glyph's
                    // colour to stay legible on light and dark alike.
                    let mix = |f: u8, b: u8| (f as f32 * 0.62 + b as f32 * 0.38) as u8;
                    fg = ColorU::new(
                        mix(fg.r, cell.bg.r),
                        mix(fg.g, cell.bg.g),
                        mix(fg.b, cell.bg.b),
                        fg.a,
                    );
                }

                // Select the appropriate font variant for bold / italic.
                let props = match (cell.bold, cell.italic) {
                    (true, true) => Properties { weight: Weight::Bold, style: Style::Italic },
                    (true, false) => Properties { weight: Weight::Bold, style: Style::Normal },
                    (false, true) => Properties { weight: Weight::Normal, style: Style::Italic },
                    (false, false) => Properties::default(),
                };
                let cell_font = if props == Properties::default() {
                    font
                } else {
                    fc.select_font(self.font_family, props)
                };

                // Draw the glyph (skip whitespace).
                if cell.ch != ' ' && cell.ch != '\0' {
                    if let Some((gid, render_font)) = fc.glyph_for_char(cell_font, cell.ch, true) {
                        let pos = vec2f(
                            origin.x() + c as f32 * cw,
                            origin.y() + r as f32 * ch + baseline,
                        );
                        ctx.scene.draw_glyph(pos, gid, render_font, self.font_size, fg);
                    }
                }

                // Underline: 1px line at the font's underline metric (part-way
                // into the descent region), matching egui's built-in underline
                // position in the old renderer rather than a fixed +2px offset.
                if cell.underline {
                    let x = origin.x() + c as f32 * cw;
                    let y = origin.y() + r as f32 * ch + baseline + underline_off;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(cw, 1.0)))
                        .with_background(Fill::Solid(fg));
                }

                // Strikethrough: 1px line at cell vertical midpoint.
                if cell.strikethrough {
                    let x = origin.x() + c as f32 * cw;
                    let y = origin.y() + r as f32 * ch + ch * 0.5;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(cw, 1.0)))
                        .with_background(Fill::Solid(fg));
                }
            }
        }

        // 5) Cursor — a translucent overlay in terminal_fg (alpha 130) painted
        // *after* the glyphs so the character stays readable and is merely tinted
        // (matches old `view.rs:1282`). DECTCEM hides it (cursor is None). The
        // DECSCUSR shape decides the geometry: Block fills the cell (doubled over
        // a wide CJK/emoji cell), Beam is a ~2px bar at the left edge, Underline a
        // ~2px bar at the cell bottom. When blink is on, the alpha toggles on a
        // 500ms-on / 500ms-off cycle driven by a shared process clock.
        if let Some((cr, cc)) = self.cursor {
            if cr < self.rows && cc < self.cols {
                // Blink phase: default = drawn. When blinking, drop the frame
                // during the "off" half and schedule the next toggle so the
                // animation keeps running at idle (no PTY activity needed).
                let mut draw = true;
                if self.cursor_blink {
                    const HALF_MS: u128 = 500;
                    let ms = blink_epoch().elapsed().as_millis();
                    let phase = ms % (HALF_MS * 2);
                    draw = phase < HALF_MS;
                    let remaining = (HALF_MS - (phase % HALF_MS)) as u64;
                    ctx.repaint_after(Duration::from_millis(remaining.max(1)));
                }
                if draw {
                    let wide = self.cells[cr * self.cols + cc].is_wide;
                    let full_w = if wide { cw * 2.0 } else { cw };
                    let x = origin.x() + cc as f32 * cw;
                    let y = origin.y() + cr as f32 * ch;
                    let rect = match self.cursor_shape {
                        CursorShape::Block => RectF::new(vec2f(x, y), vec2f(full_w, ch)),
                        CursorShape::Beam => {
                            // ~2px vertical bar hugging the cell's left edge.
                            let bw = 2.0_f32.min(full_w);
                            RectF::new(vec2f(x, y), vec2f(bw, ch))
                        }
                        CursorShape::Underline => {
                            // ~2px horizontal bar along the cell bottom.
                            let bh = 2.0_f32.min(ch);
                            RectF::new(vec2f(x, y + ch - bh), vec2f(full_w, bh))
                        }
                    };
                    ctx.scene
                        .draw_rect_without_hit_recording(rect)
                        .with_background(Fill::Solid(self.cursor_color));
                }
            }
        }

        // 6) URL hover underline — a single accent-coloured rect spanning the
        // whole URL token, drawn above the glyph layer so it is always visible.
        if let Some((hr, hcs, hce)) = self.url_hover.get() {
            if hr < self.rows && hcs < self.cols {
                let eff_end = hce.min(self.cols);
                let x = origin.x() + hcs as f32 * cw;
                let w = (eff_end.saturating_sub(hcs)) as f32 * cw;
                let y = origin.y() + hr as f32 * ch + baseline + 2.0;
                let ul_color = crate::warpui::theme::accent();
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(w, 1.0)))
                    .with_background(Fill::Solid(ul_color));
            }
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        let (Some(o), Some(s)) = (self.origin_vec, self.size) else {
            return false;
        };
        // Scroll wheel — bounds-gated: with several terminal panes (or the
        // git-log dock / other scrollables) on screen, an ungated arm here
        // would consume wheel events destined for siblings.
        if let Some(cb) = self.scroll_cb.as_ref() {
            if let Event::ScrollWheel { delta, precise, position, .. } = event.raw_event() {
                let inside = position.x() >= o.x()
                    && position.x() <= o.x() + s.x()
                    && position.y() >= o.y()
                    && position.y() <= o.y() + s.y();
                if inside {
                    cb(delta.y(), *precise);
                    return true;
                }
            }
        }
        let (cw, ch) = (self.cell_w, self.cell_h);
        let in_bounds = |p: &Vector2F| -> bool {
            p.x() >= o.x()
                && p.x() <= o.x() + s.x()
                && p.y() >= o.y()
                && p.y() <= o.y() + s.y()
        };
        let pos_to_cell = |p: &Vector2F| -> (usize, usize, Side) {
            let rel_x = (p.x() - o.x()).max(0.0);
            let rel_y = (p.y() - o.y()).max(0.0);
            let col = ((rel_x / cw).floor() as usize).min(self.cols.saturating_sub(1));
            let row = ((rel_y / ch).floor() as usize).min(self.rows.saturating_sub(1));
            let cell_frac = (rel_x % cw) / cw.max(1.0);
            let side = if cell_frac < 0.5 { Side::Left } else { Side::Right };
            (row, col, side)
        };
        // Return the link span (URL or file path) hit at (row, col), if any.
        let link_hit_at = |row: usize, col: usize| -> Option<&LinkSpan> {
            self.link_spans
                .iter()
                .find(|sp| sp.row == row && col >= sp.col_start && col < sp.col_end)
        };

        match event.raw_event() {
            Event::MouseMoved { position, .. } if in_bounds(position) => {
                let (row, col, _) = pos_to_cell(position);
                if let Some(sp) = link_hit_at(row, col) {
                    let next = Some((sp.row, sp.col_start, sp.col_end));
                    // Repaint immediately when the hovered span changes so the
                    // accent underline appears at idle (no unrelated repaint).
                    if self.url_hover.get() != next {
                        self.url_hover.set(next);
                        ctx.notify();
                    }
                    if let Some(origin_pt) = self.origin {
                        ctx.set_cursor(Cursor::PointingHand, origin_pt.z_index());
                    }
                } else {
                    // Repaint immediately when clearing so a stale underline erases.
                    if self.url_hover.get().is_some() {
                        self.url_hover.set(None);
                        ctx.notify();
                    }
                    ctx.reset_cursor();
                }
                // Don't consume — selection drag and scrollbar need hover too.
            }
            Event::MouseMoved { .. } => {
                // Cursor left the terminal area — clear any lingering hover.
                if self.url_hover.get().is_some() {
                    self.url_hover.set(None);
                    ctx.notify();
                }
            }
            _ => {}
        }

        // SGR mouse-report click forwarding — takes precedence over text
        // selection when a mouse-reporting mode is active, so TUIs that own the
        // mouse (ranger / mc / vim-mouse) receive press/release events instead of
        // us starting a selection. Only left button (SGR button code 0).
        if let Some(cb) = self.mouse_report_cb.clone() {
            match event.raw_event() {
                Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                    let (row, col, _) = pos_to_cell(position);
                    cb(true, col + 1, row + 1);
                    return true;
                }
                Event::LeftMouseUp { position, .. } => {
                    let (row, col, _) = pos_to_cell(position);
                    cb(false, col + 1, row + 1);
                    return true;
                }
                _ => {}
            }
        }

        // Mouse selection + URL click — only when a selection callback is registered.
        if let Some(cb) = self.mouse_sel_cb.clone() {
            match event.raw_event() {
                Event::LeftMouseDown { position, modifiers, .. } if in_bounds(position) => {
                    // Record the link target under the press (if any) and reset drag flag.
                    let (row, col, side) = pos_to_cell(position);
                    let pressed_link = link_hit_at(row, col).map(|sp| sp.target.clone());
                    *self.link_pressed.borrow_mut() = pressed_link;
                    self.url_did_drag.set(false);
                    self.mouse_dragging.set(true);
                    cb(MouseSelPhase::Down, row, col, side, modifiers.shift);
                    return true;
                }
                Event::LeftMouseDragged { position, .. } if self.mouse_dragging.get() => {
                    self.url_did_drag.set(true);
                    let (row, col, side) = pos_to_cell(position);
                    cb(MouseSelPhase::Drag, row, col, side, false);
                    return true;
                }
                Event::LeftMouseUp { position, .. } if self.mouse_dragging.get() => {
                    self.mouse_dragging.set(false);
                    cb(MouseSelPhase::Up, 0, 0, Side::Left, false);
                    // Link click: only when no drag happened and the release is on
                    // the same target that was pressed. This keeps text selection
                    // (drag) completely unaffected. A URL opens in the browser; a
                    // file path is routed to the shell (Crane's editor) at the
                    // detected line/col.
                    if !self.url_did_drag.get() {
                        let pressed = self.link_pressed.borrow().clone();
                        if let Some(target) = pressed {
                            let (row, col, _) = pos_to_cell(position);
                            if link_hit_at(row, col).is_some_and(|sp| sp.target == target) {
                                match target {
                                    LinkTarget::Url(url) => {
                                        let _ = webbrowser::open(&url);
                                    }
                                    LinkTarget::File { path, line, col } => {
                                        ctx.dispatch_typed_action(
                                            crate::warpui::shell::CraneShellAction::OpenFileAtPath {
                                                path,
                                                line,
                                                col,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                    *self.link_pressed.borrow_mut() = None;
                    return true;
                }
                _ => {}
            }
        }

        false
    }
}

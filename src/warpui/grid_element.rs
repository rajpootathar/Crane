//! Custom warpui Element that paints a terminal grid cell-by-cell:
//! per-cell background quad + glyph, plus a block cursor. Full repaint
//! from a cell snapshot every frame — no damage tracking, so it cannot
//! "drop" rows the way the egui grid did.

use std::cell::Cell as StdCell;
use std::rc::Rc;

use crane_term::index::{Column as TermColumn, Line as TermLine, Point as TermPoint, Side};
use crane_term::selection::SelectionRange;
use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint};

/// Phase of a mouse selection gesture.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MouseSelPhase {
    Down,
    Drag,
    Up,
}

#[derive(Clone, Copy)]
pub struct GridCell {
    pub ch: char,
    pub fg: ColorU,
    pub bg: ColorU,
    /// True for the leading column of a double-width (CJK/emoji) glyph;
    /// the trailing WIDE_CHAR_SPACER column is a blank cell.
    pub is_wide: bool,
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
    /// Written in layout() with the cols/rows that fit the available
    /// space; the View reads this next frame to drive PTY/grid resize.
    desired: Rc<StdCell<Option<(usize, usize)>>>,
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
    /// Mouse selection callback: `(phase, viewport_row, col, side)`.
    mouse_sel_cb: Option<Rc<dyn Fn(MouseSelPhase, usize, usize, Side)>>,
    /// Shared drag state (persisted by the owning View across per-frame rebuilds).
    mouse_dragging: Rc<StdCell<bool>>,
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
            desired,
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
        }
    }

    /// Attach a scroll-wheel handler that receives `(delta_y_points, precise)`.
    pub fn on_scroll(mut self, cb: Rc<dyn Fn(f32, bool)>) -> Self {
        self.scroll_cb = Some(cb);
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
        cb: Rc<dyn Fn(MouseSelPhase, usize, usize, Side)>,
    ) -> Self {
        self.mouse_dragging = dragging;
        self.mouse_sel_cb = Some(cb);
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
        self.desired.set(Some((fit_cols, fit_rows)));

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

        // 4) Block cursor (two columns wide over a double-width glyph).
        if let Some((cr, cc)) = self.cursor {
            if cr < self.rows && cc < self.cols {
                let wide = self.cells[cr * self.cols + cc].is_wide;
                let w = if wide { cw * 2.0 } else { cw };
                let x = origin.x() + cc as f32 * cw;
                let y = origin.y() + cr as f32 * ch;
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(w, ch)))
                    .with_background(Fill::Solid(self.cursor_color));
            }
        }

        // 5) Glyphs on top.
        for r in 0..self.rows {
            for c in 0..self.cols {
                let cell = self.cells[r * self.cols + c];
                if cell.ch == ' ' || cell.ch == '\0' {
                    continue;
                }
                // Invert the glyph under the block cursor for legibility.
                let color = match self.cursor {
                    Some((cr, cc)) if cr == r && cc == c => self.default_bg,
                    _ => cell.fg,
                };
                if let Some((gid, render_font)) = fc.glyph_for_char(font, cell.ch, true) {
                    let pos = vec2f(
                        origin.x() + c as f32 * cw,
                        origin.y() + r as f32 * ch + baseline,
                    );
                    ctx.scene.draw_glyph(pos, gid, render_font, self.font_size, color);
                }
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
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        // Scroll wheel — handled first; no bounds check needed (hit rect covers all).
        if let Some(cb) = self.scroll_cb.as_ref() {
            if let Event::ScrollWheel { delta, precise, .. } = event.raw_event() {
                cb(delta.y(), *precise);
                return true;
            }
        }

        // Mouse selection — only when a callback is registered.
        if let Some(cb) = self.mouse_sel_cb.clone() {
            let (Some(o), Some(s)) = (self.origin_vec, self.size) else {
                return false;
            };
            let (cw, ch) = (self.cell_w, self.cell_h);
            let in_bounds = |p: &Vector2F| {
                p.x() >= o.x()
                    && p.x() <= o.x() + s.x()
                    && p.y() >= o.y()
                    && p.y() <= o.y() + s.y()
            };
            // Convert a window-space position to (viewport_row, col, side).
            let pos_to_cell = |p: &Vector2F| -> (usize, usize, Side) {
                let rel_x = (p.x() - o.x()).max(0.0);
                let rel_y = (p.y() - o.y()).max(0.0);
                let col = ((rel_x / cw).floor() as usize)
                    .min(self.cols.saturating_sub(1));
                let row = ((rel_y / ch).floor() as usize)
                    .min(self.rows.saturating_sub(1));
                let cell_frac = (rel_x % cw) / cw.max(1.0);
                let side = if cell_frac < 0.5 { Side::Left } else { Side::Right };
                (row, col, side)
            };
            match event.raw_event() {
                Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                    self.mouse_dragging.set(true);
                    let (row, col, side) = pos_to_cell(position);
                    cb(MouseSelPhase::Down, row, col, side);
                    return true;
                }
                Event::LeftMouseDragged { position, .. } if self.mouse_dragging.get() => {
                    let (row, col, side) = pos_to_cell(position);
                    cb(MouseSelPhase::Drag, row, col, side);
                    return true;
                }
                Event::LeftMouseUp { .. } if self.mouse_dragging.get() => {
                    self.mouse_dragging.set(false);
                    cb(MouseSelPhase::Up, 0, 0, Side::Left);
                    return true;
                }
                _ => {}
            }
        }

        false
    }
}

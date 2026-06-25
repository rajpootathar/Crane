//! Custom warpui Element that paints a terminal grid cell-by-cell:
//! per-cell background quad + glyph, plus a block cursor. Full repaint
//! from a cell snapshot every frame — no damage tracking, so it cannot
//! "drop" rows the way the egui grid did.

use std::cell::Cell as StdCell;
use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint};

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
    cell_w: f32,
    cell_h: f32,
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
            cell_w: font_size * 0.6,
            cell_h: font_size * 1.2,
        }
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

        // 3) Block cursor (two columns wide over a double-width glyph).
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

        // 4) Glyphs on top.
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
        _event: &DispatchedEvent,
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        false
    }
}

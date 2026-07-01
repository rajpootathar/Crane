//! A line-number gutter painted beside the warp editor. It shares the editor's
//! `RenderState`, reading the live vertical scroll offset + line count at paint
//! time so the numbers stay in lock-step with the (self-viewporting) editor.
//! Rows are a uniform grid: with warp's text block spacing at zero vertical
//! margin, each line occupies exactly `row_pitch` px (the paragraph height).

use warp_editor::render::model::RenderState;
use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, ModelHandle, PaintContext,
    SizeConstraint,
};

pub struct GutterElement {
    render_state: ModelHandle<RenderState>,
    font_family: FamilyId,
    font_size: f32,
    /// Vertical pixels per line (must match the editor's paragraph height).
    row_pitch: f32,
    /// 0-based render line the caret sits on (highlighted brighter).
    cursor_line: Option<u32>,
    fg: ColorU,
    fg_active: ColorU,
    bg: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
    digit_w: f32,
}

impl GutterElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        render_state: ModelHandle<RenderState>,
        font_family: FamilyId,
        font_size: f32,
        row_pitch: f32,
        cursor_line: Option<u32>,
        fg: ColorU,
        fg_active: ColorU,
        bg: ColorU,
    ) -> Self {
        Self {
            render_state,
            font_family,
            font_size,
            row_pitch,
            cursor_line,
            fg,
            fg_active,
            bg,
            size: None,
            origin: None,
            digit_w: font_size * 0.6,
        }
    }

    fn line_count(&self, app: &AppContext) -> u32 {
        self.render_state.as_ref(app).max_line().as_u32()
    }
}

impl Element for GutterElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let fc = app.font_cache();
        let font = fc.select_font(self.font_family, Properties::default());
        self.digit_w = fc
            .glyph_for_char(font, '0', false)
            .and_then(|(gid, gfont)| fc.glyph_advance(gfont, self.font_size, gid).ok())
            .map(|a| a.x())
            .unwrap_or(self.font_size * 0.6);
        // Width fits the largest line number + left/right padding.
        let digits = self.line_count(app).max(1).to_string().len().max(2) as f32;
        let width = digits * self.digit_w + 16.0;
        let size = vec2f(width.ceil(), constraint.max.y());
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
        let baseline = ((self.row_pitch - (ascent - descent)) * 0.5) + ascent;

        let rs = self.render_state.as_ref(app);
        let scroll_top = rs.viewport().scroll_top().as_f32();
        let line_count = rs.max_line().as_u32();

        // Background column.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::Solid(self.bg));

        let right_pad = 8.0;
        let first = (scroll_top / self.row_pitch).floor().max(0.0) as u32;
        let visible = (size.y() / self.row_pitch).ceil() as u32 + 1;
        for i in first..(first + visible).min(line_count) {
            let y = origin.y() + i as f32 * self.row_pitch - scroll_top;
            if y + self.row_pitch < origin.y() || y > origin.y() + size.y() {
                continue;
            }
            let label = (i + 1).to_string();
            let color = if Some(i) == self.cursor_line {
                self.fg_active
            } else {
                self.fg
            };
            // Right-align the digits within the column.
            let text_w = label.chars().count() as f32 * self.digit_w;
            let mut x = origin.x() + size.x() - right_pad - text_w;
            for ch in label.chars() {
                if let Some((gid, gfont)) = fc.glyph_for_char(font, ch, false) {
                    ctx.scene
                        .draw_glyph(vec2f(x, y + baseline), gid, gfont, self.font_size, color);
                }
                x += self.digit_w;
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

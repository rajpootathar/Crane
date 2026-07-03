//! A line-number gutter painted beside the warp editor. It shares the editor's
//! `RenderState`, reading the live vertical scroll offset + line count at paint
//! time so the numbers stay in lock-step with the (self-viewporting) editor.
//! Rows are a uniform grid: with warp's text block spacing at zero vertical
//! margin, each line occupies exactly `row_pitch` px (the paragraph height).

use rangemap::RangeMap;
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

/// The kind of change on a gutter line, mapped from git's line-level diff.
/// Keyed the same way as `cursor_line` — by **0-based render line index**.
/// `Deleted` marks a boundary: line(s) were removed *above* the keyed line,
/// so it has no row of its own and is painted as a wedge at the top edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Modified,
    Deleted,
}

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
    /// Per-line git-diff markers, keyed by 0-based render line (same as `cursor_line`).
    diff: Option<RangeMap<u32, DiffKind>>,
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
            diff: None,
            size: None,
            origin: None,
            digit_w: font_size * 0.6,
        }
    }

    /// Attach git-diff line markers. The map is keyed by 0-based render line
    /// (matching `cursor_line`): `Added`/`Modified` paint a bar on that line;
    /// `Deleted` paints a wedge at that line's top boundary.
    pub fn with_diff(mut self, map: RangeMap<u32, DiffKind>) -> Self {
        self.diff = Some(map);
        self
    }

    fn line_count(&self, app: &AppContext) -> u32 {
        self.render_state.as_ref(app).max_line().as_u32()
    }

    /// Draw a small right-pointing triangle wedge whose vertical base sits on
    /// the left gutter edge, centred vertically on `y_boundary`. The scene has
    /// no polygon primitive, so approximate it with a staircase of thin rects.
    fn draw_deleted_wedge(ctx: &mut PaintContext, x: f32, y_boundary: f32, color: ColorU) {
        const H: f32 = 8.0; // wedge height
        const W: f32 = 5.0; // wedge depth (base width)
        const SLICES: usize = 8;
        let slice_h = H / SLICES as f32;
        for s in 0..SLICES {
            // t: 0 at top, 1 at bottom of the wedge.
            let t = (s as f32 + 0.5) / SLICES as f32;
            let dy = (t - 0.5) * H; // -H/2 .. H/2 relative to the boundary
            // Widest at the vertical centre, tapering to a point top and bottom.
            let w = (W * (1.0 - (dy.abs() / (H * 0.5)))).max(0.5);
            let sy = y_boundary + dy - slice_h * 0.5;
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    vec2f(x, sy),
                    vec2f(w, slice_h + 0.5),
                ))
                .with_background(Fill::Solid(color));
        }
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
            // Git-diff marker at the left edge (drawn under the digits).
            if let Some(kind) = self.diff.as_ref().and_then(|m| m.get(&i)).copied() {
                match kind {
                    DiffKind::Added | DiffKind::Modified => {
                        let color = if kind == DiffKind::Added {
                            crate::warpui::theme::success()
                        } else {
                            crate::warpui::theme::accent()
                        };
                        ctx.scene
                            .draw_rect_without_hit_recording(RectF::new(
                                vec2f(origin.x(), y),
                                vec2f(3.0, self.row_pitch),
                            ))
                            .with_background(Fill::Solid(color));
                    }
                    DiffKind::Deleted => {
                        // Deleted lines have no row of their own — paint a small
                        // right-pointing wedge centred on this line's top boundary.
                        Self::draw_deleted_wedge(ctx, origin.x(), y, crate::warpui::theme::error());
                    }
                }
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

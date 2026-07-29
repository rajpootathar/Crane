//! A thin vertical scrollbar painted at the right edge of the warp editor. It
//! shares the editor's `RenderState`, reading content height + viewport height +
//! scroll offset at paint time and drawing a proportional thumb. Visual only for
//! now (wheel scroll drives it); dragging the thumb comes next.

use std::cell::Cell;
use std::rc::Rc;

use warp_editor::render::model::RenderState;
use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, ModelHandle, PaintContext,
    SizeConstraint,
};

const WIDTH: f32 = 12.0;
const MIN_THUMB: f32 = 28.0;

pub struct ScrollbarElement {
    render_state: ModelHandle<RenderState>,
    thumb: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
    origin_vec: Option<Vector2F>,
    dragging: Rc<Cell<bool>>,
    /// Called with the track fraction (0.0 top … 1.0 bottom) on drag; dispatches
    /// a scroll action to the owning editor view via the EventContext.
    on_drag: Option<Rc<dyn Fn(&mut EventContext, f32)>>,
}

impl ScrollbarElement {
    pub fn new(render_state: ModelHandle<RenderState>, thumb: ColorU) -> Self {
        Self {
            render_state,
            thumb,
            size: None,
            origin: None,
            origin_vec: None,
            dragging: Rc::new(Cell::new(false)),
            on_drag: None,
        }
    }

    /// Make the thumb draggable. `dragging` is shared/persisted by the view.
    pub fn draggable(
        mut self,
        dragging: Rc<Cell<bool>>,
        on_drag: Rc<dyn Fn(&mut EventContext, f32)>,
    ) -> Self {
        self.dragging = dragging;
        self.on_drag = Some(on_drag);
        self
    }
}

impl Element for ScrollbarElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let size = vec2f(WIDTH, constraint.max.y());
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        let track_h = size.y();

        let rs = self.render_state.as_ref(app);
        let content_h = rs.height().as_f32().max(1.0);
        let scroll_top = rs.viewport().scroll_top().as_f32();
        let view_h = track_h; // scrollbar spans the editor viewport

        // Records the hit rect so future drag handling can grab it.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::None);

        // Always show a thumb: full-height when everything fits, partial when
        // the content is taller than the viewport.
        let (thumb_h, frac) = if content_h <= view_h + 1.0 {
            (track_h, 0.0)
        } else {
            let h = (view_h / content_h * track_h).max(MIN_THUMB).min(track_h);
            let max_scroll = (content_h - view_h).max(1.0);
            (h, (scroll_top / max_scroll).clamp(0.0, 1.0))
        };
        let thumb_y = origin.y() + frac * (track_h - thumb_h);

        let pad = 2.0;
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(origin.x() + pad, thumb_y),
                vec2f(WIDTH - pad * 2.0, thumb_h),
            ))
            .with_background(Fill::Solid(self.thumb));
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
        let Some(on_drag) = self.on_drag.clone() else {
            return false;
        };
        let (Some(o), Some(s)) = (self.origin_vec, self.size) else {
            return false;
        };
        let frac_at = |y: f32| ((y - o.y()) / s.y().max(1.0)).clamp(0.0, 1.0);
        let in_bounds = |p: &Vector2F| {
            p.x() >= o.x() && p.x() <= o.x() + s.x() && p.y() >= o.y() && p.y() <= o.y() + s.y()
        };
        match event.raw_event() {
            Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                self.dragging.set(true);
                on_drag(ctx, frac_at(position.y()));
                true
            }
            Event::LeftMouseDragged { position, .. } if self.dragging.get() => {
                on_drag(ctx, frac_at(position.y()));
                true
            }
            Event::LeftMouseUp { .. } => {
                let was = self.dragging.get();
                self.dragging.set(false);
                was
            }
            _ => false,
        }
    }
}

/// A scrollbar sized from raw line counts (for the terminal, which measures in
/// rows, not pixels). `total` = scrollback + viewport rows, `viewport` = visible
/// rows, `top` = index of the first visible row (0 = scrolled fully up).
pub struct LineScrollbar {
    total: f32,
    viewport: f32,
    top: f32,
    thumb: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
    /// Paint-time origin in window coords, for hit-testing a drag.
    origin_vec: Option<Vector2F>,
    /// Drag state, persisted across the per-frame element rebuilds.
    dragging: Rc<Cell<bool>>,
    /// Called with the drag target fraction (0.0 = top/oldest, 1.0 = bottom/live)
    /// on mouse-down or drag. `None` = display-only (e.g. editor for now).
    on_scroll: Option<Rc<dyn Fn(f32)>>,
    /// Ctx-aware variant of `on_scroll` — receives the EventContext so the
    /// owning view can dispatch a typed action (and thereby notify a repaint)
    /// from the drag. Used by the Diff Pane; the terminal keeps the plain
    /// callback + its own wake channel.
    on_scroll_ctx: Option<Rc<dyn Fn(&mut EventContext, f32)>>,
}

impl LineScrollbar {
    pub fn new(total: usize, viewport: usize, top: usize, thumb: ColorU) -> Self {
        Self {
            total: total.max(1) as f32,
            viewport: viewport as f32,
            top: top as f32,
            thumb,
            size: None,
            origin: None,
            origin_vec: None,
            dragging: Rc::new(Cell::new(false)),
            on_scroll: None,
            on_scroll_ctx: None,
        }
    }

    /// Make the scrollbar draggable. `dragging` must be shared/persisted by the
    /// owning view (the element is rebuilt every frame). `on_scroll` receives the
    /// track fraction (0.0 top … 1.0 bottom).
    pub fn draggable(mut self, dragging: Rc<Cell<bool>>, on_scroll: Rc<dyn Fn(f32)>) -> Self {
        self.dragging = dragging;
        self.on_scroll = Some(on_scroll);
        self
    }

    /// Like [`Self::draggable`], but the callback also receives the
    /// [`EventContext`] so the owning view can dispatch a typed action from
    /// the drag (which handles the notify/repaint the plain callback can't).
    pub fn draggable_with_ctx(
        mut self,
        dragging: Rc<Cell<bool>>,
        on_scroll: Rc<dyn Fn(&mut EventContext, f32)>,
    ) -> Self {
        self.dragging = dragging;
        self.on_scroll_ctx = Some(on_scroll);
        self
    }
}

impl Element for LineScrollbar {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let size = vec2f(WIDTH, constraint.max.y());
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        let track_h = size.y();
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::None);
        // Always draw a thumb so the scrollbar is visible: full-height when there
        // is nothing to scroll (whole content visible), partial when scrollable.
        let (thumb_h, frac) = if self.total <= self.viewport + 0.5 {
            (track_h, 0.0)
        } else {
            let h = (self.viewport / self.total * track_h)
                .max(MIN_THUMB)
                .min(track_h);
            let max_top = (self.total - self.viewport).max(1.0);
            (h, (self.top / max_top).clamp(0.0, 1.0))
        };
        let thumb_y = origin.y() + frac * (track_h - thumb_h);
        let pad = 2.0;
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(origin.x() + pad, thumb_y),
                vec2f(WIDTH - pad * 2.0, thumb_h),
            ))
            .with_background(Fill::Solid(self.thumb));
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
        if self.on_scroll.is_none() && self.on_scroll_ctx.is_none() {
            return false;
        }
        let (Some(o), Some(s)) = (self.origin_vec, self.size) else {
            return false;
        };
        let frac_at = |y: f32| ((y - o.y()) / s.y().max(1.0)).clamp(0.0, 1.0);
        let in_bounds = |p: &Vector2F| {
            p.x() >= o.x() && p.x() <= o.x() + s.x() && p.y() >= o.y() && p.y() <= o.y() + s.y()
        };
        let fire = |ctx: &mut EventContext, frac: f32| {
            if let Some(f) = self.on_scroll.as_ref() {
                f(frac);
            }
            if let Some(f) = self.on_scroll_ctx.as_ref() {
                f(ctx, frac);
            }
        };
        match event.raw_event() {
            Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                self.dragging.set(true);
                fire(ctx, frac_at(position.y()));
                true
            }
            Event::LeftMouseDragged { position, .. } if self.dragging.get() => {
                fire(ctx, frac_at(position.y()));
                true
            }
            Event::LeftMouseUp { .. } => {
                let was = self.dragging.get();
                self.dragging.set(false);
                was
            }
            _ => false,
        }
    }
}

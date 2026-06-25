//! A horizontal split container with a draggable splitter — the warpui port
//! of Crane's `pane_view` split geometry (manual rect math + splitter drag).
//! Lays out two child elements side by side at `ratio`, paints a splitter
//! strip between them, and adjusts `ratio` when the splitter is dragged.

use std::cell::Cell;
use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

const SPLIT_W: f32 = 5.0;

pub struct SplitRow {
    first: Box<dyn Element>,
    second: Box<dyn Element>,
    ratio: Rc<Cell<f32>>,
    dragging: Cell<bool>,
    splitter_color: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
    o: Vector2F,
    first_w: f32,
}

impl SplitRow {
    pub fn new(
        first: Box<dyn Element>,
        second: Box<dyn Element>,
        ratio: Rc<Cell<f32>>,
        splitter_color: ColorU,
    ) -> Self {
        Self {
            first,
            second,
            ratio,
            dragging: Cell::new(false),
            splitter_color,
            size: None,
            origin: None,
            o: vec2f(0.0, 0.0),
            first_w: 0.0,
        }
    }

    fn strict(size: Vector2F) -> SizeConstraint {
        SizeConstraint { min: size, max: size }
    }
}

impl Element for SplitRow {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = constraint.max;
        let avail = (size.x() - SPLIT_W).max(0.0);
        let r = self.ratio.get().clamp(0.1, 0.9);
        let fw = (avail * r).round();
        let sw = avail - fw;
        self.first
            .layout(Self::strict(vec2f(fw, size.y())), ctx, app);
        self.second
            .layout(Self::strict(vec2f(sw, size.y())), ctx, app);
        self.first_w = fw;
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.first.after_layout(ctx, app);
        self.second.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.o = origin;
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        let fw = self.first_w;

        self.first.paint(origin, ctx, app);
        let sx = origin.x() + fw;
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(sx, origin.y()),
                vec2f(SPLIT_W, size.y()),
            ))
            .with_background(Fill::Solid(self.splitter_color));
        self.second.paint(vec2f(sx + SPLIT_W, origin.y()), ctx, app);
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
        app: &AppContext,
    ) -> bool {
        let w = self.size.map(|s| s.x()).unwrap_or(1.0);
        let split_x0 = self.o.x() + self.first_w;
        match event.raw_event() {
            Event::LeftMouseDown { position, .. } => {
                if position.x() >= split_x0 - 2.0 && position.x() <= split_x0 + SPLIT_W + 2.0 {
                    self.dragging.set(true);
                    return true;
                }
            }
            Event::LeftMouseDragged { position, .. } => {
                if self.dragging.get() {
                    let r = ((position.x() - self.o.x()) / (w - SPLIT_W)).clamp(0.1, 0.9);
                    self.ratio.set(r);
                    return true;
                }
            }
            Event::LeftMouseUp { .. } => {
                if self.dragging.get() {
                    self.dragging.set(false);
                    return true;
                }
            }
            _ => {}
        }
        if self.first.dispatch_event(event, ctx, app) {
            return true;
        }
        self.second.dispatch_event(event, ctx, app)
    }
}

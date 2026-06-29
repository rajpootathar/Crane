//! A split container with a draggable splitter — the warpui port of Crane's
//! `pane_view` split geometry. Lays out two child elements at `ratio` along
//! `dir` (Horizontal = side by side, Vertical = stacked), paints a splitter
//! strip between them, and adjusts `ratio` when the splitter is dragged.

use std::cell::Cell;
use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

use crate::layout::Dir;

const SPLIT_W: f32 = 5.0;

pub struct SplitBox {
    dir: Dir,
    first: Box<dyn Element>,
    second: Box<dyn Element>,
    ratio: Rc<Cell<f32>>,
    /// Shared with the Node so drag state survives per-frame rebuilds.
    dragging: Rc<Cell<bool>>,
    splitter_color: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
    o: Vector2F,
    /// Length of the first child along the split axis.
    first_len: f32,
}

impl SplitBox {
    pub fn new(
        dir: Dir,
        first: Box<dyn Element>,
        second: Box<dyn Element>,
        ratio: Rc<Cell<f32>>,
        dragging: Rc<Cell<bool>>,
        splitter_color: ColorU,
    ) -> Self {
        Self {
            dir,
            first,
            second,
            ratio,
            dragging,
            splitter_color,
            size: None,
            origin: None,
            o: vec2f(0.0, 0.0),
            first_len: 0.0,
        }
    }

    fn strict(size: Vector2F) -> SizeConstraint {
        SizeConstraint { min: size, max: size }
    }

    /// The size along the split axis.
    fn axis(&self, v: Vector2F) -> f32 {
        match self.dir {
            Dir::Horizontal => v.x(),
            Dir::Vertical => v.y(),
        }
    }
}

impl Element for SplitBox {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = constraint.max;
        let avail = (self.axis(size) - SPLIT_W).max(0.0);
        let r = self.ratio.get().clamp(0.1, 0.9);
        let fl = (avail * r).round();
        let sl = avail - fl;
        let (first_size, second_size) = match self.dir {
            Dir::Horizontal => (vec2f(fl, size.y()), vec2f(sl, size.y())),
            Dir::Vertical => (vec2f(size.x(), fl), vec2f(size.x(), sl)),
        };
        self.first.layout(Self::strict(first_size), ctx, app);
        self.second.layout(Self::strict(second_size), ctx, app);
        self.first_len = fl;
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
        let fl = self.first_len;

        self.first.paint(origin, ctx, app);
        let (split_origin, split_size, second_origin) = match self.dir {
            Dir::Horizontal => (
                vec2f(origin.x() + fl, origin.y()),
                vec2f(SPLIT_W, size.y()),
                vec2f(origin.x() + fl + SPLIT_W, origin.y()),
            ),
            Dir::Vertical => (
                vec2f(origin.x(), origin.y() + fl),
                vec2f(size.x(), SPLIT_W),
                vec2f(origin.x(), origin.y() + fl + SPLIT_W),
            ),
        };
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(split_origin, split_size))
            .with_background(Fill::Solid(self.splitter_color));
        self.second.paint(second_origin, ctx, app);
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
        let total = self.axis(self.size.unwrap_or_else(|| vec2f(1.0, 1.0)));
        let origin_axis = self.axis(self.o);
        let split_0 = origin_axis + self.first_len;
        let pos_axis = |p: &Vector2F| match self.dir {
            Dir::Horizontal => p.x(),
            Dir::Vertical => p.y(),
        };
        match event.raw_event() {
            Event::MouseMoved { position, .. } => {
                // Resize cursor ONLY over the thin splitter band — otherwise
                // reset, else the resize cursor sticks across the whole pane
                // (warpui leaves the cursor unchanged when nothing sets it).
                let a = pos_axis(position);
                if a >= split_0 - 2.0 && a <= split_0 + SPLIT_W + 2.0 {
                    let cursor = match self.dir {
                        Dir::Horizontal => Cursor::ResizeLeftRight,
                        Dir::Vertical => Cursor::ResizeUpDown,
                    };
                    if let Some(o) = self.origin {
                        ctx.set_cursor(cursor, o.z_index());
                    }
                } else {
                    ctx.reset_cursor();
                }
            }
            Event::LeftMouseDown { position, .. } => {
                let a = pos_axis(position);
                if a >= split_0 - 2.0 && a <= split_0 + SPLIT_W + 2.0 {
                    self.dragging.set(true);
                    return true;
                }
            }
            Event::LeftMouseDragged { position, .. } => {
                if self.dragging.get() {
                    let r = ((pos_axis(position) - origin_axis) / (total - SPLIT_W)).clamp(0.1, 0.9);
                    self.ratio.set(r);
                    // Request a repaint so the resize is LIVE — mutating the
                    // ratio Cell alone doesn't tell warpui to re-render.
                    ctx.notify();
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

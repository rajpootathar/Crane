//! `RectProbe` records its painted window-space rect into a shared cell, so the
//! view can do cursor-position hit-testing for drag-drop dock zones (the warpui
//! port of old Crane's `dock_zone`, which needs the target pane's rect).

use std::cell::Cell;
use std::rc::Rc;

use warpui::elements::{Element, Point};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

use crate::warpui::layout::PaneId;

/// Which edge of a pane a drop landed on. `Center` = swap. Ported 1:1 from
/// old Crane `src/state/layout.rs::DockEdge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockEdge {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

/// Old Crane `pane_view::dock_zone` ported verbatim: a 30%×30% center square
/// docks to Center (swap); otherwise the dominant axis from center wins.
pub fn dock_zone(rect: RectF, pos: Vector2F) -> DockEdge {
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let rel_x = ((pos.x() - rect.origin().x()) / w).clamp(0.0, 1.0);
    let rel_y = ((pos.y() - rect.origin().y()) / h).clamp(0.0, 1.0);
    let (cmin, cmax) = (0.35, 0.65);
    if rel_x >= cmin && rel_x <= cmax && rel_y >= cmin && rel_y <= cmax {
        return DockEdge::Center;
    }
    let dx = rel_x - 0.5;
    let dy = rel_y - 0.5;
    if dx.abs() >= dy.abs() {
        if dx < 0.0 {
            DockEdge::Left
        } else {
            DockEdge::Right
        }
    } else if dy < 0.0 {
        DockEdge::Top
    } else {
        DockEdge::Bottom
    }
}

/// Shared map of pane id → last painted window rect.
pub type PaneRect = Rc<Cell<RectF>>;

pub struct RectProbe {
    child: Box<dyn Element>,
    cell: PaneRect,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl RectProbe {
    pub fn new(child: Box<dyn Element>, cell: PaneRect) -> Self {
        Self {
            child,
            cell,
            size: None,
            origin: None,
        }
    }
}

impl Element for RectProbe {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let s = self.child.layout(constraint, ctx, app);
        self.size = Some(s);
        s
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        self.cell.set(RectF::new(origin, size));
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &warpui::event::DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}

/// `ZoneProbe` — like [`RectProbe`], but appends `(rect, tag)` to a shared
/// list at paint time instead of writing one cell. The sidebar drag-drop
/// reorder collects every row's painted rect + drop scope this way (the list
/// is cleared at the start of each left-panel render, so it always holds
/// exactly the rows painted this frame, in visual order).
pub type ZoneList<T> = Rc<std::cell::RefCell<Vec<(RectF, T)>>>;

pub struct ZoneProbe<T: Clone + 'static> {
    child: Box<dyn Element>,
    zones: ZoneList<T>,
    tag: T,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl<T: Clone + 'static> ZoneProbe<T> {
    pub fn new(child: Box<dyn Element>, zones: ZoneList<T>, tag: T) -> Self {
        Self {
            child,
            zones,
            tag,
            size: None,
            origin: None,
        }
    }
}

impl<T: Clone + 'static> Element for ZoneProbe<T> {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let s = self.child.layout(constraint, ctx, app);
        self.size = Some(s);
        s
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));
        self.zones
            .borrow_mut()
            .push((RectF::new(origin, size), self.tag.clone()));
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &warpui::event::DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}

/// `FileDropSink` — routes OS drag-and-drop file events (Finder → Crane) to a
/// callback when the drop lands inside this element's painted rect. warpui
/// surfaces `Event::DragAndDropFiles { paths, location }` at window scope but
/// no stock element consumes it; this sink gives the Files tree its old
/// "drop OS files into the tree" behavior.
pub struct FileDropSink {
    child: Box<dyn Element>,
    on_drop: Rc<dyn Fn(&[String], Vector2F, &mut EventContext)>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl FileDropSink {
    pub fn new(
        child: Box<dyn Element>,
        on_drop: Rc<dyn Fn(&[String], Vector2F, &mut EventContext)>,
    ) -> Self {
        Self {
            child,
            on_drop,
            size: None,
            origin: None,
        }
    }
}

impl Element for FileDropSink {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let s = self.child.layout(constraint, ctx, app);
        self.size = Some(s);
        s
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &warpui::event::DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        if let warpui::event::Event::DragAndDropFiles { paths, location } = event.raw_event() {
            if let (Some(o), Some(s)) = (self.origin.map(|p| p.xy()), self.size) {
                let inside = location.x() >= o.x()
                    && location.x() <= o.x() + s.x()
                    && location.y() >= o.y()
                    && location.y() <= o.y() + s.y();
                if inside {
                    (self.on_drop)(paths, *location, ctx);
                    return true;
                }
            }
        }
        self.child.dispatch_event(event, ctx, app)
    }
}

/// Find the (non-source) pane under `cursor` and the dock edge there.
pub fn pane_under(
    rects: &[(PaneId, RectF)],
    source: PaneId,
    cursor: Vector2F,
) -> Option<(PaneId, DockEdge)> {
    for (pid, r) in rects {
        if *pid == source {
            continue;
        }
        if r.contains_point(cursor) {
            return Some((*pid, dock_zone(*r, cursor)));
        }
    }
    None
}

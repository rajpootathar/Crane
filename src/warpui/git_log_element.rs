//! Custom warpui Element that paints the Git Log commit list: the railroad
//! LANE GRAPH on the left (colored lane lines + a commit-node dot per row),
//! then REF PILLS (rounded chips for HEAD / branch / tag decorations), the
//! short hash, the subject, and the author + relative time. Viewport-aware —
//! only the rows that fit the pane are laid out and painted, so a
//! 10 000-commit frame costs the same as a 20-commit one.
//!
//! Scrolling is internal (a shared fractional row offset the shell persists
//! across per-frame rebuilds); a click on a row dispatches
//! [`CraneShellAction::GitLogSelect`] up to the shell, which loads that
//! commit's `git show` detail off-thread. A RIGHT-click on a row runs the
//! shell-supplied [`GitLogMenuCallback`] with the row's SHA + click position
//! so the shell can open its commit context menu (Checkout / Branch from /
//! Cherry-pick / Revert / Copy hash — old Crane `view/log.rs` row menu).

use std::cell::Cell as StdCell;
use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Border, CornerRadius, Element, Fill, Point, Radius};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties, Style, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

use crate::warpui::git_log::{self, GraphFrame, RefKind};
use crate::warpui::theme;

/// 8-slot lane palette (matches old Crane) — legible on light + dark.
const LANE_PALETTE: [ColorU; 8] = [
    ColorU { r: 102, g: 187, b: 106, a: 255 }, // green
    ColorU { r: 66, g: 165, b: 245, a: 255 },  // blue
    ColorU { r: 255, g: 152, b: 0, a: 255 },   // orange
    ColorU { r: 171, g: 71, b: 188, a: 255 },  // purple
    ColorU { r: 236, g: 64, b: 122, a: 255 },  // pink
    ColorU { r: 38, g: 166, b: 154, a: 255 },  // teal
    ColorU { r: 239, g: 83, b: 80, a: 255 },   // red
    ColorU { r: 255, g: 202, b: 40, a: 255 },  // yellow
];

fn lane_color(slot: u8) -> ColorU {
    LANE_PALETTE[(slot as usize) % LANE_PALETTE.len()]
}

const BLACK: ColorU = ColorU { r: 20, g: 22, b: 26, a: 255 };
const WHITE: ColorU = ColorU { r: 245, g: 247, b: 250, a: 255 };

/// `(background, foreground)` for a ref-pill of the given kind.
fn pill_colors(kind: RefKind) -> (ColorU, ColorU) {
    match kind {
        RefKind::Head => (ColorU { r: 102, g: 187, b: 106, a: 255 }, BLACK),
        RefKind::LocalBranch => (ColorU { r: 171, g: 71, b: 188, a: 255 }, WHITE),
        RefKind::RemoteBranch => (ColorU { r: 66, g: 165, b: 245, a: 255 }, WHITE),
        RefKind::Tag => (ColorU { r: 255, g: 202, b: 40, a: 255 }, BLACK),
        RefKind::Unknown => (ColorU { r: 110, g: 118, b: 132, a: 255 }, WHITE),
    }
}

// Lane-graph geometry.
const COL_W: f32 = 15.0;
const DOT_R: f32 = 4.0;
const GRAPH_PAD_LEFT: f32 = 10.0;
const LANE_STROKE: f32 = 1.5;

/// Callback the shell supplies for a right-click on a commit row. Runs inside
/// the element's event dispatch with `(sha, x, y, ctx)` — the clicked commit's
/// full SHA and the click position in window coords. The shell's closure
/// routes it into its own typed action (a context-menu overlay anchored at
/// `(x, y)`, like the Changes / Files row menus), keeping this element free
/// of any menu-variant knowledge — same shape as `WelcomeCallback` /
/// `grid_element`'s callbacks.
pub type GitLogMenuCallback = Rc<dyn Fn(&str, f32, f32, &mut EventContext)>;

pub struct GitLogListElement {
    frame: Rc<GraphFrame>,
    font_family: FamilyId,
    font_size: f32,
    /// Fractional scroll offset in ROWS (shared, persisted by the shell).
    scroll: Rc<StdCell<f32>>,
    /// Selected commit SHA (highlighted).
    selected: Option<String>,
    /// Hovered row index (shared so the highlight survives per-frame rebuilds).
    hover: Rc<StdCell<Option<usize>>>,
    /// Right-click handler (commit context menu); `None` disables it.
    on_context_menu: Option<GitLogMenuCallback>,

    // Layout/paint scratch.
    size: Option<Vector2F>,
    origin: Option<Point>,
    origin_vec: Option<Vector2F>,
    row_h: f32,
    cell_w: f32,
}

impl GitLogListElement {
    pub fn new(
        frame: Rc<GraphFrame>,
        font_family: FamilyId,
        font_size: f32,
        scroll: Rc<StdCell<f32>>,
        selected: Option<String>,
        hover: Rc<StdCell<Option<usize>>>,
    ) -> Self {
        Self {
            frame,
            font_family,
            font_size,
            scroll,
            selected,
            hover,
            on_context_menu: None,
            size: None,
            origin: None,
            origin_vec: None,
            row_h: font_size * 1.7,
            cell_w: font_size * 0.6,
        }
    }

    /// Attach the right-click commit-menu callback (see [`GitLogMenuCallback`]).
    pub fn with_context_menu(mut self, cb: GitLogMenuCallback) -> Self {
        self.on_context_menu = Some(cb);
        self
    }

    fn total_rows(&self) -> usize {
        self.frame.commits.len()
    }

    fn graph_width(&self) -> f32 {
        let max_lane = self.frame.lanes.max_lane.max(1) as f32;
        GRAPH_PAD_LEFT + (max_lane + 1.0) * COL_W
    }

    /// Clamp the shared scroll offset to `[0, total - visible]`.
    fn clamp_scroll(&self) -> f32 {
        let visible = (self.size.map(|s| s.y()).unwrap_or(0.0) / self.row_h).floor() as usize;
        let max = self.total_rows().saturating_sub(visible.max(1)) as f32;
        let clamped = self.scroll.get().clamp(0.0, max.max(0.0));
        self.scroll.set(clamped);
        clamped
    }

    /// X-center of `lane` in window coords.
    fn lane_x(&self, origin_x: f32, lane: u8) -> f32 {
        origin_x + GRAPH_PAD_LEFT + lane as f32 * COL_W + COL_W * 0.5
    }
}

/// Rasterize a straight line as a run of small squares — warpui's scene only
/// draws rects, so an off-lane parent edge is approximated by stepping a
/// `width`-sized square along the segment (fine at 1.5 px on a ~22 px row).
fn draw_line(ctx: &mut PaintContext, a: Vector2F, b: Vector2F, width: f32, color: ColorU) {
    let dx = b.x() - a.x();
    let dy = b.y() - a.y();
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let steps = len.ceil() as usize;
    let half = width * 0.5;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = a.x() + dx * t - half;
        let y = a.y() + dy * t - half;
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(vec2f(x, y), vec2f(width, width)))
            .with_background(Fill::Solid(color));
    }
}

impl Element for GitLogListElement {
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
        self.row_h = fc.line_height(self.font_size, 1.7);
        let size = constraint.apply(constraint.max);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));

        // Background fill + the single hit rect covering the whole list.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::Solid(theme::bg()));

        let total = self.total_rows();
        if total == 0 {
            return;
        }

        let fc = app.font_cache();
        let font = fc.select_font(self.font_family, Properties::default());
        let bold = fc.select_font(
            self.font_family,
            Properties { weight: Weight::Bold, style: Style::Normal },
        );
        let ascent = fc.ascent(font, self.font_size);
        let descent = fc.descent(font, self.font_size);
        let text_h = ascent - descent;
        let baseline = ((self.row_h - text_h) * 0.5) + ascent;

        let scroll = self.clamp_scroll();
        let first = scroll.floor() as usize;
        let y_off = -(scroll.fract()) * self.row_h;
        let visible = (size.y() / self.row_h).ceil() as usize + 1;

        let graph_w = self.graph_width();
        let text_muted = theme::text_muted();
        let text_col = theme::text();
        let accent = theme::accent();
        let sel_bg = theme::selection();
        let hover_bg = theme::row_hover();

        let hover_row = self.hover.get();

        // A small text drawer: monospace advance, truncates at `max_w`.
        let draw_text =
            |ctx: &mut PaintContext, x: f32, y_baseline: f32, s: &str, color: ColorU, max_w: f32, fid| {
                let mut cx = x;
                for ch in s.chars() {
                    if cx + self.cell_w > x + max_w {
                        break;
                    }
                    if ch != ' ' {
                        if let Some((gid, rf)) = fc.glyph_for_char(fid, ch, true) {
                            ctx.scene
                                .draw_glyph(vec2f(cx, y_baseline), gid, rf, self.font_size, color);
                        }
                    }
                    cx += self.cell_w;
                }
                cx
            };

        for vi in 0..visible {
            let i = first + vi;
            if i >= total {
                break;
            }
            let row_top = origin.y() + y_off + vi as f32 * self.row_h;
            if row_top >= origin.y() + size.y() {
                break;
            }
            let commit = &self.frame.commits[i];
            let lane = self.frame.lanes.rows.get(i);
            let next_lane = self.frame.lanes.rows.get(i + 1);

            // Row background: selection wins over hover.
            let is_sel = self.selected.as_deref() == Some(commit.sha.as_str());
            if is_sel {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(origin.x(), row_top),
                        vec2f(size.x(), self.row_h),
                    ))
                    .with_background(Fill::Solid(sel_bg));
            } else if hover_row == Some(i) {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(origin.x(), row_top),
                        vec2f(size.x(), self.row_h),
                    ))
                    .with_background(Fill::Solid(hover_bg));
            }

            // ── Lane graph ────────────────────────────────────────────────
            if let Some(lr) = lane {
                let dot_y = row_top + self.row_h * 0.5;
                let dot_x = self.lane_x(origin.x(), lr.own_lane);

                // Passthrough vertical lanes (full-row segments).
                for &(pl, pc) in &lr.passthrough_lanes {
                    let px = self.lane_x(origin.x(), pl);
                    let col = lane_color(pc);
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(
                            vec2f(px - LANE_STROKE * 0.5, row_top),
                            vec2f(LANE_STROKE, self.row_h),
                        ))
                        .with_background(Fill::Solid(col));
                }

                // Parent connections down to the next row.
                if next_lane.is_some() {
                    let next_dot_y = dot_y + self.row_h;
                    for &pl in &lr.parent_lanes {
                        let next = next_lane.unwrap();
                        let edge_col = if next.own_lane == pl {
                            lane_color(next.color)
                        } else if let Some(&(_, c)) =
                            next.passthrough_lanes.iter().find(|(l, _)| *l == pl)
                        {
                            lane_color(c)
                        } else {
                            lane_color(lr.color)
                        };
                        let px = self.lane_x(origin.x(), pl);
                        if pl == lr.own_lane {
                            // Straight continuation.
                            ctx.scene
                                .draw_rect_without_hit_recording(RectF::new(
                                    vec2f(dot_x - LANE_STROKE * 0.5, dot_y),
                                    vec2f(LANE_STROKE, self.row_h),
                                ))
                                .with_background(Fill::Solid(edge_col));
                        } else {
                            // Off-lane branch/merge — diagonal.
                            draw_line(
                                ctx,
                                vec2f(dot_x, dot_y),
                                vec2f(px, next_dot_y),
                                LANE_STROKE,
                                edge_col,
                            );
                        }
                    }
                }

                // Terminating lane caps (small hollow ring at row top).
                for &term in &lr.terminating_lanes {
                    let tx = self.lane_x(origin.x(), term);
                    let r = DOT_R - 1.0;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(
                            vec2f(tx - r, row_top + 2.0 - r),
                            vec2f(r * 2.0, r * 2.0),
                        ))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(r)))
                        .with_border(Border {
                            width: 1.0,
                            color: Fill::Solid(text_muted),
                            top: true,
                            left: true,
                            bottom: true,
                            right: true,
                            dash: None,
                        });
                }

                // Commit dot (drawn last so it sits over incoming edges).
                let dot_col = lane_color(lr.color);
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(dot_x - DOT_R, dot_y - DOT_R),
                        vec2f(DOT_R * 2.0, DOT_R * 2.0),
                    ))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(DOT_R)))
                    .with_background(Fill::Solid(dot_col));
            }

            // ── Ref pills ─────────────────────────────────────────────────
            let mut cx = origin.x() + graph_w + 4.0;
            let pill_h = self.row_h - 8.0;
            let pill_top = row_top + 4.0;
            for pill in git_log::parse_ref_pills(&commit.refs_decoration, &self.frame.refs) {
                let (bg, fg) = pill_colors(pill.kind);
                let label_w = pill.label.chars().count() as f32 * self.cell_w;
                let pw = label_w + 10.0;
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(cx, pill_top),
                        vec2f(pw, pill_h),
                    ))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .with_background(Fill::Solid(bg));
                let pill_baseline = pill_top + ((pill_h - text_h) * 0.5) + ascent;
                draw_text(ctx, cx + 5.0, pill_baseline, &pill.label, fg, label_w + 2.0, font);
                cx += pw + 4.0;
            }

            // ── short hash · subject ──────────────────────────────────────
            let base_y = row_top + baseline;
            let short: String = commit.sha.chars().take(8).collect();
            let hash_w = 9.0 * self.cell_w;
            cx = draw_text(ctx, cx, base_y, &short, text_muted, hash_w, font);
            cx += self.cell_w;

            // Meta (author + relative age) reserved on the right.
            let meta = format!("{}  {}", commit.author, commit.relative);
            let meta_w = meta.chars().count() as f32 * self.cell_w;
            let meta_x = origin.x() + size.x() - meta_w - 10.0;

            // Subject fills the gap between hash and meta.
            let subj_max = (meta_x - cx - 8.0).max(0.0);
            let subj_col = if is_sel { accent } else { text_col };
            draw_text(ctx, cx, base_y, &commit.subject, subj_col, subj_max, if is_sel { bold } else { font });

            if meta_x > cx {
                draw_text(ctx, meta_x, base_y, &meta, text_muted, meta_w + self.cell_w, font);
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
        let in_bounds = |p: &Vector2F| -> bool {
            p.x() >= o.x() && p.x() <= o.x() + s.x() && p.y() >= o.y() && p.y() <= o.y() + s.y()
        };
        // Which commit index sits under `p` (given the current scroll).
        let row_at = |p: &Vector2F| -> Option<usize> {
            let rel_y = p.y() - o.y();
            if rel_y < 0.0 {
                return None;
            }
            let idx = (self.scroll.get() + rel_y / self.row_h).floor() as usize;
            if idx < self.total_rows() {
                Some(idx)
            } else {
                None
            }
        };

        match event.raw_event() {
            Event::ScrollWheel { delta, precise, .. } => {
                let dy = delta.y();
                let delta_rows = if *precise { dy / self.row_h } else { dy };
                // Positive dy scrolls content up (toward newer) → decrease offset.
                let next = self.scroll.get() - delta_rows;
                self.scroll.set(next.max(0.0));
                let _ = self.clamp_scroll();
                ctx.notify();
                return true;
            }
            Event::MouseMoved { position, .. } if in_bounds(position) => {
                let next = row_at(position);
                if self.hover.get() != next {
                    self.hover.set(next);
                    ctx.notify();
                }
                if let Some(p) = self.origin {
                    ctx.set_cursor(Cursor::PointingHand, p.z_index());
                }
            }
            Event::MouseMoved { .. } => {
                if self.hover.get().is_some() {
                    self.hover.set(None);
                    ctx.notify();
                }
            }
            Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                if let Some(idx) = row_at(position) {
                    let sha = self.frame.commits[idx].sha.clone();
                    ctx.dispatch_typed_action(
                        crate::warpui::shell::CraneShellAction::GitLogSelect(sha),
                    );
                    return true;
                }
            }
            Event::RightMouseDown { position, .. } if in_bounds(position) => {
                if let (Some(cb), Some(idx)) = (self.on_context_menu.clone(), row_at(position)) {
                    let sha = self.frame.commits[idx].sha.clone();
                    cb(&sha, position.x(), position.y(), ctx);
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

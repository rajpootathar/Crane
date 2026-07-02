//! A horizontal bar rendered above the warp editor for Find / Replace / Goto-line.
//! The bar is a plain painted element (no internal warpui widget tree) — it
//! draws rects and glyphs with the PaintContext scene API, tracks button hit
//! regions set during `paint`, and dispatches `EditAction`s from its
//! `dispatch_event` handler.
//!
//! One of three modes is displayed, chosen by `BarMode`:
//!   • `Find`        — magnifying-glass icon + query field + count + prev/next/close
//!   • `FindReplace` — as above plus a replace row with Replace / Replace-All buttons
//!   • `GotoLine`    — arrow icon + "Line:" label + number field + close

use std::rc::Rc;

use warpui::color::ColorU;
use warpui::elements::{Element, Fill, Point};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint};

use crate::warpui::{icons, theme};

/// Height of one bar row in pixels.
const ROW_H: f32 = 32.0;
/// Horizontal padding between bar elements.
const PAD: f32 = 6.0;
/// Icon / button size.
const BTN: f32 = 24.0;
/// Font size for bar text.
const FONT_SZ: f32 = 12.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BarMode {
    Find,
    FindReplace,
    GotoLine,
}

/// Which button was hit (returned internally; actions dispatched via callbacks).
#[derive(Clone, Copy, Debug)]
enum Hit {
    Prev,
    Next,
    Close,
    Replace,
    ReplaceAll,
    FindField,
    ReplaceField,
}

pub struct FindBarElement {
    pub mode: BarMode,
    pub find_query: String,
    pub replace_query: String,
    pub match_count: usize,
    pub current_match: Option<usize>,  // 0-based
    pub find_field_active: bool,       // false = replace field active
    pub font: FamilyId,

    // layout
    size: Option<Vector2F>,
    origin: Option<Point>,
    origin_vec: Option<Vector2F>,
    /// Hit regions set during `paint`; order matches `Hit` discriminants above.
    regions: Vec<(Hit, RectF)>,

    // callbacks — dispatched from `dispatch_event`
    pub on_prev:             Rc<dyn Fn(&mut EventContext)>,
    pub on_next:             Rc<dyn Fn(&mut EventContext)>,
    pub on_close:            Rc<dyn Fn(&mut EventContext)>,
    pub on_replace:          Rc<dyn Fn(&mut EventContext)>,
    pub on_replace_all:      Rc<dyn Fn(&mut EventContext)>,
    pub on_find_field_click:    Rc<dyn Fn(&mut EventContext)>,
    pub on_replace_field_click: Rc<dyn Fn(&mut EventContext)>,
}

impl FindBarElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: BarMode,
        find_query: String,
        replace_query: String,
        match_count: usize,
        current_match: Option<usize>,
        find_field_active: bool,
        font: FamilyId,
        on_prev: Rc<dyn Fn(&mut EventContext)>,
        on_next: Rc<dyn Fn(&mut EventContext)>,
        on_close: Rc<dyn Fn(&mut EventContext)>,
        on_replace: Rc<dyn Fn(&mut EventContext)>,
        on_replace_all: Rc<dyn Fn(&mut EventContext)>,
        on_find_field_click: Rc<dyn Fn(&mut EventContext)>,
        on_replace_field_click: Rc<dyn Fn(&mut EventContext)>,
    ) -> Self {
        Self {
            mode,
            find_query,
            replace_query,
            match_count,
            current_match,
            find_field_active,
            font,
            size: None,
            origin: None,
            origin_vec: None,
            regions: Vec::new(),
            on_prev,
            on_next,
            on_close,
            on_replace,
            on_replace_all,
            on_find_field_click,
            on_replace_field_click,
        }
    }

    /// Total height in pixels (1 or 2 rows).
    fn bar_height(&self) -> f32 {
        match self.mode {
            BarMode::FindReplace => ROW_H * 2.0,
            _ => ROW_H,
        }
    }

    /// Draw one glyph at `pos` using `font`; return the advance width.
    fn draw_char(
        ctx: &mut PaintContext,
        fc: &warpui::fonts::Cache,
        font_id: warpui::fonts::FontId,
        ch: char,
        pos: Vector2F,
        size: f32,
        color: ColorU,
    ) -> f32 {
        if let Some((gid, gfont)) = fc.glyph_for_char(font_id, ch, false) {
            ctx.scene.draw_glyph(pos, gid, gfont, size, color);
            fc.glyph_advance(gfont, size, gid)
                .map(|a| a.x())
                .unwrap_or(size * 0.6)
        } else {
            size * 0.6
        }
    }

    /// Draw a string; return x after the last glyph.
    fn draw_str(
        ctx: &mut PaintContext,
        fc: &warpui::fonts::Cache,
        font_id: warpui::fonts::FontId,
        text: &str,
        mut x: f32,
        y: f32,
        size: f32,
        color: ColorU,
    ) -> f32 {
        for ch in text.chars() {
            x += Self::draw_char(ctx, fc, font_id, ch, vec2f(x, y), size, color);
        }
        x
    }

    /// Approximate pixel width of a string (for pre-allocation; does not draw).
    fn text_width(fc: &warpui::fonts::Cache, font_id: warpui::fonts::FontId, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|ch| {
                fc.glyph_for_char(font_id, ch, false)
                    .and_then(|(gid, gf)| fc.glyph_advance(gf, size, gid).ok())
                    .map(|a| a.x())
                    .unwrap_or(size * 0.6)
            })
            .sum()
    }

    /// Draw a small button (icon or label) with a hover-style background; register region.
    #[allow(clippy::too_many_arguments)]
    fn draw_button(
        ctx: &mut PaintContext,
        fc: &warpui::fonts::Cache,
        font_id: warpui::fonts::FontId,
        icon: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fg: ColorU,
        regions: &mut Vec<(Hit, RectF)>,
        hit: Hit,
    ) {
        let rect = RectF::new(vec2f(x, y), vec2f(w, h));
        ctx.scene.draw_rect_without_hit_recording(rect).with_background(Fill::Solid(theme::surface()));
        // vertically center the icon text
        let ascent = fc.ascent(font_id, FONT_SZ);
        let descent = fc.descent(font_id, FONT_SZ);
        let text_h = ascent - descent;
        let baseline = (h - text_h) * 0.5 + ascent;
        let text_w = Self::text_width(fc, font_id, icon, FONT_SZ);
        let tx = x + (w - text_w) * 0.5;
        Self::draw_str(ctx, fc, font_id, icon, tx, y + baseline, FONT_SZ, fg);
        regions.push((hit, rect));
    }
}

impl Element for FindBarElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let size = vec2f(constraint.max.x(), self.bar_height());
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        self.regions.clear();
        let size = self.size.unwrap_or_else(|| vec2f(300.0, ROW_H));
        let w = size.x();

        // Look up fonts.
        let fc = app.font_cache();
        let mono_id = fc.select_font(self.font, Properties::default());
        let icon_fam = fc.family_id_for_name("phosphor");
        let icon_id = icon_fam
            .map(|fam| fc.select_font(fam, Properties::default()))
            .unwrap_or(mono_id);

        // ascent/descent for vertical centering on row 1.
        let ascent = fc.ascent(mono_id, FONT_SZ);
        let descent = fc.descent(mono_id, FONT_SZ);
        let text_h = ascent - descent;
        let baseline = (ROW_H - text_h) * 0.5 + ascent;

        let fg_text = theme::text();
        let fg_muted = theme::text_muted();
        let fg_active = theme::accent();
        let bar_bg = theme::surface();
        let field_bg = theme::bg();
        let sep = theme::border();

        // ── Draw bar background (all rows) ──────────────────────────────────
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::Solid(bar_bg));
        // bottom border
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(origin.x(), origin.y() + self.bar_height() - 1.0),
                vec2f(w, 1.0),
            ))
            .with_background(Fill::Solid(sep));

        match self.mode {
            BarMode::GotoLine => {
                // ── Goto-line bar ────────────────────────────────────────────
                let oy = origin.y();
                let mut x = origin.x() + PAD;

                // Arrow icon
                let arrow_icon = icons::ARROW_CLOCKWISE;
                let icon_w = Self::text_width(fc, icon_id, arrow_icon, FONT_SZ) + PAD;
                Self::draw_str(ctx, fc, icon_id, arrow_icon, x, oy + baseline, FONT_SZ, fg_active);
                x += icon_w;

                // "Line:" label
                let label = "Line:";
                x = Self::draw_str(ctx, fc, mono_id, label, x, oy + baseline, FONT_SZ, fg_muted) + PAD;

                // Number input field
                let close_w = BTN + PAD;
                let field_w = (w - x - close_w - PAD).max(60.0);
                let field_rect = RectF::new(vec2f(x, oy + 4.0), vec2f(field_w, ROW_H - 8.0));
                ctx.scene
                    .draw_rect_without_hit_recording(field_rect)
                    .with_background(Fill::Solid(field_bg));
                let show = if self.find_query.is_empty() { "1" } else { &self.find_query };
                let q_color = if self.find_query.is_empty() { fg_muted } else { fg_text };
                Self::draw_str(ctx, fc, mono_id, show, x + 4.0, oy + baseline, FONT_SZ, q_color);
                self.regions.push((Hit::FindField, field_rect));
                x += field_w + PAD;

                // Close [X]
                Self::draw_button(
                    ctx, fc, icon_id, icons::X,
                    x, oy, BTN, ROW_H,
                    fg_muted, &mut self.regions, Hit::Close,
                );
            }

            BarMode::Find | BarMode::FindReplace => {
                // ── Row 1: Find ──────────────────────────────────────────────
                let oy = origin.y();
                let mut x = origin.x() + PAD;

                // Magnifying glass icon
                let mg_w = Self::text_width(fc, icon_id, icons::MAGNIFYING_GLASS, FONT_SZ) + PAD;
                Self::draw_str(
                    ctx, fc, icon_id, icons::MAGNIFYING_GLASS,
                    x, oy + baseline, FONT_SZ,
                    if self.find_field_active || self.mode == BarMode::Find { fg_active } else { fg_muted },
                );
                x += mg_w;

                // Count text (fixed-width on the right side, before buttons)
                let btn_area = BTN * 3.0 + PAD * 3.0 + PAD; // 3 buttons + spacing
                let count_w = 56.0;
                let field_end = w - btn_area - count_w;
                let field_w = (field_end - x - PAD).max(40.0);

                // Find query field
                let find_active = self.find_field_active || self.mode == BarMode::Find;
                let field_border = if find_active { fg_active } else { sep };
                let find_rect = RectF::new(vec2f(x, oy + 4.0), vec2f(field_w, ROW_H - 8.0));
                ctx.scene
                    .draw_rect_without_hit_recording(find_rect)
                    .with_background(Fill::Solid(field_bg));
                // 1px border on the bottom when active
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(x, oy + ROW_H - 6.0),
                        vec2f(field_w, 1.0),
                    ))
                    .with_background(Fill::Solid(field_border));
                let q_color = if self.find_query.is_empty() { fg_muted } else { fg_text };
                let q_show = if self.find_query.is_empty() { "find…" } else { &self.find_query };
                Self::draw_str(ctx, fc, mono_id, q_show, x + 4.0, oy + baseline, FONT_SZ, q_color);
                self.regions.push((Hit::FindField, find_rect));
                x += field_w + PAD;

                // Match count "N/M" or "no matches"
                let count_str = match (self.match_count, self.current_match) {
                    (0, _) => "no match".to_string(),
                    (total, Some(cur)) => format!("{}/{}", cur + 1, total),
                    (total, None) => format!("0/{}", total),
                };
                let count_color = if self.match_count == 0 { fg_muted } else { fg_text };
                Self::draw_str(ctx, fc, mono_id, &count_str, x, oy + baseline, FONT_SZ, count_color);
                x = field_end + count_w;

                // Prev [^], Next [v], Close [X]
                Self::draw_button(ctx, fc, icon_id, icons::CARET_UP,
                    x, oy, BTN, ROW_H, fg_text, &mut self.regions, Hit::Prev);
                x += BTN + PAD;
                Self::draw_button(ctx, fc, icon_id, icons::CARET_DOWN,
                    x, oy, BTN, ROW_H, fg_text, &mut self.regions, Hit::Next);
                x += BTN + PAD;
                Self::draw_button(ctx, fc, icon_id, icons::X,
                    x, oy, BTN, ROW_H, fg_muted, &mut self.regions, Hit::Close);

                // ── Row 2: Replace (FindReplace only) ───────────────────────
                if self.mode == BarMode::FindReplace {
                    let oy2 = origin.y() + ROW_H;
                    let mut rx = origin.x() + PAD;

                    // Pencil icon
                    let pen_w = Self::text_width(fc, icon_id, icons::PENCIL_SIMPLE, FONT_SZ) + PAD;
                    Self::draw_str(
                        ctx, fc, icon_id, icons::PENCIL_SIMPLE,
                        rx, oy2 + baseline, FONT_SZ,
                        if !self.find_field_active { fg_active } else { fg_muted },
                    );
                    rx += pen_w;

                    // Replace query field (same width as find field)
                    let rep_active = !self.find_field_active;
                    let rep_border = if rep_active { fg_active } else { sep };
                    let rep_rect = RectF::new(vec2f(rx, oy2 + 4.0), vec2f(field_w, ROW_H - 8.0));
                    ctx.scene
                        .draw_rect_without_hit_recording(rep_rect)
                        .with_background(Fill::Solid(field_bg));
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(
                            vec2f(rx, oy2 + ROW_H - 6.0),
                            vec2f(field_w, 1.0),
                        ))
                        .with_background(Fill::Solid(rep_border));
                    let rq_color = if self.replace_query.is_empty() { fg_muted } else { fg_text };
                    let rq_show = if self.replace_query.is_empty() { "replace…" } else { &self.replace_query };
                    Self::draw_str(ctx, fc, mono_id, rq_show, rx + 4.0, oy2 + baseline, FONT_SZ, rq_color);
                    self.regions.push((Hit::ReplaceField, rep_rect));
                    rx += field_w + PAD;

                    // [Replace] button
                    let replace_label = "Replace";
                    let replace_lw = Self::text_width(fc, mono_id, replace_label, FONT_SZ) + PAD * 2.0;
                    let rep_btn_rect = RectF::new(vec2f(rx, oy2 + 4.0), vec2f(replace_lw, ROW_H - 8.0));
                    ctx.scene
                        .draw_rect_without_hit_recording(rep_btn_rect)
                        .with_background(Fill::Solid(theme::surface()));
                    let r_ascent = fc.ascent(mono_id, FONT_SZ);
                    let r_descent = fc.descent(mono_id, FONT_SZ);
                    let r_text_h = r_ascent - r_descent;
                    let r_baseline = (ROW_H - 8.0 - r_text_h) * 0.5 + r_ascent;
                    Self::draw_str(ctx, fc, mono_id, replace_label, rx + PAD, oy2 + 4.0 + r_baseline, FONT_SZ, fg_text);
                    self.regions.push((Hit::Replace, rep_btn_rect));
                    rx += replace_lw + PAD;

                    // [Replace All] button
                    let rep_all_label = "Replace All";
                    let rep_all_lw = Self::text_width(fc, mono_id, rep_all_label, FONT_SZ) + PAD * 2.0;
                    let rep_all_rect = RectF::new(vec2f(rx, oy2 + 4.0), vec2f(rep_all_lw, ROW_H - 8.0));
                    ctx.scene
                        .draw_rect_without_hit_recording(rep_all_rect)
                        .with_background(Fill::Solid(theme::surface()));
                    Self::draw_str(ctx, fc, mono_id, rep_all_label, rx + PAD, oy2 + 4.0 + r_baseline, FONT_SZ, fg_text);
                    self.regions.push((Hit::ReplaceAll, rep_all_rect));
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
        ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        let Event::LeftMouseDown { position, .. } = event.raw_event() else {
            return false;
        };
        let in_rect = |r: RectF| -> bool {
            position.x() >= r.origin().x()
                && position.x() <= r.origin().x() + r.size().x()
                && position.y() >= r.origin().y()
                && position.y() <= r.origin().y() + r.size().y()
        };
        for (hit, rect) in &self.regions {
            if in_rect(*rect) {
                match hit {
                    Hit::Prev            => (self.on_prev)(ctx),
                    Hit::Next            => (self.on_next)(ctx),
                    Hit::Close           => (self.on_close)(ctx),
                    Hit::Replace         => (self.on_replace)(ctx),
                    Hit::ReplaceAll      => (self.on_replace_all)(ctx),
                    Hit::FindField       => (self.on_find_field_click)(ctx),
                    Hit::ReplaceField    => (self.on_replace_field_click)(ctx),
                }
                return true;
            }
        }
        false
    }
}

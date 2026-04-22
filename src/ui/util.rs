//! Crane widget primitives. Use these in place of raw egui::Button /
//! ui.button / ui.small_button so every surface in the app shares one style.

use egui::{Color32, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icons;

// --- Shared tokens ---

pub const TEXT_BTN_H: f32 = 28.0;
pub const BTN_TEXT_SIZE: f32 = 12.5;
pub const ICON_BTN_SIZE: Vec2 = Vec2::new(28.0, 24.0);

pub const ROW_H: f32 = 26.0;
pub const INDENT_W: f32 = 10.0;
pub const CHEVRON_W: f32 = 14.0;

// Colour accessors — read from the active theme every call.
// Previously these were const Color32 for the dark theme which stranded
// light themes with white-on-white text.
pub fn text() -> Color32 {
    crate::theme::current().text.to_color32()
}
pub fn muted() -> Color32 {
    crate::theme::current().text_muted.to_color32()
}
pub fn header_fg() -> Color32 {
    crate::theme::current().text_header.to_color32()
}
pub fn accent() -> Color32 {
    crate::theme::current().accent.to_color32()
}
pub fn row_hover() -> Color32 {
    crate::theme::current().row_hover.to_color32()
}
pub fn row_active() -> Color32 {
    let a = crate::theme::current().accent;
    Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)
}
pub fn trailing_hover() -> Color32 {
    crate::theme::current().surface_alt.to_color32()
}

// --- Buttons ---

/// Borderless icon-only button — transparent resting, subtle hover tint.
pub fn icon_button(ui: &mut Ui, glyph: &str, size: f32, tooltip: &str) -> Response {
    let resp = ui
        .scope(|ui| {
            let v = ui.visuals_mut();
            v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.hovered.bg_stroke = Stroke::NONE;
            v.widgets.active.bg_stroke = Stroke::NONE;
            ui.add(
                egui::Button::new(RichText::new(glyph).size(size)).min_size(ICON_BTN_SIZE),
            )
        })
        .inner;
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// Full-width filled button — spans remaining horizontal space.
pub fn full_width_primary_button(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    tooltip: &str,
) -> Response {
    let text = match icon {
        Some(g) => format!("{g}  {label}"),
        None => label.to_string(),
    };
    let width = ui.available_width();
    let resp = ui.add(
        egui::Button::new(RichText::new(text).size(BTN_TEXT_SIZE))
            .min_size(Vec2::new(width, TEXT_BTN_H)),
    );
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// Uppercase muted section header.
pub fn section_header(ui: &mut Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(label)
                .size(10.5)
                .color(header_fg())
                .strong(),
        );
    });
}

// --- Tree row primitive ---

pub struct RowConfig<'a> {
    pub depth: usize,
    pub expanded: Option<bool>,
    pub leading: Option<&'a str>,
    pub leading_color: Option<Color32>,
    pub label: &'a str,
    pub label_color: Option<Color32>,
    pub is_active: bool,
    pub active_bar: bool,
    pub badge: Option<(usize, usize, Color32, Color32)>,
    /// Number of trailing icon buttons that will be drawn on this row.
    /// Used to reserve right-edge space so badges don't collide with them.
    pub trailing_count: usize,
    /// Draw vertical tree-guide lines at each ancestor depth. Used to
    /// give the multi-repo Project tree a file-explorer feel without
    /// affecting flat lists elsewhere. Default off.
    pub tree_guides: bool,
    /// When `Some(checked)`, render a persistent checkbox just after the
    /// expand chevron. Click on the checkbox is reported in
    /// `RowResult::checkbox_clicked`; click anywhere else still fires
    /// `main_clicked`. Used by the Changes tree for JetBrains-style
    /// stage / unstage toggles.
    pub checkbox: Option<bool>,
}

pub struct RowResult {
    pub rect: Rect,
    pub main_clicked: bool,
    pub hovered: bool,
    /// True when the user clicked the checkbox rendered by
    /// `RowConfig::checkbox`. `main_clicked` stays false in that case so
    /// callers can dispatch stage/unstage without also triggering the
    /// row's primary action (e.g. open diff).
    pub checkbox_clicked: bool,
    /// Response for the row's click-sense rect. Used by callers that need
    /// `.context_menu(...)` for right-click actions.
    pub response: Response,
}

pub fn draw_row(ui: &mut Ui, cfg: RowConfig<'_>) -> RowResult {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let hovered = response.hovered();

    // Smooth hover fade — 90 ms ease so the tint doesn't flicker as the
    // pointer sweeps through the list. `animate_bool_with_time` returns
    // 0..1, which we use as alpha multiplier on the hover tint.
    let hover_id = response.id.with("row_hover_t");
    let hover_t = ui.ctx().animate_bool_with_time(hover_id, hovered, 0.09);
    if cfg.is_active {
        painter.rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 4.0, row_active());
    } else if hover_t > 0.01 {
        let base = row_hover();
        let faded = Color32::from_rgba_unmultiplied(
            base.r(),
            base.g(),
            base.b(),
            (base.a() as f32 * hover_t) as u8,
        );
        painter.rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 4.0, faded);
    }
    if cfg.active_bar {
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.min.x + 4.0, rect.min.y + 3.0),
                Vec2::new(2.0, rect.height() - 6.0),
            ),
            1.0,
            accent(),
        );
    }

    if cfg.tree_guides && cfg.depth > 0 {
        let guide_color = crate::theme::current().divider.to_color32();
        for d in 0..cfg.depth {
            let x = rect.min.x + 12.0 + (d as f32 * INDENT_W) + CHEVRON_W / 2.0;
            painter.line_segment(
                [
                    Pos2::new(x, rect.min.y),
                    Pos2::new(x, rect.max.y),
                ],
                Stroke::new(1.0, guide_color),
            );
        }
    }

    let mut cursor_x = rect.min.x + 12.0 + (cfg.depth as f32 * INDENT_W);

    if let Some(expanded) = cfg.expanded {
        // Animate the caret glyph swap with a ~110 ms cross-fade so
        // expand/collapse feels continuous instead of jumping.
        let chev_id = response.id.with("row_chev_t");
        let t = ui.ctx().animate_bool_with_time(chev_id, expanded, 0.11);
        let col_base = if cfg.is_active { text() } else { muted() };
        let fade = |c: Color32, alpha: f32| -> Color32 {
            Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                (c.a() as f32 * alpha.clamp(0.0, 1.0)) as u8,
            )
        };
        if t < 0.999 {
            painter.text(
                Pos2::new(cursor_x + CHEVRON_W / 2.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                icons::CARET_RIGHT,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                fade(col_base, 1.0 - t),
            );
        }
        if t > 0.001 {
            painter.text(
                Pos2::new(cursor_x + CHEVRON_W / 2.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                icons::CARET_DOWN,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                fade(col_base, t),
            );
        }
    }
    cursor_x += CHEVRON_W + 2.0;

    // Optional persistent checkbox. The row itself was already
    // allocated with `click_and_drag`, so a fresh `ui.interact` here
    // loses to the row for hover + click. Use `rect_contains_pointer`
    // + primary-click detection so the checkbox can paint its own
    // hover state and steal the click from the row reliably.
    let mut checkbox_clicked = false;
    if let Some(checked) = cfg.checkbox {
        let cb_rect = Rect::from_min_size(
            Pos2::new(cursor_x, rect.center().y - 9.0),
            Vec2::splat(18.0),
        );
        let pointer_over = ui.rect_contains_pointer(cb_rect);
        // Hover background fades in over 80 ms — same feel as the row
        // tint, just scoped to the checkbox square.
        let cb_hover_id = response.id.with("row_cb_hover_t");
        let cb_hover_t = ui.ctx().animate_bool_with_time(cb_hover_id, pointer_over, 0.08);
        if cb_hover_t > 0.01 {
            let base = trailing_hover();
            let faded = Color32::from_rgba_unmultiplied(
                base.r(), base.g(), base.b(),
                (base.a() as f32 * cb_hover_t) as u8,
            );
            painter.rect_filled(cb_rect, 4.0, faded);
        }
        if pointer_over {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if ui.input(|i| i.pointer.primary_clicked()) {
                checkbox_clicked = true;
            }
        }
        // Cross-fade between the empty box and the filled check so the
        // state change reads as a transition, not a glyph swap. 130 ms
        // is long enough to notice, short enough to feel responsive.
        let cb_state_id = response.id.with("row_cb_state_t");
        let state_t = ui.ctx().animate_bool_with_time(cb_state_id, checked, 0.13);
        let fade = |c: Color32, a: f32| -> Color32 {
            Color32::from_rgba_unmultiplied(
                c.r(), c.g(), c.b(),
                (c.a() as f32 * a.clamp(0.0, 1.0)) as u8,
            )
        };
        // A tiny scale pop when state_t is mid-transition — peaks at
        // 0.5 and returns to 1.0 at both ends.
        let pop = 1.0 + (state_t * std::f32::consts::PI).sin() * 0.12;
        let font_size = 14.0 * pop;
        if state_t < 0.999 {
            painter.text(
                cb_rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::SQUARE,
                egui::FontId::new(font_size, egui::FontFamily::Proportional),
                fade(muted(), 1.0 - state_t),
            );
        }
        if state_t > 0.001 {
            painter.text(
                cb_rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::CHECK_SQUARE,
                egui::FontId::new(font_size, egui::FontFamily::Proportional),
                fade(accent(), state_t),
            );
        }
        cursor_x += 20.0;
    }

    if let Some(leading) = cfg.leading {
        let color = cfg.leading_color.unwrap_or(muted());
        painter.text(
            Pos2::new(cursor_x + 8.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            leading,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            color,
        );
        cursor_x += 18.0;
    }

    let text_color = cfg.label_color.unwrap_or(text());
    // Reserve right-edge space for badge + trailing buttons so a long
    // label (e.g. a verbose git branch name) can't overrun into them.
    // We clip the label paint to the allowable rect rather than
    // elide with "…" — the clean right-edge cutoff is enough of a
    // visual hint that the text continues, matches how VS Code /
    // Finder truncate narrow sidebar labels, and avoids the measure-
    // and-shrink loop that an ellipsis pass would need.
    let badge_reserve: f32 = if cfg.badge.is_some() { 64.0 } else { 0.0 };
    let trailing_reserve = (cfg.trailing_count as f32) * 22.0;
    let label_right = rect.max.x - 8.0 - badge_reserve - trailing_reserve;
    let label_clip = Rect::from_min_max(
        Pos2::new(cursor_x, rect.min.y),
        Pos2::new(label_right.max(cursor_x + 1.0), rect.max.y),
    );
    painter.with_clip_rect(label_clip).text(
        Pos2::new(cursor_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        cfg.label,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        text_color,
    );

    if let Some((added, deleted, add_color, del_color)) = cfg.badge {
        let trailing_reserve = (cfg.trailing_count as f32) * 22.0;
        let mut bx = rect.max.x - 10.0 - trailing_reserve;
        if deleted > 0 {
            let s = format!("-{deleted}");
            let galley = painter.layout_no_wrap(
                s,
                egui::FontId::new(10.5, egui::FontFamily::Proportional),
                del_color,
            );
            bx -= galley.size().x + 4.0;
            painter.galley(
                Pos2::new(bx, rect.center().y - galley.size().y / 2.0),
                galley,
                del_color,
            );
        }
        if added > 0 {
            let s = format!("+{added}");
            let galley = painter.layout_no_wrap(
                s,
                egui::FontId::new(10.5, egui::FontFamily::Proportional),
                add_color,
            );
            bx -= galley.size().x + 4.0;
            painter.galley(
                Pos2::new(bx, rect.center().y - galley.size().y / 2.0),
                galley,
                add_color,
            );
        }
    }

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Swallow the main click when it landed on the checkbox so the row
    // doesn't fire its default action at the same time.
    let main_clicked = response.clicked() && !checkbox_clicked;
    RowResult {
        rect,
        main_clicked,
        hovered,
        checkbox_clicked,
        response,
    }
}

/// Draw up to N trailing icon buttons on the right edge of a row.
/// Always registers the hitbox so clicks work on the first hover frame;
/// paints only when the row (or the button itself) is hovered.
pub fn draw_trailing(
    ui: &mut Ui,
    rect: Rect,
    row_hovered: bool,
    actions: &[(&str, &str, usize)],
) -> [bool; 4] {
    let mut out = [false; 4];
    let size = 20.0;
    let mut x = rect.max.x - 8.0;
    for (icon, tip, slot) in actions.iter().rev() {
        x -= size;
        let btn_rect = Rect::from_min_size(
            Pos2::new(x, rect.center().y - size / 2.0),
            Vec2::splat(size),
        );
        let id = ui
            .id()
            .with(("trailing", rect.min.x as i32, rect.min.y as i32, *slot));
        let resp = ui.interact(btn_rect, id, Sense::click()).on_hover_text(*tip);
        if row_hovered || resp.hovered() {
            let painter = ui.painter_at(btn_rect);
            if resp.hovered() {
                painter.rect_filled(btn_rect, 4.0, trailing_hover());
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            painter.text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                *icon,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
                text(),
            );
        }
        if resp.clicked() && *slot < 4 {
            out[*slot] = true;
        }
        x -= 2.0;
    }
    out
}

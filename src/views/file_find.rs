//! Cmd+F find bar and per-match highlight overlay for the Files pane.
//!
//! Extracted from `file_view.rs` to keep the main editor loop readable.
//! `render_find_bar` renders the bar + consumes keys (Enter / Shift+Enter
//! / Esc) and returns which nav action fired this frame. The caller
//! (file_view::render) is responsible for clearing `tab.find_query` on
//! close and doing the actual cursor jump on `next` / `prev`.

use crate::theme;
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;

pub struct FindBarOutcome {
    pub close: bool,
    pub next: bool,
    pub prev: bool,
}

pub fn render_find_bar(
    ui: &mut egui::Ui,
    tab: &mut crate::state::layout::FileTab,
) -> FindBarOutcome {
    let mut close = false;
    let mut next = false;
    let mut prev = false;
    let Some(query) = tab.find_query.as_mut() else {
        // Bar just closed — reset the one-shot focus flag so the next
        // Cmd+F will refocus cleanly.
        let focus_flag = egui::Id::new(("find_focused", &tab.path));
        ui.memory_mut(|m| {
            m.data.remove::<bool>(focus_flag);
        });
        return FindBarOutcome { close, next, prev };
    };
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{}  Find", icons::MAGNIFYING_GLASS))
                .size(11.0)
                .color(theme::current().text_muted.to_color32()),
        );
        let input_id = egui::Id::new(("find_input", &tab.path));
        let resp = ui.add(
            egui::TextEdit::singleline(query)
                .id(input_id)
                .desired_width(ui.available_width() - 180.0)
                .hint_text("type to search…"),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            next = true;
        }
        // Focus ONCE when the bar opens — per-frame request_focus was
        // stealing clicks from the nav/close buttons.
        let focus_flag = egui::Id::new(("find_focused", &tab.path));
        let already_focused = ui
            .memory(|m| m.data.get_temp::<bool>(focus_flag))
            .unwrap_or(false);
        if !already_focused {
            resp.request_focus();
            ui.memory_mut(|m| m.data.insert_temp(focus_flag, true));
        }
        let hits = if query.is_empty() {
            0
        } else {
            tab.content.matches(query.as_str()).count()
        };
        ui.label(
            RichText::new(format!("{hits} hits"))
                .size(10.5)
                .color(theme::current().text_muted.to_color32()),
        );
        let btn = |glyph: &str| {
            egui::Button::new(
                RichText::new(glyph)
                    .size(14.0)
                    .color(theme::current().text.to_color32()),
            )
            .min_size(egui::vec2(22.0, 22.0))
        };
        if ui
            .add(btn(icons::ARROW_UP))
            .on_hover_text("Previous (Shift+Enter)")
            .clicked()
        {
            prev = true;
        }
        if ui
            .add(btn(icons::ARROW_DOWN))
            .on_hover_text("Next (Enter)")
            .clicked()
        {
            next = true;
        }
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.add_space(6.0);
                if ui
                    .add(btn(icons::X_CIRCLE))
                    .on_hover_text("Close (Esc)")
                    .clicked()
                {
                    close = true;
                }
            },
        );
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.shift) {
        prev = true;
    }
    ui.add_space(2.0);
    FindBarOutcome { close, next, prev }
}

/// Paint a soft amber fill behind every occurrence of `query` in the
/// visible galley. Called from the editor render path after the
/// TextEdit output is available so the highlight sits under the text.
pub fn paint_find_matches(
    ui: &egui::Ui,
    galley: &std::sync::Arc<egui::Galley>,
    origin: egui::Pos2,
    text: &str,
    query: &str,
) {
    let amber = Color32::from_rgba_unmultiplied(220, 180, 50, 90);
    let painter = ui.painter();
    let mut byte = 0usize;
    while let Some(offset) = text[byte..].find(query) {
        let abs = byte + offset;
        let end = abs + query.len();
        let char_start = text[..abs].chars().count();
        let char_end = char_start + text[abs..end].chars().count();
        let r_start = galley.pos_from_cursor(egui::text::CCursor::new(char_start));
        let r_end = galley.pos_from_cursor(egui::text::CCursor::new(char_end));
        // Only paint matches that fit on a single visual line (the common
        // case for a user-typed query; skipping multi-line avoids ugly
        // cross-row rectangles).
        if (r_start.max.y - r_end.max.y).abs() < 1.0 {
            let rect = egui::Rect::from_min_max(
                egui::pos2(origin.x + r_start.min.x, origin.y + r_start.min.y),
                egui::pos2(origin.x + r_end.max.x, origin.y + r_start.max.y),
            );
            painter.rect_filled(rect, 2.0, amber);
        }
        byte = end;
    }
}

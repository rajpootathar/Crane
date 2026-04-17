use egui::{Color32, RichText, Stroke};

/// Borderless icon-only button. No inactive background, subtle hover tint.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, size: f32, tooltip: &str) -> egui::Response {
    let resp = ui
        .scope(|ui| {
            let v = ui.visuals_mut();
            v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.hovered.bg_stroke = Stroke::NONE;
            v.widgets.active.bg_stroke = Stroke::NONE;
            ui.add(
                egui::Button::new(RichText::new(glyph).size(size))
                    .min_size(egui::vec2(28.0, 24.0)),
            )
        })
        .inner;
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

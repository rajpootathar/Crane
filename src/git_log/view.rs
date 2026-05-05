use egui::{Color32, Pos2, Rect, Stroke};
use egui_phosphor::regular as icons;

use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

const HEADER_H: f32 = 28.0;

/// Render the Git Log bottom region inside `region`. Mutates `state`
/// (worker poll, header chrome). Returns `true` if the user clicked
/// the close (×) button — caller should flip `tab.git_log_visible`
/// to false.
pub fn render(ui: &mut egui::Ui, region: Rect, state: &mut GitLogState) -> bool {
    let mut request_close = false;
    state.poll_worker();

    ui.painter()
        .rect_filled(region, 0.0, Color32::from_rgb(20, 22, 28));

    // Header strip
    let header = Rect::from_min_max(
        region.min,
        Pos2::new(region.max.x, region.min.y + HEADER_H),
    );
    let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(header));
    header_ui.set_clip_rect(header);
    header_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Git Log").strong());
        ui.add_space(8.0);

        if state.is_loading() {
            ui.spinner();
            ui.label(
                egui::RichText::new("loading…")
                    .small()
                    .color(muted()),
            );
        } else if let Some(frame) = state.frame.as_ref() {
            ui.label(
                egui::RichText::new(format!("{} commits", frame.commits.len()))
                    .small()
                    .color(muted()),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui
                .button(icons::X)
                .on_hover_text("Close (Cmd+9)")
                .clicked()
            {
                request_close = true;
            }
            ui.add_space(4.0);
            if ui
                .button(icons::ARROW_COUNTER_CLOCKWISE)
                .on_hover_text("Refresh")
                .clicked()
            {
                // Drop cached frame; the next maybe_reload tick will
                // see no in-flight worker and kick a fresh load.
                state.frame = None;
                state.worker_rx = None;
            }
        });
    });

    // Body region
    let body = Rect::from_min_max(
        Pos2::new(region.min.x, region.min.y + HEADER_H),
        region.max,
    );
    ui.painter().rect_stroke(
        body,
        0.0,
        Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
        egui::epaint::StrokeKind::Inside,
    );

    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    body_ui.set_clip_rect(body);
    body_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(
            egui::vec2(state.col_refs_width, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(egui::RichText::new("Refs").color(muted()).small());
            },
        );
        ui.separator();

        let mid_w = (body.width() - state.col_refs_width - state.col_details_width - 24.0)
            .max(160.0);
        ui.allocate_ui_with_layout(
            egui::vec2(mid_w, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                if let Some(frame) = state.frame.as_ref() {
                    ui.label(format!("{} commits loaded", frame.commits.len()));
                } else if state.is_loading() {
                    ui.label("loading…");
                } else {
                    ui.label(egui::RichText::new("no data").color(muted()).small());
                }
            },
        );

        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(state.col_details_width, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(egui::RichText::new("Details").color(muted()).small());
            },
        );
    });

    request_close
}

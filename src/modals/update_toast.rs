use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.update_check.should_show() {
        return;
    }
    let version = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.version.clone())
        .unwrap_or_default();
    let url = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.url.clone())
        .unwrap_or_default();

    let screen = ctx.content_rect();
    let toast_w = 440.0_f32.min(screen.width() - 40.0);
    egui::Area::new(egui::Id::new("update_toast"))
        .order(egui::Order::Tooltip)
        .fixed_pos(egui::pos2(
            screen.max.x - toast_w - 20.0,
            screen.max.y - 140.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(egui::Color32::from_rgb(28, 32, 44))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 66, 86)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(toast_w - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::ARROW_CIRCLE_UP)
                                .size(18.0)
                                .color(egui::Color32::from_rgb(96, 140, 220)),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Crane v{version} is available"))
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(212, 216, 228))
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "You're on v{}. Grab the new build?",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .size(11.5)
                                .color(egui::Color32::from_rgb(150, 156, 172)),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!(
                                    "{}  Download",
                                    egui_phosphor::regular::DOWNLOAD_SIMPLE
                                ))
                                .size(12.0)
                                .strong(),
                            )
                            .clicked()
                        {
                            let _ = webbrowser::open(&url);
                            app.update_check.dismiss_forever();
                        }
                        if ui
                            .button(egui::RichText::new("Not now").size(12.0))
                            .clicked()
                        {
                            app.update_check.dismiss_session();
                        }
                        if ui
                            .button(egui::RichText::new("Remind in 7 days").size(12.0))
                            .clicked()
                        {
                            app.update_check.remind_later();
                        }
                    });
                });
        });
}

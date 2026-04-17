use crate::state::App;
use crate::theme;

pub fn render(ctx: &egui::Context, app: &mut App, apply_style: impl FnOnce(&egui::Context)) {
    if !app.show_settings {
        return;
    }
    let mut open = true;
    let mut selected_now: Option<String> = None;
    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(420.0, 380.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Theme").strong());
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for theme in theme::load_all() {
                        let is_active = app.selected_theme == theme.name;
                        let label = format!(
                            "{}{}",
                            if is_active { "● " } else { "  " },
                            theme.name
                        );
                        let resp = ui.add(
                            egui::Button::new(egui::RichText::new(label).size(13.0))
                                .min_size(egui::vec2(ui.available_width(), 28.0)),
                        );
                        if resp.clicked() && !is_active {
                            selected_now = Some(theme.name.clone());
                        }
                    }
                });
            ui.add_space(8.0);
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "Drop custom themes (*.toml) at: {}",
                    theme::themes_dir().display()
                ))
                .size(10.5)
                .color(theme::current().text_muted.to_color32()),
            );
            if ui.small_button("Open themes folder").clicked() {
                let dir = theme::themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                super::open_in_file_manager(&dir);
            }
        });
    if !open {
        app.show_settings = false;
    }
    if let Some(name) = selected_now
        && let Some(t) = theme::find_by_name(&name)
    {
        theme::set(t);
        app.selected_theme = name;
        apply_style(ctx);
        ctx.request_repaint();
    }
}

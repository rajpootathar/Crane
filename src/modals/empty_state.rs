use crate::state::App;

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
    let mut empty_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::centered_and_justified(
                egui::Direction::TopDown,
            )),
    );
    empty_ui.set_clip_rect(rect);
    empty_ui.vertical_centered(|ui| {
        ui.add_space(rect.height() * 0.25);
        let has_project = !app.projects.is_empty();
        let (title, hint) = if has_project {
            ("No tabs open", "Cmd+T to create a new terminal tab")
        } else {
            (
                "Welcome to Crane",
                "Add a project from the Left Panel to get started",
            )
        };
        ui.label(
            egui::RichText::new(title)
                .size(18.0)
                .color(egui::Color32::from_rgb(200, 204, 220)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(hint)
                .size(12.0)
                .color(egui::Color32::from_rgb(130, 136, 150)),
        );
        ui.add_space(20.0);
        if has_project {
            if ui
                .add_sized(
                    [180.0, 32.0],
                    egui::Button::new(egui::RichText::new("+ New Terminal Tab").size(13.0)),
                )
                .clicked()
            {
                app.new_tab_in_active_workspace(ctx);
            }
        } else if ui
            .add_sized(
                [180.0, 32.0],
                egui::Button::new(
                    egui::RichText::new(format!(
                        "{}  Add Project…",
                        egui_phosphor::regular::FOLDER_PLUS
                    ))
                    .size(13.0),
                ),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose project folder")
                .pick_folder()
        {
            app.add_project_from_path(path, ctx);
        }
    });
}

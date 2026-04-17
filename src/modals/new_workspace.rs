use crate::state::{self, App};

pub fn render(ctx: &egui::Context, app: &mut App) {
    let mut open = app.new_workspace_modal.is_some();
    if !open {
        return;
    }
    let mut create = false;
    let mut cancel = false;
    let mut browse: Option<String> = None;
    let modal_width = 480.0;
    let project_info = app.new_workspace_modal.as_ref().and_then(|m| {
        app.projects
            .iter()
            .find(|p| p.id == m.project_id)
            .map(|p| (p.path.clone(), p.name.clone()))
    });

    egui::Window::new("New Worktree")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(modal_width, 280.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(modal_width - 20.0);
            let input_width = modal_width - 40.0;
            if let Some(modal) = app.new_workspace_modal.as_mut() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Branch").strong());
                ui.add(
                    egui::TextEdit::singleline(&mut modal.branch)
                        .hint_text("feature/my-branch")
                        .desired_width(input_width),
                );
                ui.checkbox(&mut modal.create_new_branch, "Create new branch");
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Location").strong());
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut modal.mode, state::LocationMode::Global, "Global")
                        .on_hover_text("~/.crane-worktrees/<project>/<branch>");
                    ui.selectable_value(
                        &mut modal.mode,
                        state::LocationMode::ProjectLocal,
                        "Project-local",
                    )
                    .on_hover_text("<project>/.crane-worktrees/<branch>");
                    ui.selectable_value(&mut modal.mode, state::LocationMode::Custom, "Custom")
                        .on_hover_text("Pick any folder");
                });
                if modal.mode == state::LocationMode::Custom {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut modal.custom_path)
                                .hint_text("/path/to/parent")
                                .desired_width(input_width - 88.0),
                        );
                        if ui.button("Browse…").clicked() {
                            browse = Some(modal.custom_path.clone());
                        }
                    });
                }
                let preview = project_info
                    .as_ref()
                    .map(|(p, n)| modal.resolved_parent(p, n))
                    .unwrap_or_default();
                let preview_str = format!(
                    "→ {}/{}",
                    preview.display().to_string().trim_end_matches('/'),
                    if modal.branch.is_empty() {
                        "<branch>"
                    } else {
                        &modal.branch
                    }
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(preview_str)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(130, 136, 150)),
                    )
                    .truncate(),
                );
                if let Some(err) = &modal.error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Create").strong()).clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            }
        });
    if !open || cancel {
        app.new_workspace_modal = None;
    } else if let Some(current) = browse {
        let start = std::path::PathBuf::from(if current.is_empty() {
            std::env::var("HOME").unwrap_or_default()
        } else {
            current
        });
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Choose worktree parent folder")
            .set_directory(start)
            .pick_folder()
            && let Some(modal) = app.new_workspace_modal.as_mut()
        {
            modal.custom_path = p.to_string_lossy().to_string();
            modal.mode = state::LocationMode::Custom;
        }
    } else if create {
        app.create_workspace_from_modal(ctx);
    }
}

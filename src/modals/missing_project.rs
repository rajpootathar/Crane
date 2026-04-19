//! "Project Not Found" modal. Triggered on session restore when a
//! Project's root folder no longer exists on disk. Offers two options:
//!
//! - Open from new location…  → native folder picker, updates
//!   `project.path`, clears the `missing` flag, refreshes git status.
//! - Close                     → dismisses this entry. The project
//!   stays in the tree (greyed out, all actions no-op) so the user
//!   can relocate later from the right-click menu.

use crate::state::{App, Project};

pub fn render(ctx: &egui::Context, app: &mut App) {
    let Some(&pid) = app.missing_project_modals.first() else {
        return;
    };
    let project_info: Option<(String, std::path::PathBuf)> = app
        .projects
        .iter()
        .find(|p| p.id == pid)
        .map(|p| (p.name.clone(), p.path.clone()));
    let Some((name, path)) = project_info else {
        // Project removed out from under us — dequeue silently.
        app.missing_project_modals.remove(0);
        return;
    };

    let mut relocate = false;
    let mut close = false;
    egui::Window::new("Project Not Found")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size(egui::vec2(480.0, 180.0))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("“{name}” was opened from:"))
                    .size(12.5),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(path.display().to_string())
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::from_rgb(150, 160, 180)),
                )
                .truncate(),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "The folder wasn't found. You can point Crane at its new location, \
                     or close this and decide later — the project will stay in the sidebar \
                     greyed out until you relocate or remove it.",
                )
                .size(11.5),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button(egui::RichText::new("Open from new location…").strong())
                    .clicked()
                {
                    relocate = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });

    if relocate {
        let start = path.parent().unwrap_or(&path).to_path_buf();
        if let Some(picked) = rfd::FileDialog::new()
            .set_title("Pick the project folder")
            .set_directory(start)
            .pick_folder()
            && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
        {
            p.path = picked;
            p.missing = false;
            // New name from the folder's basename, so the tree label
            // reflects the relocation rather than the stale name.
            if let Some(n) = p.path.file_name().and_then(|s| s.to_str()) {
                p.name = n.to_string();
            }
        }
        app.missing_project_modals.remove(0);
    } else if close {
        app.missing_project_modals.remove(0);
    }
    // Silence the unused-var warning when Project fields are added
    // later — referencing the type here is cheap.
    let _ = std::marker::PhantomData::<Project>;
}

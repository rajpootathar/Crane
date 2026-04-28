use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    let Some(pending) = app.pending_remove_worktree.as_ref() else {
        return;
    };
    let label = pending.label.clone();
    let path = pending.path.clone();
    let project_id = pending.project_id;
    let workspace_id = pending.workspace_id;
    let unpushed = pending.unpushed_commits;
    let modified = pending.modified_files;
    let has_upstream = pending.has_upstream;
    let is_main = pending.is_main;
    let has_unsaved = unpushed > 0 || modified > 0;

    let mut cancel = false;
    let mut confirm = false;

    let title = if has_unsaved {
        "Worktree has unsaved work"
    } else {
        "Remove worktree"
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.add_space(4.0);
            ui.label(format!("Remove worktree \"{label}\"?"));
            ui.add_space(8.0);
            if unpushed > 0 {
                let suffix = if has_upstream { "not pushed to upstream" } else { "ahead of main (no upstream set)" };
                ui.label(format!("• {unpushed} commit(s) {suffix}"));
            }
            if modified > 0 {
                ui.label(format!("• {modified} file(s) modified or untracked"));
            }
            if has_unsaved {
                ui.add_space(6.0);
            }
            if is_main {
                ui.label("This is the project's main checkout. Crane will close it but the directory on disk stays.");
            } else if has_unsaved {
                ui.label("Removing will run `git worktree remove --force` and delete the directory. Local work will be lost.");
            } else {
                ui.label("Removing will run `git worktree remove` and delete the directory on disk.");
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                let confirm_label = if has_unsaved { "Remove anyway" } else { "Remove" };
                if ui.button(confirm_label).clicked() {
                    confirm = true;
                }
            });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }

    if cancel {
        app.pending_remove_worktree = None;
        return;
    }
    if !confirm {
        return;
    }

    if !is_main {
        let repo = app
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.path.clone());
        if let Some(repo) = repo {
            let _ = crate::git::workspace_remove(&repo, &path);
        }
    }
    if let Some(p) = app.projects.iter_mut().find(|p| p.id == project_id) {
        p.workspaces.retain(|w| w.id != workspace_id);
    }
    if app.active.map(|(_, w, _)| w == workspace_id).unwrap_or(false) {
        app.active = None;
    }
    app.pending_remove_worktree = None;
}

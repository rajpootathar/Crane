use crate::state::{App, FILE_OP_HISTORY_CAP, FileOp};

pub fn render(ctx: &egui::Context, app: &mut App) {
    let Some(pending) = app.pending_delete_file.as_ref() else {
        return;
    };
    let path = pending.path.clone();
    let is_dir = path.is_dir();
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("(unnamed)")
        .to_string();

    let mut cancel = false;
    let mut confirm = false;

    let title = if is_dir {
        "Move folder to Trash"
    } else {
        "Move file to Trash"
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.add_space(4.0);
            ui.label(format!("Move \"{name}\" to the Trash?"));
            ui.add_space(8.0);
            if is_dir {
                ui.label("This will move the folder and everything inside it to the Trash.");
            } else {
                ui.label("This will move the file to the Trash. Restore it from the Trash, or undo with Cmd+Z.");
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button("Move to Trash").clicked() {
                    confirm = true;
                }
            });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }

    if cancel {
        app.pending_delete_file = None;
        return;
    }
    if !confirm {
        return;
    }

    if let Err(e) = trash::delete(&path) {
        app.git_error = Some(format!("Trash: {e}"));
        app.pending_delete_file = None;
        return;
    }
    if app.selected_file.as_deref() == Some(path.as_path()) {
        app.selected_file = None;
    }
    app.close_file_tabs_for_path(&path);
    if app.file_op_history.len() >= FILE_OP_HISTORY_CAP {
        app.file_op_history.pop_front();
    }
    app.file_op_history.push_back(FileOp::Trash { path });
    app.pending_delete_file = None;
}

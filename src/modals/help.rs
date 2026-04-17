use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.show_help {
        return;
    }
    let mut open = true;
    egui::Window::new("Keyboard Shortcuts")
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let rows: &[(&str, &str)] = &[
                ("Cmd+T", "Split active Pane with new terminal"),
                ("Cmd+Shift+T", "New Tab in active Workspace"),
                ("Cmd+D", "Split Pane horizontally (side-by-side)"),
                ("Cmd+Shift+D", "Split Pane vertically (stacked)"),
                ("Cmd+W", "Close focused Pane"),
                ("Cmd+Shift+W", "Close active Tab"),
                ("Cmd+[ / Cmd+]", "Focus prev / next Pane"),
                ("Cmd+B", "Toggle Left Panel"),
                ("Cmd+/", "Toggle Right Panel"),
                ("Cmd+= / Cmd+-", "Increase / decrease font size"),
                ("Cmd+0", "Reset font size"),
                ("Ctrl+C / Ctrl+D", "Terminal: interrupt / EOF"),
            ];
            egui::Grid::new("shortcuts_grid")
                .num_columns(2)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for (key, desc) in rows {
                        ui.label(egui::RichText::new(*key).monospace().strong());
                        ui.label(*desc);
                        ui.end_row();
                    }
                });
        });
    if !open {
        app.show_help = false;
    }
}

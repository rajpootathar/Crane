use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.show_help {
        return;
    }
    let mut close_clicked = false;
    let modal = egui::Modal::new(egui::Id::new("help_modal")).show(ctx, |ui| {
        ui.set_min_width(420.0);
        ui.horizontal(|ui| {
            ui.heading("Keyboard Shortcuts");
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(egui_phosphor::regular::X).size(13.0),
                            )
                            .frame(false)
                            .min_size(egui::vec2(22.0, 22.0)),
                        )
                        .on_hover_text("Close")
                        .clicked()
                    {
                        close_clicked = true;
                    }
                },
            );
        });
        ui.separator();
        ui.add_space(6.0);
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
            ("F2 / Cmd+R", "Rename active Tab"),
            ("Cmd+`", "Tab switcher — next (release Cmd to commit)"),
            ("Cmd+~", "Tab switcher — previous"),
            ("Cmd+K", "Terminal: clear screen + scrollback"),
            ("Shift+Tab", "Terminal: back-tab (CSI Z) for TUIs"),
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
    if modal.should_close() || close_clicked {
        app.show_help = false;
    }
}

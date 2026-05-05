//! Confirm modal for quitting Crane (Cmd+Q / window close).
//!
//! The OS's close request would tear down every running terminal,
//! editor and unsaved buffer with no warning. The render path in
//! `main.rs` cancels the close, sets `app.pending_quit_modal`, and
//! we ask first.

use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.pending_quit_modal {
        return;
    }
    // Count running terminals so the user sees the cost up-front.
    let running = count_running_terminals(app);

    let mut cancel = false;
    let mut confirm = false;
    egui::Window::new("Quit Crane")
        .id(egui::Id::new("confirm_quit_modal"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.add_space(4.0);
            ui.label("Quit Crane?");
            ui.add_space(4.0);
            let body = if running > 0 {
                format!(
                    "{running} running terminal process{} will be killed.",
                    if running == 1 { "" } else { "es" }
                )
            } else {
                "All open panes will close.".to_string()
            };
            ui.label(
                egui::RichText::new(body)
                    .size(11.5)
                    .color(crate::theme::current().text_muted.to_color32()),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button("Quit").clicked() {
                    confirm = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        app.pending_quit_modal = false;
        app.confirmed_quit = false;
        return;
    }
    if confirm {
        app.pending_quit_modal = false;
        app.confirmed_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn count_running_terminals(app: &App) -> usize {
    let mut n = 0;
    for project in &app.projects {
        for ws in &project.workspaces {
            for tab in &ws.tabs {
                for pane in tab.layout.panes.values() {
                    if let crate::state::layout::PaneContent::Terminal(tp) = &pane.content {
                        n += tp
                            .tabs
                            .iter()
                            .filter(|t| t.terminal.has_foreground_process())
                            .count();
                    }
                }
            }
        }
    }
    n
}

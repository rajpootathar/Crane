//! Confirm modal for closing a workspace tab from the projects pane.
//!
//! Fires on × button or middle-click. The tab owns a whole Layout
//! (panes, terminals, open files) — losing it by a stray click is
//! expensive, so we always ask first.

use crate::state::App;

pub fn render(ctx: &egui::Context, app: &mut App) {
    let Some((pid, wid, tid)) = app.pending_close_tab else {
        return;
    };
    let tab_name = app
        .projects
        .iter()
        .find(|p| p.id == pid)
        .and_then(|p| p.workspaces.iter().find(|w| w.id == wid))
        .and_then(|w| w.tabs.iter().find(|t| t.id == tid))
        .map(|t| t.name.clone())
        .unwrap_or_default();

    let mut cancel = false;
    let mut confirm = false;
    egui::Window::new("Close tab")
        .id(egui::Id::new(("confirm_close_tab", pid, wid, tid)))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.add_space(4.0);
            ui.label(format!(
                "Close tab \"{tab_name}\"?"
            ));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Any running processes in this tab's terminals will be killed.",
                )
                .size(11.5)
                .color(crate::theme::current().text_muted.to_color32()),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button("Close tab").clicked() {
                    confirm = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        app.pending_close_tab = None;
        return;
    }
    if !confirm {
        return;
    }

    if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
        && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid)
    {
        w.tabs.retain(|t| t.id != tid);
        w.active_tab = w.tabs.first().map(|t| t.id);
        if app.active.map(|(_, _, t)| t == tid).unwrap_or(false) {
            app.active = w.active_tab.map(|nt| (pid, wid, nt));
        }
        app.last_workspace = Some((pid, wid));
    }
    app.pending_close_tab = None;
}

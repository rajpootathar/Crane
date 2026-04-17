use crate::state::App;
use egui::{Color32, RichText};

pub const WIDTH: f32 = 220.0;

const HEADER: Color32 = Color32::from_rgb(180, 184, 196);
const DIM: Color32 = Color32::from_rgb(130, 136, 150);
const ACTIVE: Color32 = Color32::from_rgb(100, 140, 220);
const ADD: Color32 = Color32::from_rgb(140, 220, 150);
const DEL: Color32 = Color32::from_rgb(230, 130, 130);

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("PROJECTS").size(11.0).color(HEADER).strong());
    });
    ui.add_space(4.0);

    let mut set_active: Option<(u64, u64, u64)> = None;
    let mut toggle_project: Option<u64> = None;
    let mut toggle_worktree: Option<(u64, u64)> = None;
    let mut close_tab: Option<(u64, u64, u64)> = None;
    let mut new_tab_for_worktree: Option<(u64, u64)> = None;

    egui::ScrollArea::vertical()
        .id_salt("left_projects")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for project in &app.projects {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    let arrow = if project.expanded { "▾" } else { "▸" };
                    if ui
                        .small_button(format!("{arrow} {}", project.name))
                        .clicked()
                    {
                        toggle_project = Some(project.id);
                    }
                });
                if project.expanded {
                    for wt in &project.worktrees {
                        ui.push_id(("wt_row", wt.id), |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                let arrow = if wt.expanded { "▾" } else { "▸" };
                                let active_wt =
                                    app.active.map(|(_, w, _)| w == wt.id).unwrap_or(false);
                                let color = if active_wt { ACTIVE } else { HEADER };
                                let label = RichText::new(format!("{arrow} {}", wt.name))
                                    .color(color);
                                if ui.small_button(label).clicked() {
                                    toggle_worktree = Some((project.id, wt.id));
                                }
                                if let Some(status) = &wt.git_status {
                                    if status.added > 0 || status.deleted > 0 {
                                        ui.add_space(4.0);
                                        ui.label(
                                            RichText::new(format!("+{}", status.added))
                                                .color(ADD)
                                                .size(10.5),
                                        );
                                        ui.label(
                                            RichText::new(format!("-{}", status.deleted))
                                                .color(DEL)
                                                .size(10.5),
                                        );
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button(RichText::new("+").size(11.0))
                                            .on_hover_text("New tab in this worktree (⌘⇧T)")
                                            .clicked()
                                        {
                                            new_tab_for_worktree = Some((project.id, wt.id));
                                        }
                                    },
                                );
                            });
                        });
                        if wt.expanded {
                            for tab in &wt.tabs {
                                ui.push_id(("tab_row", wt.id, tab.id), |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_space(32.0);
                                        let active = app
                                            .active
                                            .map(|(_, w, t)| w == wt.id && t == tab.id)
                                            .unwrap_or(false);
                                        let color = if active { ACTIVE } else { DIM };
                                        if ui
                                            .small_button(
                                                RichText::new(format!("◦ {}", tab.name))
                                                    .color(color),
                                            )
                                            .clicked()
                                        {
                                            set_active = Some((project.id, wt.id, tab.id));
                                        }
                                        if ui
                                            .small_button(
                                                RichText::new("×").color(DIM).size(11.0),
                                            )
                                            .on_hover_text("Close tab")
                                            .clicked()
                                        {
                                            close_tab = Some((project.id, wt.id, tab.id));
                                        }
                                    });
                                });
                            }
                        }
                    }
                }
                ui.add_space(2.0);
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("ADD PROJECT").size(10.5).color(DIM).strong());
            });
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut app.add_project_buf)
                        .hint_text("path…")
                        .desired_width(WIDTH - 96.0),
                );
                if ui.small_button("Add").clicked() {
                    let p = std::path::PathBuf::from(app.add_project_buf.trim());
                    app.add_project_buf.clear();
                    app.add_project_from_path(p, ctx);
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                if ui.small_button("📁 Browse…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose project folder")
                        .pick_folder()
                    {
                        app.add_project_from_path(path, ctx);
                    }
                }
            });
        });

    if let Some(pid) = toggle_project {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            p.expanded = !p.expanded;
        }
    }
    if let Some((pid, wid)) = toggle_worktree {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.worktrees.iter_mut().find(|w| w.id == wid) {
                w.expanded = !w.expanded;
                if let Some(tid) = w.active_tab {
                    app.active = Some((pid, wid, tid));
                }
            }
        }
    }
    if let Some((pid, wid, tid)) = set_active {
        app.set_active(pid, wid, tid);
    }
    if let Some((pid, wid)) = new_tab_for_worktree {
        app.active = app.active.map(|(_, _, t)| (pid, wid, t)).or(Some((pid, wid, 0)));
        app.last_worktree = Some((pid, wid));
        app.new_tab_in_active_worktree(ctx);
    }
    if let Some((pid, wid, tid)) = close_tab {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.worktrees.iter_mut().find(|w| w.id == wid) {
                w.tabs.retain(|t| t.id != tid);
                w.active_tab = w.tabs.first().map(|t| t.id);
                if app.active.map(|(_, _, t)| t == tid).unwrap_or(false) {
                    app.active = w.active_tab.map(|nt| (pid, wid, nt));
                }
                app.last_worktree = Some((pid, wid));
            }
        }
    }
}

use crate::state::App;
use crate::ui_util::{
    draw_row, draw_trailing, full_width_primary_button, RowConfig, ACCENT,
};
use egui::{Color32, Pos2, Rect, RichText, Stroke};
use egui_phosphor::regular as icons;

pub const WIDTH: f32 = 240.0;

const HEADER: Color32 = Color32::from_rgb(140, 146, 162);
const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let full = ui.available_rect_before_wrap();
    let footer_h = 44.0;
    let scroll_rect = Rect::from_min_max(full.min, Pos2::new(full.max.x, full.max.y - footer_h));
    let footer_rect = Rect::from_min_max(Pos2::new(full.min.x, full.max.y - footer_h), full.max);

    let mut scroll_ui = ui.new_child(egui::UiBuilder::new().max_rect(scroll_rect));
    scroll_ui.set_clip_rect(scroll_rect);
    render_tree(&mut scroll_ui, app, ctx);

    let mut footer_ui = ui.new_child(egui::UiBuilder::new().max_rect(footer_rect));
    footer_ui.set_clip_rect(footer_rect);
    footer_ui.painter().line_segment(
        [
            Pos2::new(footer_rect.min.x, footer_rect.min.y),
            Pos2::new(footer_rect.max.x, footer_rect.min.y),
        ],
        Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
    );
    footer_ui.add_space(8.0);
    footer_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.scope(|ui| {
            if full_width_primary_button(
                ui,
                Some(icons::FOLDER_PLUS),
                "Add Project…",
                "Choose a folder",
            )
            .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose project folder")
                    .pick_folder()
                {
                    app.add_project_from_path(path, ctx);
                }
            }
        });
        ui.add_space(8.0);
    });
}

fn render_tree(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let _ = ctx;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("PROJECTS")
                .size(10.5)
                .color(HEADER)
                .strong(),
        );
    });
    ui.add_space(4.0);

    let mut set_active: Option<(u64, u64, u64)> = None;
    let mut toggle_project: Option<u64> = None;
    let mut toggle_worktree: Option<(u64, u64)> = None;
    let mut close_tab: Option<(u64, u64, u64)> = None;
    let mut new_tab_for_worktree: Option<(u64, u64)> = None;
    let mut new_workspace_for_project: Option<u64> = None;
    let mut remove_project: Option<u64> = None;

    egui::ScrollArea::vertical()
        .id_salt("left_projects")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for project in &app.projects {
                let row = draw_row(
                    ui,
                    RowConfig {
                        depth: 0,
                        expanded: Some(project.expanded),
                        leading: Some(icons::CUBE),
                        leading_color: Some(ACCENT),
                        label: &project.name,
                        label_color: None,
                        is_active: false,
                        active_bar: false,
                        badge: None,
                        trailing_count: 2,
                    },
                );
                let project_trailing = draw_trailing(
                    ui,
                    row.rect,
                    row.hovered,
                    &[
                        (icons::PLUS, "New worktree", 0),
                        (icons::X, "Remove project", 1),
                    ],
                );
                if project_trailing[0] {
                    new_workspace_for_project = Some(project.id);
                } else if project_trailing[1] {
                    remove_project = Some(project.id);
                } else if row.main_clicked {
                    toggle_project = Some(project.id);
                }

                if project.expanded {
                    for wt in &project.workspaces {
                        let active_wt = app.active.map(|(_, w, _)| w == wt.id).unwrap_or(false);
                        let badge = wt.git_status.as_ref().and_then(|s| {
                            if s.added > 0 || s.deleted > 0 {
                                Some((s.added, s.deleted, ADD, DEL))
                            } else {
                                None
                            }
                        });
                        let wt_row = draw_row(
                            ui,
                            RowConfig {
                                depth: 1,
                                expanded: Some(wt.expanded),
                                leading: Some(icons::GIT_BRANCH),
                                leading_color: if active_wt { Some(ACCENT) } else { None },
                                label: &wt.name,
                                label_color: None,
                                is_active: active_wt,
                                active_bar: active_wt,
                                badge,
                                trailing_count: 1,
                            },
                        );
                        let wt_trailing = draw_trailing(
                            ui,
                            wt_row.rect,
                            wt_row.hovered,
                            &[(icons::PLUS, "New tab", 0)],
                        );
                        if wt_trailing[0] {
                            new_tab_for_worktree = Some((project.id, wt.id));
                        } else if wt_row.main_clicked {
                            toggle_worktree = Some((project.id, wt.id));
                        }

                        if wt.expanded {
                            for tab in &wt.tabs {
                                let is_active = app
                                    .active
                                    .map(|(_, w, t)| w == wt.id && t == tab.id)
                                    .unwrap_or(false);
                                let tab_row = draw_row(
                                    ui,
                                    RowConfig {
                                        depth: 2,
                                        expanded: None,
                                        leading: Some(icons::TERMINAL_WINDOW),
                                        leading_color: if is_active { Some(ACCENT) } else { None },
                                        label: &tab.name,
                                        label_color: None,
                                        is_active,
                                        active_bar: is_active,
                                        badge: None,
                                        trailing_count: 1,
                                    },
                                );
                                let tab_trailing = draw_trailing(
                                    ui,
                                    tab_row.rect,
                                    tab_row.hovered,
                                    &[(icons::X, "Close tab", 0)],
                                );
                                if tab_trailing[0] {
                                    close_tab = Some((project.id, wt.id, tab.id));
                                } else if tab_row.main_clicked {
                                    set_active = Some((project.id, wt.id, tab.id));
                                }
                            }
                        }
                    }
                }
            }
        });


    if let Some(pid) = toggle_project {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            p.expanded = !p.expanded;
        }
    }
    if let Some((pid, wid)) = toggle_worktree {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
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
        app.last_workspace = Some((pid, wid));
        app.new_tab_in_active_workspace(ctx);
    }
    if let Some(pid) = new_workspace_for_project {
        app.open_new_workspace_modal(pid);
    }
    if let Some(pid) = remove_project {
        app.remove_project(pid);
    }
    if let Some((pid, wid, tid)) = close_tab {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
                w.tabs.retain(|t| t.id != tid);
                w.active_tab = w.tabs.first().map(|t| t.id);
                if app.active.map(|(_, _, t)| t == tid).unwrap_or(false) {
                    app.active = w.active_tab.map(|nt| (pid, wid, nt));
                }
                app.last_workspace = Some((pid, wid));
            }
        }
    }
}


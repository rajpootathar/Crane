use crate::state::App;
use crate::ui::util::{
    accent, draw_row, draw_trailing, full_width_primary_button, RowConfig,
};
use egui::{Color32, Pos2, Rect, RichText, Stroke};
use egui_phosphor::regular as icons;


const HEADER: Color32 = Color32::from_rgb(140, 146, 162);
const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);

fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

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
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose project folder")
                    .pick_folder()
                {
                    app.add_project_from_path(path, ctx);
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
    let mut remove_worktree: Option<(u64, u64)> = None;

    // Snapshot rename state into local buffers so the tree walk only
    // needs an immutable borrow of `app`. Buffers are flushed back into
    // `app.renaming_tab` / `app.renaming_workspace` after the walk.
    let renaming_ref: Option<(u64, u64, u64, String)> =
        app.renaming_tab.as_ref().map(|(p, w, t, b)| (*p, *w, *t, b.clone()));
    let mut rename_buffer: Option<String> =
        renaming_ref.as_ref().map(|(_, _, _, b)| b.clone());
    let mut start_rename: Option<(u64, u64, u64, String)> = None;
    let mut commit_rename: Option<(u64, u64, u64, String)> = None;
    let mut cancel_rename = false;
    let mut rename_focused = false;

    let renaming_wt_ref: Option<(u64, u64, String)> =
        app.renaming_workspace.as_ref().map(|(p, w, b)| (*p, *w, b.clone()));
    let mut rename_wt_buffer: Option<String> =
        renaming_wt_ref.as_ref().map(|(_, _, b)| b.clone());
    let mut start_rename_wt: Option<(u64, u64, String)> = None;
    let mut commit_rename_wt: Option<(u64, u64, String)> = None;
    let mut cancel_rename_wt = false;
    let mut rename_wt_focused = false;

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
                        leading_color: Some(accent()),
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
                let pid = project.id;
                let proj_path = project.path.clone();
                row.response.context_menu(|ui| {
                    if ui.button(format!("{}  Reveal in File Manager", icons::FOLDER_OPEN)).clicked() {
                        reveal_in_file_manager(&proj_path);
                        ui.close();
                    }
                    if ui.button(format!("{}  Copy Path", icons::COPY)).clicked() {
                        ui.ctx().copy_text(proj_path.to_string_lossy().to_string());
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(format!("{}  Remove Project", icons::X)).clicked() {
                        remove_project = Some(pid);
                        ui.close();
                    }
                });

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
                        let wt_renaming = renaming_wt_ref
                            .as_ref()
                            .map(|(p, w, _)| *p == project.id && *w == wt.id)
                            .unwrap_or(false);
                        if wt_renaming {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 26.0),
                                egui::Sense::hover(),
                            );
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect.shrink2(egui::vec2(32.0, 2.0))),
                            );
                            let buf = rename_wt_buffer
                                .as_mut()
                                .expect("wt rename buffer matches renaming_wt_ref");
                            let te_id = egui::Id::new(("rename_wt", wt.id));
                            let resp = child.add(
                                egui::TextEdit::singleline(buf)
                                    .id(te_id)
                                    .hint_text(&wt.name)
                                    .desired_width(f32::INFINITY),
                            );
                            if !ui.memory(|m| m.has_focus(te_id)) && !rename_wt_focused {
                                resp.request_focus();
                                rename_wt_focused = true;
                            }
                            let is_focused = ui.memory(|m| m.has_focus(te_id));
                            let enter = is_focused
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let esc = is_focused
                                && ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if enter {
                                commit_rename_wt =
                                    Some((project.id, wt.id, buf.clone()));
                            } else if esc || resp.lost_focus() {
                                cancel_rename_wt = true;
                            }
                            continue;
                        }
                        let wt_label = wt.label();
                        let wt_row = draw_row(
                            ui,
                            RowConfig {
                                depth: 1,
                                expanded: Some(wt.expanded),
                                leading: Some(icons::GIT_BRANCH),
                                leading_color: if active_wt { Some(accent()) } else { None },
                                label: &wt_label,
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
                        } else if wt_row.response.double_clicked() {
                            start_rename_wt = Some((
                                project.id,
                                wt.id,
                                wt.display_name.clone().unwrap_or_default(),
                            ));
                        } else if wt_row.main_clicked {
                            toggle_worktree = Some((project.id, wt.id));
                        }
                        let wt_pid = project.id;
                        let wt_id = wt.id;
                        let wt_path = wt.path.clone();
                        let wt_display_seed = wt.display_name.clone().unwrap_or_default();
                        wt_row.response.context_menu(|ui| {
                            if ui.button(format!("{}  Rename", icons::PENCIL_SIMPLE)).clicked() {
                                start_rename_wt = Some((wt_pid, wt_id, wt_display_seed.clone()));
                                ui.close();
                            }
                            if ui.button(format!("{}  Reveal in File Manager", icons::FOLDER_OPEN)).clicked() {
                                reveal_in_file_manager(&wt_path);
                                ui.close();
                            }
                            if ui.button(format!("{}  Copy Path", icons::COPY)).clicked() {
                                ui.ctx().copy_text(wt_path.to_string_lossy().to_string());
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(format!("{}  Remove Worktree", icons::X)).clicked() {
                                remove_worktree = Some((wt_pid, wt_id));
                                ui.close();
                            }
                        });

                        if wt.expanded {
                            for tab in &wt.tabs {
                                let is_active = app
                                    .active
                                    .map(|(_, w, t)| w == wt.id && t == tab.id)
                                    .unwrap_or(false);
                                let is_renaming = renaming_ref
                                    .as_ref()
                                    .map(|(p, w, t, _)| *p == project.id && *w == wt.id && *t == tab.id)
                                    .unwrap_or(false);
                                if is_renaming {
                                    // Render the row as an inline TextEdit
                                    // bound to the rename buffer. Caller
                                    // applies commit/cancel after the
                                    // tree walk to avoid double borrows.
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 26.0),
                                        egui::Sense::hover(),
                                    );
                                    let mut child = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(rect.shrink2(egui::vec2(32.0, 2.0))),
                                    );
                                    let buf = rename_buffer
                                        .as_mut()
                                        .expect("rename buffer matches renaming_ref");
                                    let te_id = egui::Id::new(("rename_tab", tab.id));
                                    let resp = child.add(
                                        egui::TextEdit::singleline(buf)
                                            .id(te_id)
                                            .desired_width(f32::INFINITY),
                                    );
                                    if !ui.memory(|m| m.has_focus(te_id)) && !rename_focused {
                                        resp.request_focus();
                                        rename_focused = true;
                                    }
                                    // Detect Enter while the TextEdit is
                                    // focused — `resp.lost_focus()` fires
                                    // the frame AFTER the key, by which
                                    // time the Enter event has drained.
                                    let is_focused = ui.memory(|m| m.has_focus(te_id));
                                    let enter = is_focused
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    let esc = is_focused
                                        && ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if enter {
                                        commit_rename =
                                            Some((project.id, wt.id, tab.id, buf.clone()));
                                    } else if esc {
                                        cancel_rename = true;
                                    } else if resp.lost_focus() {
                                        // Clicked away without Enter —
                                        // cancel (preserves the current
                                        // tab name).
                                        cancel_rename = true;
                                    }
                                    continue;
                                }
                                let tab_row = draw_row(
                                    ui,
                                    RowConfig {
                                        depth: 2,
                                        expanded: None,
                                        leading: Some(icons::TERMINAL_WINDOW),
                                        leading_color: if is_active { Some(accent()) } else { None },
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
                                } else if tab_row.response.double_clicked() {
                                    start_rename = Some((project.id, wt.id, tab.id, tab.name.clone()));
                                } else if tab_row.main_clicked {
                                    set_active = Some((project.id, wt.id, tab.id));
                                }
                                // F2 / Cmd+R on the active tab starts
                                // rename. Scoped to the active tab so
                                // the keys only affect the tab you're
                                // working on. Cmd+R is a macOS-friendly
                                // alias — F2 feels foreign on a Mac
                                // keyboard.
                                let rename_chord = ctx.input(|i| {
                                    i.key_pressed(egui::Key::F2)
                                        || ((i.modifiers.mac_cmd || i.modifiers.command)
                                            && !i.modifiers.shift
                                            && i.key_pressed(egui::Key::R))
                                });
                                if is_active && rename_chord && app.renaming_tab.is_none() {
                                    start_rename =
                                        Some((project.id, wt.id, tab.id, tab.name.clone()));
                                }
                                tab_row.response.context_menu(|ui| {
                                    if ui.button(format!("{}  Rename", icons::PENCIL_SIMPLE)).clicked() {
                                        start_rename = Some((
                                            project.id,
                                            wt.id,
                                            tab.id,
                                            tab.name.clone(),
                                        ));
                                        ui.close();
                                    }
                                });
                            }
                        }
                    }
                }
            }
        });


    // Flush tab rename edits back into App.
    if let (Some(buf), Some(slot)) = (rename_buffer.as_ref(), app.renaming_tab.as_mut()) {
        slot.3 = buf.clone();
    }
    if let Some((pid, wid, tid, new_name)) = commit_rename {
        let trimmed = new_name.trim().to_string();
        if !trimmed.is_empty()
            && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
            && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid)
            && let Some(t) = w.tabs.iter_mut().find(|t| t.id == tid)
        {
            t.name = trimmed;
        }
        app.renaming_tab = None;
    } else if cancel_rename {
        app.renaming_tab = None;
    } else if let Some(start) = start_rename {
        app.renaming_tab = Some(start);
    }

    // Flush workspace rename edits back into App. Empty trimmed input
    // clears the alias (reverting to folder / branch name only).
    if let (Some(buf), Some(slot)) =
        (rename_wt_buffer.as_ref(), app.renaming_workspace.as_mut())
    {
        slot.2 = buf.clone();
    }
    if let Some((pid, wid, new_alias)) = commit_rename_wt {
        let trimmed = new_alias.trim().to_string();
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
            && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid)
        {
            w.display_name = if trimmed.is_empty() { None } else { Some(trimmed) };
        }
        app.renaming_workspace = None;
    } else if cancel_rename_wt {
        app.renaming_workspace = None;
    } else if let Some(start) = start_rename_wt {
        app.renaming_workspace = Some(start);
    }

    if let Some(pid) = toggle_project
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            p.expanded = !p.expanded;
        }
    if let Some((pid, wid)) = toggle_worktree
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
            && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
                w.expanded = !w.expanded;
                if let Some(tid) = w.active_tab {
                    app.active = Some((pid, wid, tid));
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
    if let Some((pid, wid)) = remove_worktree
        && let Some(p) = app.projects.iter().find(|p| p.id == pid)
    {
        // If the worktree has unpushed commits or modified files, stage
        // a confirmation modal instead of removing immediately — the
        // `--force` path below would otherwise discard local work
        // silently. Main checkout (path == project path) always goes
        // through the plain in-memory removal because we never call
        // `git worktree remove` on it.
        let repo = p.path.clone();
        let ws = p.workspaces.iter().find(|w| w.id == wid);
        let is_main = ws.map(|w| w.path == repo).unwrap_or(true);
        let dirty = ws
            .filter(|_| !is_main)
            .map(|w| crate::git::worktree_dirty(&w.path))
            .unwrap_or_default();
        let needs_confirm = dirty.unpushed_commits > 0 || dirty.modified_files > 0;

        if needs_confirm
            && let Some(w) = ws
        {
            app.pending_remove_worktree = Some(crate::state::PendingRemoveWorktree {
                project_id: pid,
                workspace_id: wid,
                label: w.label(),
                path: w.path.clone(),
                unpushed_commits: dirty.unpushed_commits,
                modified_files: dirty.modified_files,
                has_upstream: dirty.has_upstream,
            });
        } else {
            if !is_main
                && let Some(w) = ws
            {
                let _ = crate::git::workspace_remove(&repo, &w.path);
            }
            if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
                p.workspaces.retain(|w| w.id != wid);
            }
            if app.active.map(|(_, w, _)| w == wid).unwrap_or(false) {
                app.active = None;
            }
        }
    }
    if let Some((pid, wid, tid)) = close_tab
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
            && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
                w.tabs.retain(|t| t.id != tid);
                w.active_tab = w.tabs.first().map(|t| t.id);
                if app.active.map(|(_, _, t)| t == tid).unwrap_or(false) {
                    app.active = w.active_tab.map(|nt| (pid, wid, nt));
                }
                app.last_workspace = Some((pid, wid));
            }
}


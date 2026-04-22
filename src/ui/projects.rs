use crate::state::App;
use crate::ui::util::{
    accent, draw_row, draw_trailing, full_width_primary_button, muted, RowConfig,
};
use egui::{Color32, Pos2, Rect, RichText, Stroke};
use egui_phosphor::regular as icons;


const HEADER: Color32 = Color32::from_rgb(140, 146, 162);
const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);

/// Project-tint palette for the right-click "Highlight color" picker.
/// Hand-picked to stay legible on both light and dark themes — full
/// saturation but moderate lightness so the label stays readable when
/// the project name inherits the tint.
const PROJECT_TINT_PALETTE: &[(&str, [u8; 3])] = &[
    ("Red",    [239,  83,  80]),
    ("Orange", [255, 152,   0]),
    ("Yellow", [255, 202,  40]),
    ("Green",  [102, 187, 106]),
    ("Teal",   [ 38, 166, 154]),
    ("Blue",   [ 66, 165, 245]),
    ("Purple", [171,  71, 188]),
    ("Pink",   [236,  64, 122]),
];

fn reveal_in_file_manager(path: &std::path::Path) {
    // Resolve symlinks and expand any relative segments so `open`
    // receives a concrete on-disk path. Worktrees can live under
    // symlinked paths (e.g. `/var` → `/private/var` on macOS) and a
    // stale `~` prefix would also silently drop the command here.
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "macos")]
    {
        // `open <dir>` opens the folder in a new Finder window —
        // matches what users expect when they say "reveal". We
        // prefer that over `open -R <dir>` (which highlights the
        // folder in its parent) since worktrees' parents are usually
        // `~/.crane-worktrees/<project>` which isn't meaningful UI.
        let _ = std::process::Command::new("open").arg(&resolved).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&resolved).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(&resolved).spawn();
    }
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
    // Pending tint change from a context-menu color pick.
    // `Some((id, None))` means "clear tint" (restore accent).
    let mut set_tint: Option<(u64, Option<[u8; 3]>)> = None;
    // Pending "remove entire folder group" from a folder-header
    // context-menu pick.
    let mut remove_group_pending: Option<std::path::PathBuf> = None;
    // Pending tint change for a folder-group header. `None` value
    // clears the tint.
    let mut set_group_tint: Option<(std::path::PathBuf, Option<[u8; 3]>)> = None;
    // Pending tint change for a branch (Workspace).
    let mut set_workspace_tint: Option<(u64, u64, Option<[u8; 3]>)> = None;

    // Precompute group member counts so individual-project "Remove"
    // can be suppressed when the group has siblings. Atomic unload
    // rule: a multi-member group is removed whole via its folder
    // header; per-member removal would leave the group in a weird
    // half-state the user didn't ask for.
    let mut group_counts: std::collections::HashMap<std::path::PathBuf, usize> =
        std::collections::HashMap::new();
    for p in &app.projects {
        if let Some(gp) = &p.group_path {
            *group_counts.entry(gp.clone()).or_insert(0) += 1;
        }
    }

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
            let mut last_group: Option<std::path::PathBuf> = None;
            for project in &app.projects {
                // Render a group header row whenever we enter a new
                // group. Projects without a group render flush-left as
                // before; grouped Projects nest one level deeper under
                // their shared folder name.
                let in_group = project.group_path.is_some();
                if in_group && project.group_path != last_group {
                    let group_name = project
                        .group_name
                        .clone()
                        .unwrap_or_else(|| "group".into());
                    let group_tint = project
                        .group_path
                        .as_ref()
                        .and_then(|gp| app.group_tints.get(gp).copied())
                        .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
                    let folder_row = draw_row(
                        ui,
                        RowConfig {
                            depth: 0,
                            expanded: Some(true),
                            leading: Some(icons::FOLDER),
                            leading_color: Some(group_tint.unwrap_or_else(muted)),
                            label: &group_name,
                            label_color: Some(group_tint.unwrap_or_else(muted)),
                            is_active: false,
                            active_bar: false,
                            badge: None,
                            trailing_count: 0,
                            tree_guides: false, checkbox: None,
                        },
                    );
                    if let Some(gp) = &project.group_path {
                        let gp = gp.clone();
                        folder_row.response.context_menu(|ui| {
                            ui.label(
                                egui::RichText::new("Highlight color")
                                    .size(11.0)
                                    .color(muted()),
                            );
                            ui.horizontal(|ui| {
                                for (label, rgb) in PROJECT_TINT_PALETTE {
                                    let color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                                    let btn = egui::Button::new(
                                        egui::RichText::new(icons::FOLDER)
                                            .color(color)
                                            .size(14.0),
                                    )
                                    .min_size(egui::vec2(22.0, 22.0))
                                    .frame(false);
                                    if ui.add(btn).on_hover_text(*label).clicked() {
                                        set_group_tint = Some((gp.clone(), Some(*rgb)));
                                        ui.close();
                                    }
                                }
                            });
                            if ui
                                .button(format!(
                                    "{}  Default color",
                                    icons::ARROW_COUNTER_CLOCKWISE
                                ))
                                .clicked()
                            {
                                set_group_tint = Some((gp.clone(), None));
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .button(format!("{}  Remove folder group", icons::X))
                                .clicked()
                            {
                                remove_group_pending = Some(gp.clone());
                                ui.close();
                            }
                        });
                    }
                }
                last_group = project.group_path.clone();
                let project_depth = if in_group { 1 } else { 0 };
                let tint_color = project
                    .tint
                    .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
                let row = draw_row(
                    ui,
                    RowConfig {
                        depth: project_depth,
                        expanded: Some(project.expanded),
                        leading: Some(icons::CUBE),
                        leading_color: Some(tint_color.unwrap_or_else(accent)),
                        label: &project.name,
                        label_color: tint_color,
                        is_active: false,
                        active_bar: false,
                        badge: None,
                        trailing_count: 2,
                        tree_guides: in_group, checkbox: None,
                    },
                );
                // Suppress per-Project removal when the Project is one
                // of several siblings inside a folder group. In that
                // case the group must be removed atomically via the
                // folder header's "Remove folder group" context menu.
                let in_multi_group = project
                    .group_path
                    .as_ref()
                    .and_then(|gp| group_counts.get(gp))
                    .is_some_and(|c| *c > 1);
                let project_trailing = if in_multi_group {
                    draw_trailing(
                        ui,
                        row.rect,
                        row.hovered,
                        &[(icons::PLUS, "New worktree", 0)],
                    )
                } else {
                    draw_trailing(
                        ui,
                        row.rect,
                        row.hovered,
                        &[
                            (icons::PLUS, "New worktree", 0),
                            (icons::X, "Remove project", 1),
                        ],
                    )
                };
                if project_trailing[0] {
                    new_workspace_for_project = Some(project.id);
                } else if !in_multi_group
                    && project_trailing.get(1).copied().unwrap_or(false)
                {
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
                    // Color picker — tints both the cube icon and the
                    // project name. Swatch buttons show the actual color
                    // so the palette is self-documenting. "Default"
                    // clears the tint and falls back to the theme accent.
                    ui.label(egui::RichText::new("Highlight color").size(11.0).color(muted()));
                    ui.horizontal(|ui| {
                        for (label, rgb) in PROJECT_TINT_PALETTE {
                            let color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                            let btn = egui::Button::new(
                                egui::RichText::new(icons::CUBE).color(color).size(14.0),
                            )
                            .min_size(egui::vec2(22.0, 22.0))
                            .frame(false);
                            if ui.add(btn).on_hover_text(*label).clicked() {
                                set_tint = Some((pid, Some(*rgb)));
                                ui.close();
                            }
                        }
                    });
                    if ui.button(format!("{}  Default color", icons::ARROW_COUNTER_CLOCKWISE)).clicked() {
                        set_tint = Some((pid, None));
                        ui.close();
                    }
                    // Only expose individual Remove when this Project
                    // isn't part of a multi-member folder group —
                    // those must be removed atomically via the folder
                    // header to keep groups internally consistent.
                    if !in_multi_group {
                        ui.separator();
                        if ui.button(format!("{}  Remove Project", icons::X)).clicked() {
                            remove_project = Some(pid);
                            ui.close();
                        }
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
                            // Defensive: `wt_renaming` is true iff
                                // `renaming_wt_ref` points at this row, and
                                // `rename_wt_buffer` is set alongside
                                // `renaming_wt_ref`. A state desync here
                                // shouldn't crash the UI — bail out of the
                                // rename for this frame and let the next
                                // frame re-seed, or the click handler
                                // cancel it.
                            let Some(buf) = rename_wt_buffer.as_mut() else {
                                cancel_rename_wt = true;
                                continue;
                            };
                            let te_id = egui::Id::new(("rename_wt", wt.id));
                            let resp = child.add(
                                egui::TextEdit::singleline(buf)
                                    .id(te_id)
                                    .hint_text(&wt.name)
                                    .desired_width(f32::INFINITY),
                            );
                            // Only request focus once per opening of the
                            // rename. A per-frame `request_focus()` steals
                            // focus back from the click that was trying to
                            // leave the field, so `lost_focus()` never
                            // stays true. Gate via egui memory keyed by
                            // the rename id; cleared when rename ends.
                            let focus_done_id =
                                egui::Id::new(("rename_wt_focus_done", wt.id));
                            let focus_done = ui
                                .ctx()
                                .memory(|m| m.data.get_temp::<bool>(focus_done_id))
                                .unwrap_or(false);
                            if !focus_done {
                                resp.request_focus();
                                ui.ctx().memory_mut(|m| {
                                    m.data.insert_temp(focus_done_id, true)
                                });
                            }
                            let _ = rename_wt_focused;
                            rename_wt_focused = true;
                            let is_focused = ui.memory(|m| m.has_focus(te_id));
                            let enter = is_focused
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let esc = is_focused
                                && ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if enter {
                                commit_rename_wt =
                                    Some((project.id, wt.id, buf.clone()));
                            } else if esc {
                                cancel_rename_wt = true;
                            } else if resp.lost_focus() {
                                let trimmed = buf.trim();
                                if trimmed.is_empty() {
                                    cancel_rename_wt = true;
                                } else {
                                    commit_rename_wt =
                                        Some((project.id, wt.id, buf.clone()));
                                }
                            }
                            continue;
                        }
                        let wt_label = wt.label();
                        let wt_depth = if in_group { 2 } else { 1 };
                        let wt_tint_color = wt
                            .tint
                            .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
                        // Tint priority: explicit user tint wins over
                        // the active-branch accent hint. Without this,
                        // the active row always paints the leading
                        // icon in accent and any tint picked is
                        // invisible on the active branch.
                        let wt_leading_color = wt_tint_color.or(if active_wt {
                            Some(accent())
                        } else {
                            None
                        });
                        let wt_row = draw_row(
                            ui,
                            RowConfig {
                                depth: wt_depth,
                                expanded: Some(wt.expanded),
                                leading: Some(icons::GIT_BRANCH),
                                leading_color: wt_leading_color,
                                label: &wt_label,
                                label_color: wt_tint_color,
                                is_active: active_wt,
                                active_bar: active_wt,
                                badge,
                                trailing_count: 1,
                                tree_guides: in_group, checkbox: None,
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
                            ui.label(
                                egui::RichText::new("Highlight color")
                                    .size(11.0)
                                    .color(muted()),
                            );
                            ui.horizontal(|ui| {
                                for (label, rgb) in PROJECT_TINT_PALETTE {
                                    let color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                                    let btn = egui::Button::new(
                                        egui::RichText::new(icons::GIT_BRANCH)
                                            .color(color)
                                            .size(14.0),
                                    )
                                    .min_size(egui::vec2(22.0, 22.0))
                                    .frame(false);
                                    if ui.add(btn).on_hover_text(*label).clicked() {
                                        set_workspace_tint = Some((wt_pid, wt_id, Some(*rgb)));
                                        ui.close();
                                    }
                                }
                            });
                            if ui
                                .button(format!(
                                    "{}  Default color",
                                    icons::ARROW_COUNTER_CLOCKWISE
                                ))
                                .clicked()
                            {
                                set_workspace_tint = Some((wt_pid, wt_id, None));
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
                                    // Same defensive fallback as the
                                    // worktree rename above.
                                    let Some(buf) = rename_buffer.as_mut() else {
                                        cancel_rename = true;
                                        continue;
                                    };
                                    let te_id = egui::Id::new(("rename_tab", tab.id));
                                    let resp = child.add(
                                        egui::TextEdit::singleline(buf)
                                            .id(te_id)
                                            .desired_width(f32::INFINITY),
                                    );
                                    // Same per-frame focus-stealing
                                    // issue as the workspace rename above.
                                    let focus_done_id =
                                        egui::Id::new(("rename_tab_focus_done", tab.id));
                                    let focus_done = ui
                                        .ctx()
                                        .memory(|m| m.data.get_temp::<bool>(focus_done_id))
                                        .unwrap_or(false);
                                    if !focus_done {
                                        resp.request_focus();
                                        ui.ctx().memory_mut(|m| {
                                            m.data.insert_temp(focus_done_id, true)
                                        });
                                    }
                                    let _ = rename_focused;
                                    rename_focused = true;
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
                                        // commit the rename if the buffer
                                        // is non-empty, otherwise cancel.
                                        let trimmed = buf.trim();
                                        if trimmed.is_empty() {
                                            cancel_rename = true;
                                        } else {
                                            commit_rename = Some((
                                                project.id,
                                                wt.id,
                                                tab.id,
                                                buf.clone(),
                                            ));
                                        }
                                    }
                                    continue;
                                }
                                let tab_depth = if in_group { 3 } else { 2 };
                                let tab_row = draw_row(
                                    ui,
                                    RowConfig {
                                        depth: tab_depth,
                                        expanded: None,
                                        leading: Some(icons::TERMINAL_WINDOW),
                                        leading_color: if is_active { Some(accent()) } else { None },
                                        label: &tab.name,
                                        label_color: None,
                                        is_active,
                                        active_bar: is_active,
                                        badge: None,
                                        trailing_count: 1,
                                        tree_guides: in_group, checkbox: None,
                                    },
                                );
                                let tab_trailing = draw_trailing(
                                    ui,
                                    tab_row.rect,
                                    tab_row.hovered,
                                    &[(icons::X, "Close tab", 0)],
                                );
                                // × button or middle-click → confirm-close
                                // instead of immediate removal. Tabs hold
                                // terminals / editors; silent close on a
                                // stray click has cost the user work.
                                let close_requested =
                                    tab_trailing[0] || tab_row.response.middle_clicked();
                                if close_requested {
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
        ctx.memory_mut(|m| {
            m.data.remove::<bool>(egui::Id::new(("rename_tab_focus_done", tid)));
        });
    } else if cancel_rename {
        if let Some((_, _, tid, _)) = &app.renaming_tab {
            let tid = *tid;
            ctx.memory_mut(|m| {
                m.data.remove::<bool>(egui::Id::new(("rename_tab_focus_done", tid)));
            });
        }
        app.renaming_tab = None;
    } else if let Some(start) = start_rename {
        // Clear any stale focus flag so the first frame of the rename
        // triggers resp.request_focus(). Without this, re-opening a
        // rename on the same tab after the flag was left set could
        // skip the initial focus and leave the TextEdit unfocused.
        let tid = start.2;
        ctx.memory_mut(|m| {
            m.data.remove::<bool>(egui::Id::new(("rename_tab_focus_done", tid)));
        });
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
        ctx.memory_mut(|m| {
            m.data.remove::<bool>(egui::Id::new(("rename_wt_focus_done", wid)));
        });
    } else if cancel_rename_wt {
        if let Some((_, wid, _)) = &app.renaming_workspace {
            let wid = *wid;
            ctx.memory_mut(|m| {
                m.data.remove::<bool>(egui::Id::new(("rename_wt_focus_done", wid)));
            });
        }
        app.renaming_workspace = None;
    } else if let Some(start) = start_rename_wt {
        let wid = start.1;
        ctx.memory_mut(|m| {
            m.data.remove::<bool>(egui::Id::new(("rename_wt_focus_done", wid)));
        });
        app.renaming_workspace = Some(start);
    }

    if let Some((pid, tint)) = set_tint
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            p.tint = tint;
        }
    if let Some((group, tint)) = set_group_tint {
        match tint {
            Some(rgb) => {
                app.group_tints.insert(group, rgb);
            }
            None => {
                app.group_tints.remove(&group);
            }
        }
    }
    if let Some((pid, wid, tint)) = set_workspace_tint
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
        && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid)
    {
        w.tint = tint;
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
    if let Some(group) = remove_group_pending {
        app.remove_group(&group);
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
    if let Some(target) = close_tab {
        // Stage the close — the confirm modal handles the actual drop.
        app.pending_close_tab = Some(target);
    }
}


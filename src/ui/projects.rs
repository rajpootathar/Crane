use crate::state::App;
use crate::ui::util::{
    accent, draw_row, draw_trailing, full_width_primary_button, muted, RowConfig,
};
use egui::{Color32, Pos2, Rect, RichText, Stroke};
use egui_phosphor::regular as icons;

/// Payload stored in egui's drag state when a tree row is being
/// dragged.  Identifies the item and its parent scope so drop targets
/// can validate same-level, same-parent scope.
#[derive(Clone)]
enum TreeDrag {
    Project { id: u64 },
    Workspace { project_id: u64, id: u64 },
    Tab { project_id: u64, workspace_id: u64, id: u64 },
}

/// Drop-scope tag for a single row's hit region. Collected during the
/// tree walk and used by the post-walk dispatcher to figure out which
/// rows count as siblings for the in-flight drag.
#[derive(Clone)]
enum DropScope {
    Project,
    Workspace { project_id: u64 },
    Tab { project_id: u64, workspace_id: u64 },
}

struct DropZone {
    rect: Rect,
    scope: DropScope,
}

/// Paint a 2px accent line at the top or bottom edge of `rect` so the
/// user can see exactly where a release will land. Inset horizontally
/// so the line sits inside the row's padding rather than against the
/// scroll-area gutter.
fn paint_drop_line(ui: &egui::Ui, rect: Rect, above: bool) {
    let y = if above { rect.min.y } else { rect.max.y - 1.0 };
    ui.painter().line_segment(
        [Pos2::new(rect.min.x + 6.0, y), Pos2::new(rect.max.x - 6.0, y)],
        Stroke::new(2.0, accent()),
    );
}

const HEADER: Color32 = Color32::from_rgb(140, 146, 162);

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
    // Pending tint change for a Tab. Project / Workspace / Tab keys
    // together uniquely identify the target row.
    let mut set_tab_tint: Option<(u64, u64, u64, Option<[u8; 3]>)> = None;
    // Pending "toggle folder group collapse" from a folder-header click.
    let mut toggle_group_collapsed: Option<std::path::PathBuf> = None;
    let mut reorder_project: Option<(u64, usize)> = None;
    let mut reorder_workspace: Option<(u64, u64, usize)> = None;
    let mut reorder_tab: Option<(u64, u64, u64, usize)> = None;
    // Each visible row registers its hit rect + scope here. Drop dispatch
    // happens once after the walk so inter-row gaps and the empty space
    // above the first / below the last sibling all resolve to a sane
    // target index instead of getting swallowed by a per-row hit-test.
    let mut drop_zones: Vec<DropZone> = Vec::new();
    // Pointer position (used by the indicator to decide above/below).
    // Read once from input so detection works during drag, when egui's
    // per-widget `contains_pointer` can be suppressed by the captured
    // drag interaction.
    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

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
            for project in app.projects.iter() {
                // Render a group header row whenever we enter a new
                // group. Projects without a group render flush-left as
                // before; grouped Projects nest one level deeper under
                // their shared folder name. When a group is collapsed,
                // the header still renders but its member Projects are
                // skipped entirely.
                let in_group = project.group_path.is_some();
                let group_is_collapsed = project
                    .group_path
                    .as_ref()
                    .is_some_and(|gp| app.group_collapsed.contains(gp));
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
                            expanded: Some(!group_is_collapsed),
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
                    if folder_row.main_clicked
                        && let Some(gp) = &project.group_path
                    {
                        toggle_group_collapsed = Some(gp.clone());
                    }
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
                // Skip rendering this Project (and everything under it)
                // when its group is collapsed.
                if group_is_collapsed {
                    continue;
                }
                let project_depth = if in_group { 1 } else { 0 };
                let tint_color = project
                    .tint
                    .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
                // Reserve space for the buttons we ACTUALLY draw: one
                // "new worktree" + (optional) "remove" for singleton /
                // standalone Projects. When the Project belongs to a
                // multi-member group, removal happens via the folder
                // header, so we don't reserve its slot — otherwise the
                // +N -M change badge sits further left than the icon
                // row suggests.
                let in_multi_group = project
                    .group_path
                    .as_ref()
                    .and_then(|gp| group_counts.get(gp))
                    .is_some_and(|c| *c > 1);
                // A missing Project inside a multi-member group would
                // otherwise be a dead end — the folder's "Remove folder
                // group" nukes every healthy sibling, and the atomic-group
                // rule hides the individual ×. Missing entries are
                // already inconsistent placeholders asking to be
                // relocated or removed, so let the user remove them
                // in-place; the group recomputes cleanly next frame.
                let allow_individual_remove = !in_multi_group || project.missing;
                let row_trailing_count = if allow_individual_remove { 2 } else { 1 };
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
                        trailing_count: row_trailing_count,
                        tree_guides: in_group, checkbox: None,
                    },
                );
                // `in_multi_group` reused from the row-construction
                // block above — suppress per-Project removal when the
                // Project is one of several siblings inside a folder
                // group. In that case the group must be removed
                // atomically via the folder header's "Remove folder
                // group" context menu.
                let project_trailing = if allow_individual_remove {
                    draw_trailing(
                        ui,
                        row.rect,
                        row.hovered,
                        &[
                            (icons::PLUS, "New worktree", 0),
                            (icons::X, "Remove project", 1),
                        ],
                    )
                } else {
                    draw_trailing(
                        ui,
                        row.rect,
                        row.hovered,
                        &[(icons::PLUS, "New worktree", 0)],
                    )
                };
                if project_trailing[0] {
                    new_workspace_for_project = Some(project.id);
                } else if allow_individual_remove
                    && project_trailing.get(1).copied().unwrap_or(false)
                {
                    remove_project = Some(project.id);
                } else if row.main_clicked {
                    toggle_project = Some(project.id);
                }
                // Drag source. Setting payload every frame while
                // dragged keeps it alive so the post-walk dispatcher can
                // read it on the release frame.
                if row.response.dragged() {
                    row.response.dnd_set_drag_payload(TreeDrag::Project { id: project.id });
                }
                // Indicator: paint a drop-line preview while a same-
                // scope drag is in flight and the pointer is over this
                // row's rect. Use input-based hover detection — egui
                // can suppress per-widget `contains_pointer` during an
                // active drag because the interaction is captured.
                if let Some(p) = pointer_pos {
                    if row.rect.contains(p) {
                        if let Some(payload) = egui::DragAndDrop::payload::<TreeDrag>(ctx) {
                            if let TreeDrag::Project { id: src_id } = payload.as_ref() {
                                if *src_id != project.id {
                                    let above = p.y < row.rect.center().y;
                                    paint_drop_line(ui, row.rect, above);
                                }
                            }
                        }
                    }
                }
                drop_zones.push(DropZone {
                    rect: row.rect,
                    scope: DropScope::Project,
                });
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
                    // Exception: a missing Project gets the escape hatch
                    // regardless, so the modal's "relocate or remove"
                    // promise holds when the project lives inside a group.
                    if allow_individual_remove {
                        ui.separator();
                        if ui.button(format!("{}  Remove Project", icons::X)).clicked() {
                            remove_project = Some(pid);
                            ui.close();
                        }
                    }
                });

                if project.expanded {
                    for wt in project.workspaces.iter() {
                        let active_wt = app.active.map(|(_, w, _)| w == wt.id).unwrap_or(false);
                        let t = crate::theme::current();
                        let badge = wt.git_status.as_ref().and_then(|s| {
                            if s.added > 0 || s.deleted > 0 {
                                Some((s.added, s.deleted, t.diff_added(), t.diff_deleted()))
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
                        if wt_row.response.dragged() {
                            wt_row.response.dnd_set_drag_payload(TreeDrag::Workspace {
                                project_id: project.id, id: wt.id });
                        }
                        if let Some(p) = pointer_pos {
                            if wt_row.rect.contains(p) {
                                if let Some(payload) = egui::DragAndDrop::payload::<TreeDrag>(ctx) {
                                    if let TreeDrag::Workspace { project_id: src_pid, id: src_id } = payload.as_ref() {
                                        if *src_id != wt.id && *src_pid == project.id {
                                            let above = p.y < wt_row.rect.center().y;
                                            paint_drop_line(ui, wt_row.rect, above);
                                        }
                                    }
                                }
                            }
                        }
                        drop_zones.push(DropZone {
                            rect: wt_row.rect,
                            scope: DropScope::Workspace { project_id: project.id },
                        });
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
                            for tab in wt.tabs.iter() {
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
                                let tab_tint_color = tab
                                    .tint
                                    .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b));
                                // Tint priority mirrors the Workspace row:
                                // explicit user tint wins over the active
                                // accent hint. Without this, the active
                                // tab would always paint its icon in
                                // accent and any tint the user picked on
                                // the active row would be invisible.
                                let tab_leading_color = tab_tint_color.or(if is_active {
                                    Some(accent())
                                } else {
                                    None
                                });
                                let tab_row = draw_row(
                                    ui,
                                    RowConfig {
                                        depth: tab_depth,
                                        expanded: None,
                                        leading: Some(icons::TERMINAL_WINDOW),
                                        leading_color: tab_leading_color,
                                        label: &tab.name,
                                        label_color: tab_tint_color,
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
                                if tab_row.response.dragged() {
                                    tab_row.response.dnd_set_drag_payload(TreeDrag::Tab {
                                        project_id: project.id, workspace_id: wt.id, id: tab.id });
                                }
                                if let Some(p) = pointer_pos {
                                    if tab_row.rect.contains(p) {
                                        if let Some(payload) = egui::DragAndDrop::payload::<TreeDrag>(ctx) {
                                            if let TreeDrag::Tab { project_id: src_pid, workspace_id: src_wid, id: src_id } = payload.as_ref() {
                                                if *src_id != tab.id && *src_pid == project.id && *src_wid == wt.id {
                                                    let above = p.y < tab_row.rect.center().y;
                                                    paint_drop_line(ui, tab_row.rect, above);
                                                }
                                            }
                                        }
                                    }
                                }
                                drop_zones.push(DropZone {
                                    rect: tab_row.rect,
                                    scope: DropScope::Tab {
                                        project_id: project.id,
                                        workspace_id: wt.id,
                                    },
                                });
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
                                let tab_pid = project.id;
                                let tab_wid = wt.id;
                                let tab_tid = tab.id;
                                let tab_name_snap = tab.name.clone();
                                tab_row.response.context_menu(|ui| {
                                    if ui.button(format!("{}  Rename", icons::PENCIL_SIMPLE)).clicked() {
                                        start_rename = Some((
                                            tab_pid,
                                            tab_wid,
                                            tab_tid,
                                            tab_name_snap.clone(),
                                        ));
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
                                            let color = egui::Color32::from_rgb(
                                                rgb[0], rgb[1], rgb[2],
                                            );
                                            let btn = egui::Button::new(
                                                egui::RichText::new(icons::TERMINAL_WINDOW)
                                                    .color(color)
                                                    .size(14.0),
                                            )
                                            .min_size(egui::vec2(22.0, 22.0))
                                            .frame(false);
                                            if ui.add(btn).on_hover_text(*label).clicked() {
                                                set_tab_tint = Some((
                                                    tab_pid,
                                                    tab_wid,
                                                    tab_tid,
                                                    Some(*rgb),
                                                ));
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
                                        set_tab_tint =
                                            Some((tab_pid, tab_wid, tab_tid, None));
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
    if let Some((pid, wid, tid, tint)) = set_tab_tint
        && let Some(p) = app.projects.iter_mut().find(|p| p.id == pid)
        && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid)
        && let Some(t) = w.tabs.iter_mut().find(|t| t.id == tid)
    {
        t.tint = tint;
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
    if let Some(group) = toggle_group_collapsed {
        if !app.group_collapsed.remove(&group) {
            app.group_collapsed.insert(group);
        }
    }
    if let Some((pid, wid)) = remove_worktree
        && let Some(p) = app.projects.iter().find(|p| p.id == pid)
    {
        // Always route through the confirm modal — workspace removal
        // runs `git worktree remove --force` and deletes the directory
        // on disk, so even a clean worktree deserves a "are you sure"
        // prompt before we discard a checked-out branch. Main checkout
        // (path == project path) skips the git call inside the modal
        // because git refuses to remove the primary worktree.
        let repo = p.path.clone();
        let ws = p.workspaces.iter().find(|w| w.id == wid);
        let is_main = ws.map(|w| w.path == repo).unwrap_or(true);
        let dirty = ws
            .filter(|_| !is_main)
            .map(|w| crate::git::worktree_dirty(&w.path))
            .unwrap_or_default();

        if let Some(w) = ws {
            app.pending_remove_worktree = Some(crate::state::PendingRemoveWorktree {
                project_id: pid,
                workspace_id: wid,
                label: w.label(),
                path: w.path.clone(),
                unpushed_commits: dirty.unpushed_commits,
                modified_files: dirty.modified_files,
                has_upstream: dirty.has_upstream,
                is_main,
            });
        }
    }
    if let Some(target) = close_tab {
        // Stage the close — the confirm modal handles the actual drop.
        app.pending_close_tab = Some(target);
    }
    // Global drop dispatch. Runs once per release frame and works
    // regardless of which row (or inter-row gap) the pointer was over —
    // the per-row `dnd_release_payload` approach swallowed releases
    // that landed in the ~3px spacing between rows. We:
    //   1) wait for an actual pointer release with a payload still set,
    //   2) filter the collected drop_zones to siblings of the source,
    //   3) require the release Y to fall within the visible sibling
    //      range (with a small pad so dropping just-below-last appends
    //      cleanly), and
    //   4) compute the new index by counting how many sibling row
    //      centers sit at-or-above the release Y. That count maps
    //      directly to the same `pi` / `pi + 1` semantics the per-row
    //      branch used, and `move_in_vec` corrects for the source's
    //      removal so downward drags land where the user dropped them.
    let release_pos = ctx.input(|i| {
        if i.pointer.any_released() {
            i.pointer.interact_pos()
        } else {
            None
        }
    });
    if let Some(release_pos) = release_pos {
        if let Some(payload) = egui::DragAndDrop::take_payload::<TreeDrag>(ctx) {
            let candidates: Vec<&DropZone> = drop_zones
                .iter()
                .filter(|z| match (payload.as_ref(), &z.scope) {
                    (TreeDrag::Project { .. }, DropScope::Project) => true,
                    (
                        TreeDrag::Workspace { project_id: pid, .. },
                        DropScope::Workspace { project_id: zpid },
                    ) => zpid == pid,
                    (
                        TreeDrag::Tab { project_id: pid, workspace_id: wid, .. },
                        DropScope::Tab { project_id: zpid, workspace_id: zwid },
                    ) => zpid == pid && zwid == wid,
                    _ => false,
                })
                .collect();

            if !candidates.is_empty() {
                let pad = 8.0;
                let first_y = candidates.first().unwrap().rect.min.y - pad;
                let last_y = candidates.last().unwrap().rect.max.y + pad;
                if release_pos.y >= first_y && release_pos.y <= last_y {
                    let new_index = candidates
                        .iter()
                        .filter(|z| z.rect.center().y <= release_pos.y)
                        .count();
                    match payload.as_ref() {
                        TreeDrag::Project { id } => {
                            reorder_project = Some((*id, new_index));
                        }
                        TreeDrag::Workspace { project_id, id } => {
                            reorder_workspace = Some((*project_id, *id, new_index));
                        }
                        TreeDrag::Tab { project_id, workspace_id, id } => {
                            reorder_tab = Some((*project_id, *workspace_id, *id, new_index));
                        }
                    }
                }
            }
        }
    }

    if let Some((pid, idx)) = reorder_project {
        app.reorder_project(pid, idx);
    }
    if let Some((pid, wid, idx)) = reorder_workspace {
        app.reorder_workspace(pid, wid, idx);
    }
    if let Some((pid, wid, tid, idx)) = reorder_tab {
        app.reorder_tab(pid, wid, tid, idx);
    }
}


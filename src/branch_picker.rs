//! Bottom-anchored branch picker popup. Lists local + remote branches
//! for every git repo under the active Workspace. Monorepos with nested
//! submodules/sub-repos get a repo filter at the top (All / per-repo);
//! each visible repo renders its own Local + per-remote tree.
//!
//! Clicking a branch either switches to an existing Workspace whose
//! worktree sits on that branch, or opens the "new workspace" modal
//! pre-filled to create one.
//!
//! Sizing: popup is bottom-anchored. A drag handle sits on the **top-right
//! corner** of the outer frame (diagonal resize). Dragging up/left grows
//! the popup; the bottom stays pinned above the status bar. Size persists
//! on `App` across opens so collapsing sections doesn't shrink it.

use crate::state::App;
use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use egui_phosphor::regular as icons;
use std::path::{Path, PathBuf};

const MIN_WIDTH: f32 = 280.0;
const MIN_HEIGHT: f32 = 200.0;
const CORNER_HANDLE: f32 = 14.0;

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.branch_picker_open {
        return;
    }
    crate::ui_status::poll_branch_picker(app);
    let Some((pid, wid, _)) = app.active else {
        app.branch_picker_open = false;
        return;
    };

    let t = theme::current();
    let screen = ctx.screen_rect();
    let max_h = screen.height() - crate::ui_status::HEIGHT - 40.0;
    let max_w = screen.width() - 24.0;
    let width = app.branch_picker_width.clamp(MIN_WIDTH, max_w);
    let height = app.branch_picker_height.clamp(MIN_HEIGHT, max_h);
    app.branch_picker_width = width;
    app.branch_picker_height = height;

    // Fixed position: bottom-left, floating just above the status bar.
    let bottom = screen.max.y - crate::ui_status::HEIGHT - 6.0;
    let left = screen.min.x + 12.0;
    let top = bottom - height;
    let right = left + width;
    let outer = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom));

    let mut close = false;
    let mut switch_to: Option<(crate::state::WorkspaceId, crate::state::TabId)> = None;
    let mut create_branch: Option<String> = None;
    let mut in_place: Option<(PathBuf, String)> = None;
    let mut new_filter: Option<Option<PathBuf>> = None;

    let ws_root = app
        .active_workspace_path()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    let repos_snapshot: Vec<(PathBuf, Vec<String>, Vec<String>)> =
        app.branch_picker_repos.clone();
    let filter_snapshot: Option<PathBuf> = app.branch_picker_filter.clone();

    let existing: std::collections::HashMap<String, (crate::state::WorkspaceId, crate::state::TabId)> = {
        let project = app.projects.iter().find(|p| p.id == pid);
        project
            .map(|p| {
                p.workspaces
                    .iter()
                    .filter_map(|w| {
                        let tid = w.active_tab.or_else(|| w.tabs.first().map(|t| t.id))?;
                        Some((w.name.clone(), (w.id, tid)))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    egui::Area::new(egui::Id::new("branch_picker"))
        .order(egui::Order::Foreground)
        .fixed_pos(outer.min)
        .show(ctx, |ui| {
            // Single outer frame — no inner frames / sub-borders.
            egui::Frame::NONE
                .fill(t.surface.to_color32())
                .stroke(egui::Stroke::new(1.0, t.divider.to_color32()))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_size(outer.size());
                    ui.set_max_size(outer.size());

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}  Switch branch", icons::GIT_BRANCH))
                                .size(12.0)
                                .color(t.text.to_color32()),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                // Close × flush with the right edge — the
                                // resize handle now lives in its own row
                                // anchored to the outer top-right corner,
                                // so the title bar no longer has to
                                // reserve space for it.
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(icons::X).size(13.0))
                                            .frame(false)
                                            .min_size(egui::vec2(22.0, 22.0)),
                                    )
                                    .on_hover_text("Close (Esc)")
                                    .clicked()
                                {
                                    close = true;
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);

                    if repos_snapshot.len() > 1 {
                        ScrollArea::horizontal()
                            .id_salt("branch_picker_repos")
                            .max_height(30.0)
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if chip(ui, "All repos", filter_snapshot.is_none(), &t) {
                                        new_filter = Some(None);
                                    }
                                    for (root, _, _) in &repos_snapshot {
                                        let name = repo_display(root, &ws_root);
                                        let selected = filter_snapshot.as_deref()
                                            == Some(root.as_path());
                                        if chip(ui, &name, selected, &t) {
                                            new_filter = Some(Some(root.clone()));
                                        }
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }

                    let input_id = egui::Id::new("branch_picker_query");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut app.branch_picker_query)
                            .id(input_id)
                            .hint_text("Filter branches…")
                            .desired_width(f32::INFINITY),
                    );
                    let focus_flag = egui::Id::new("branch_picker_focused");
                    let already = ctx
                        .memory(|m| m.data.get_temp::<bool>(focus_flag))
                        .unwrap_or(false);
                    if !already {
                        resp.request_focus();
                        ctx.memory_mut(|m| m.data.insert_temp(focus_flag, true));
                    }

                    ui.add_space(6.0);

                    // Surface the last in-place switch error (typically
                    // "please commit or stash first"). Dismissible by
                    // clicking it. No auto-stash — by design.
                    if let Some(err) = app.branch_picker_error.clone() {
                        let resp = ui.add(
                            egui::Label::new(
                                RichText::new(err)
                                    .size(11.0)
                                    .color(t.error.to_color32()),
                            )
                            .sense(egui::Sense::click())
                            .wrap(),
                        );
                        if resp.clicked() {
                            app.branch_picker_error = None;
                        }
                        ui.add_space(4.0);
                    }

                    let query = app.branch_picker_query.trim().to_lowercase();
                    let visible_repos: Vec<&(PathBuf, Vec<String>, Vec<String>)> =
                        repos_snapshot
                            .iter()
                            .filter(|(r, _, _)| {
                                filter_snapshot
                                    .as_deref()
                                    .map(|f| f == r.as_path())
                                    .unwrap_or(true)
                            })
                            .collect();

                    if app.branch_picker_loading && visible_repos.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(
                                RichText::new("Loading branches…")
                                    .size(11.5)
                                    .color(t.text_muted.to_color32()),
                            );
                        });
                    } else if visible_repos.is_empty() {
                        ui.label(
                            RichText::new("No repos found under this Workspace")
                                .size(11.5)
                                .color(t.text_muted.to_color32()),
                        );
                    } else {
                        ScrollArea::vertical()
                            .id_salt("branch_picker_list")
                            .max_height(ui.available_height())
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let multi_repo = visible_repos.len() > 1;
                                for (root, locals, remotes) in &visible_repos {
                                    render_repo_section(
                                        ui,
                                        root,
                                        &ws_root,
                                        locals,
                                        remotes,
                                        &query,
                                        &existing,
                                        wid,
                                        multi_repo,
                                        &mut app.branch_picker_collapsed,
                                        &mut switch_to,
                                        &mut create_branch,
                                        &mut in_place,
                                        &t,
                                    );
                                }
                            });
                    }
                });

            // Top-right corner drag handle — lives on the outer frame so
            // the popup grows up / left. Bottom stays pinned above the
            // status bar.
            let handle_rect = egui::Rect::from_min_max(
                egui::pos2(outer.max.x - CORNER_HANDLE, outer.min.y),
                egui::pos2(outer.max.x, outer.min.y + CORNER_HANDLE),
            );
            let handle_resp = ui.interact(
                handle_rect,
                egui::Id::new("branch_picker_resize_corner"),
                egui::Sense::drag(),
            );
            let grip_color = if handle_resp.hovered() || handle_resp.dragged() {
                t.accent.to_color32()
            } else {
                t.text_muted.to_color32()
            };
            // Two diagonal ticks forming a resize indicator.
            let painter = ui.painter();
            painter.line_segment(
                [
                    egui::pos2(handle_rect.max.x - 2.0, handle_rect.min.y + 8.0),
                    egui::pos2(handle_rect.max.x - 8.0, handle_rect.min.y + 2.0),
                ],
                egui::Stroke::new(1.5, grip_color),
            );
            painter.line_segment(
                [
                    egui::pos2(handle_rect.max.x - 2.0, handle_rect.min.y + 12.0),
                    egui::pos2(handle_rect.max.x - 12.0, handle_rect.min.y + 2.0),
                ],
                egui::Stroke::new(1.5, grip_color),
            );
            if handle_resp.hovered() || handle_resp.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeNeSw);
            }
            if handle_resp.dragged() {
                let d = handle_resp.drag_delta();
                // Up/left = grow; down/right = shrink.
                app.branch_picker_height =
                    (app.branch_picker_height - d.y).clamp(MIN_HEIGHT, max_h);
                app.branch_picker_width =
                    (app.branch_picker_width + d.x).clamp(MIN_WIDTH, max_w);
            }
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    // Grace window: ignore outside-clicks in the first 150ms so the
    // opening click itself can't race-close the popup.
    let grace = app
        .branch_picker_opened_at
        .map(|t| t.elapsed().as_millis() < 150)
        .unwrap_or(false);
    if !grace
        && ctx.input(|i| i.pointer.any_click())
        && let Some(p) = ctx.input(|i| i.pointer.latest_pos())
        && !outer.expand(4.0).contains(p)
    {
        close = true;
    }

    if let Some(f) = new_filter {
        app.branch_picker_filter = f;
    }
    if close {
        app.branch_picker_open = false;
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("branch_picker_focused")));
    }
    if let Some((w, tab)) = switch_to {
        app.set_active(pid, w, tab);
        app.branch_picker_open = false;
    }
    if let Some(b) = create_branch {
        app.open_new_workspace_modal(pid);
        if let Some(modal) = app.new_workspace_modal.as_mut() {
            modal.branch = b;
            modal.create_new_branch = false;
            modal.branch_locked = true;
        }
        app.branch_picker_open = false;
    }
    if let Some((repo, branch)) = in_place {
        // Synchronous shell-out — git switch is fast when the tree is
        // clean; when it's dirty the call returns in milliseconds with
        // the refusal we want to surface. No worker thread needed.
        match crate::git::checkout_branch(&repo, &branch) {
            Ok(()) => {
                app.branch_picker_error = None;
                app.refresh_active_git_status(ctx);
            }
            Err(msg) => {
                app.branch_picker_error = Some(msg);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_repo_section(
    ui: &mut egui::Ui,
    repo_root: &Path,
    ws_root: &Path,
    locals: &[String],
    remotes: &[String],
    query: &str,
    existing: &std::collections::HashMap<String, (crate::state::WorkspaceId, crate::state::TabId)>,
    active_wid: crate::state::WorkspaceId,
    multi_repo: bool,
    collapsed: &mut std::collections::HashSet<String>,
    switch_to: &mut Option<(crate::state::WorkspaceId, crate::state::TabId)>,
    create_branch: &mut Option<String>,
    in_place: &mut Option<(PathBuf, String)>,
    t: &theme::Theme,
) {
    // `existing` maps outer-Project worktree names to Workspace ids. A
    // submodule's "main" is unrelated to the outer Project's "main", so
    // only consult `existing` when this section is the outer repo.
    let match_existing = repo_root == ws_root;
    let repo_key = format!("repo:{}", repo_root.display());

    let mut remote_groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for r in remotes {
        if !query.is_empty() && !r.to_lowercase().contains(query) {
            continue;
        }
        if let Some((remote, branch)) = r.split_once('/') {
            remote_groups
                .entry(remote.to_string())
                .or_default()
                .push(branch.to_string());
        }
    }
    let local_filtered: Vec<&String> = locals
        .iter()
        .filter(|b| query.is_empty() || b.to_lowercase().contains(query))
        .collect();
    let total = local_filtered.len() + remote_groups.values().map(|v| v.len()).sum::<usize>();

    if multi_repo {
        let repo_collapsed = collapsed.contains(&repo_key);
        let name = repo_display(repo_root, ws_root);
        if section_header(ui, &name, total, repo_collapsed, 0, t) {
            toggle(collapsed, &repo_key);
        }
        if repo_collapsed {
            return;
        }
    }

    let local_key = format!("{repo_key}::local");
    let local_collapsed = collapsed.contains(&local_key);
    if section_header(
        ui,
        "Local",
        local_filtered.len(),
        local_collapsed,
        if multi_repo { 1 } else { 0 },
        t,
    ) {
        toggle(collapsed, &local_key);
    }
    if !local_collapsed {
        for b in &local_filtered {
            let existing_wt = if match_existing { existing.get(*b).copied() } else { None };
            let is_active = existing_wt.map(|(w, _)| w == active_wid).unwrap_or(false);
            // Scope every row's widget ids by (repo, local, branch) —
            // without this, `main` appearing in Local + origin + another
            // repo all share the same egui id and trigger clash overlays.
            let action = ui
                .push_id((repo_root, "local", b.as_str()), |ui| {
                    row(
                        ui,
                        b,
                        is_active,
                        existing_wt.is_some(),
                        if multi_repo { 2 } else { 1 },
                        t,
                    )
                })
                .inner;
            match action {
                RowAction::Primary => {
                    if let Some((w, tab)) = existing_wt {
                        *switch_to = Some((w, tab));
                    } else {
                        *create_branch = Some(b.to_string());
                    }
                }
                RowAction::InPlace => {
                    *in_place = Some((repo_root.to_path_buf(), b.to_string()));
                }
                RowAction::None => {}
            }
        }
    }

    for (remote, branches) in &remote_groups {
        let key = format!("{repo_key}::remote::{remote}");
        let rc = collapsed.contains(&key);
        if section_header(
            ui,
            remote,
            branches.len(),
            rc,
            if multi_repo { 1 } else { 0 },
            t,
        ) {
            toggle(collapsed, &key);
        }
        if rc {
            continue;
        }
        for b in branches {
            let existing_wt = if match_existing { existing.get(b.as_str()).copied() } else { None };
            let is_active = existing_wt.map(|(w, _)| w == active_wid).unwrap_or(false);
            let action = ui
                .push_id((repo_root, "remote", remote.as_str(), b.as_str()), |ui| {
                    row(
                        ui,
                        b,
                        is_active,
                        existing_wt.is_some(),
                        if multi_repo { 2 } else { 1 },
                        t,
                    )
                })
                .inner;
            match action {
                RowAction::Primary => {
                    if let Some((w, tab)) = existing_wt {
                        *switch_to = Some((w, tab));
                    } else {
                        *create_branch = Some(b.clone());
                    }
                }
                RowAction::InPlace => {
                    *in_place = Some((repo_root.to_path_buf(), b.clone()));
                }
                RowAction::None => {}
            }
        }
    }
}

fn repo_display(repo: &Path, ws_root: &Path) -> String {
    if repo == ws_root {
        return repo
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("repo")
            .to_string();
    }
    repo.strip_prefix(ws_root)
        .ok()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            repo.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("repo")
                .to_string()
        })
}

fn chip(ui: &mut egui::Ui, label: &str, selected: bool, t: &theme::Theme) -> bool {
    let txt = RichText::new(label).size(10.5);
    let btn = if selected {
        let a = t.accent;
        egui::Button::new(txt.color(t.text.to_color32()))
            .fill(Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55))
            .stroke(egui::Stroke::new(1.0, t.accent.to_color32()))
    } else {
        egui::Button::new(txt.color(t.text_muted.to_color32()))
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, t.divider.to_color32()))
    };
    ui.add(btn.min_size(egui::vec2(0.0, 22.0))).clicked()
}

/// What the user clicked on a branch row.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    None,
    /// The row body — open an existing worktree if present, else
    /// open the "new worktree from this branch" modal.
    Primary,
    /// The small right-side icon — in-place `git switch`. Respects
    /// git's dirty-tree refusal (error bubbles via
    /// `App::branch_picker_error`); does no auto-stash.
    InPlace,
}

fn row(
    ui: &mut egui::Ui,
    branch: &str,
    is_active: bool,
    has_worktree: bool,
    indent: u8,
    t: &theme::Theme,
) -> RowAction {
    let height = 24.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let hovered = resp.hovered();
    let bg = if is_active {
        let a = t.accent;
        Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 45)
    } else if hovered {
        t.row_hover.to_color32()
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    let x = rect.min.x + 8.0 + 16.0 * indent as f32;
    ui.painter().text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        branch,
        egui::FontId::proportional(12.0),
        t.text.to_color32(),
    );
    let badge_text = if is_active {
        "current"
    } else if has_worktree {
        "open"
    } else {
        "create"
    };
    let badge_color = if is_active {
        t.accent.to_color32()
    } else {
        t.text_muted.to_color32()
    };
    // Measure badge text width so we can place the in-place action
    // icon just to its left without overlapping.
    let badge_font = egui::FontId::proportional(10.5);
    let badge_w = ui
        .fonts_mut(|f| f.layout_no_wrap(badge_text.to_string(), badge_font.clone(), badge_color))
        .size()
        .x;
    ui.painter().text(
        egui::pos2(rect.max.x - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        badge_text,
        badge_font,
        badge_color,
    );

    // In-place switch icon — only offered when the branch isn't the
    // active one. Hidden on hover-zero so it doesn't clutter the row.
    let mut in_place_clicked = false;
    if !is_active {
        let icon_size = 18.0;
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.max.x - 16.0 - badge_w - icon_size,
                rect.center().y - icon_size / 2.0,
            ),
            egui::vec2(icon_size, icon_size),
        );
        // Row itself is already scoped via ui.push_id by the caller, so
        // a constant local salt is enough to disambiguate body vs icon.
        let icon_resp = ui.interact(
            icon_rect,
            ui.id().with("in_place_icon"),
            egui::Sense::click(),
        );
        let icon_color = if icon_resp.hovered() {
            t.accent.to_color32()
        } else if hovered {
            t.text_muted.to_color32()
        } else {
            Color32::TRANSPARENT
        };
        if icon_color != Color32::TRANSPARENT {
            ui.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::ARROW_RIGHT,
                egui::FontId::proportional(13.0),
                icon_color,
            );
        }
        if icon_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            icon_resp.clone().on_hover_text(
                "Switch in place (git switch) — requires a clean tree",
            );
        }
        if icon_resp.clicked() {
            in_place_clicked = true;
        }
    }

    if hovered && !in_place_clicked {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if in_place_clicked {
        RowAction::InPlace
    } else if resp.clicked() && !is_active {
        RowAction::Primary
    } else {
        RowAction::None
    }
}

fn section_header(
    ui: &mut egui::Ui,
    name: &str,
    count: usize,
    collapsed: bool,
    indent: u8,
    t: &theme::Theme,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 22.0),
        egui::Sense::click(),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let caret = if collapsed {
        icons::CARET_RIGHT
    } else {
        icons::CARET_DOWN
    };
    let x0 = rect.min.x + 4.0 + 16.0 * indent as f32;
    ui.painter().text(
        egui::pos2(x0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        caret,
        egui::FontId::proportional(11.0),
        t.text_muted.to_color32(),
    );
    ui.painter().text(
        egui::pos2(x0 + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(11.5),
        if indent == 0 {
            t.text.to_color32()
        } else {
            t.text_muted.to_color32()
        },
    );
    ui.painter().text(
        egui::pos2(rect.max.x - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(10.5),
        t.text_muted.to_color32(),
    );
    resp.clicked()
}

fn toggle(set: &mut std::collections::HashSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_string());
    }
}

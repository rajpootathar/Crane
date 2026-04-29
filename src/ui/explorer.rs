use crate::git::{self, FileChange};
use crate::state::{
    App, FileOp, GitOpKind, GitOpStatus, NewEntryKind, PendingNewEntry, RightTab,
    FILE_OP_HISTORY_CAP,
};
use crate::ui::util::{
    CheckState, draw_row,
    RowConfig, accent, muted, text,
};
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;


const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);
const WARN: Color32 = Color32::from_rgb(220, 180, 110);
const MODIFIED_BLUE: Color32 = Color32::from_rgb(100, 160, 230);

fn status_color(status: git::ChangeStatus) -> Color32 {
    match status {
        git::ChangeStatus::Added => ADD,
        git::ChangeStatus::Modified => MODIFIED_BLUE,
        git::ChangeStatus::Deleted => DEL,
        git::ChangeStatus::Renamed => MODIFIED_BLUE,
        git::ChangeStatus::Untracked => ADD,
    }
}

fn status_glyph(status: git::ChangeStatus) -> &'static str {
    match status {
        git::ChangeStatus::Added => "A",
        git::ChangeStatus::Modified => "M",
        git::ChangeStatus::Deleted => "D",
        git::ChangeStatus::Renamed => "R",
        git::ChangeStatus::Untracked => "U",
    }
}

/// Compact toolbar button for the Changes-pane top row. Shows the
/// kind icon, plus tooltip, and a spinner when its op is running.
/// All buttons disable while ANY op is in flight so a double-click
/// can't enqueue a competing op.
fn toolbar_button(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    running: bool,
    any_running: bool,
) -> bool {
    let label = if running {
        // The phosphor "circle notch" / spinner glyph isn't in our
        // icon set; use a small ring-ish placeholder. The repaint
        // tick in the toolbar makes this re-render at 6-7Hz so it
        // reads as "active".
        format!("{}", icons::ARROW_COUNTER_CLOCKWISE)
    } else {
        icon.to_string()
    };
    let resp = ui.add_enabled(
        !any_running || running,
        egui::Button::new(egui::RichText::new(label).size(13.0))
            .min_size(egui::vec2(28.0, 22.0)),
    );
    let resp = resp.on_hover_text(tooltip);
    if running {
        // Tint when busy so the running button stands apart.
        let painter = ui.painter();
        painter.rect_stroke(
            resp.rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0, accent()),
            egui::StrokeKind::Inside,
        );
    }
    resp.clicked()
}

fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "linux")]
    {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("/"));
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

pub fn render(ui: &mut egui::Ui, app: &mut App) {
    // Match the Main Panel top bar (`ui::top::TOPBAR_H = 34.0`) so the
    // Changes/Files tab strip and the Browser/Terminal button row sit
    // on the same horizontal line across the whole window. Using
    // `ui.add_space()` + `ui.horizontal()` produced a 40px strip that
    // floated ~6px above the main top bar — the misalignment showed
    // up immediately next to the Browser button.
    const STRIP_H: f32 = crate::ui::top::TOPBAR_H;
    let outer = ui.available_rect_before_wrap();
    let strip_rect = egui::Rect::from_min_size(
        outer.min,
        egui::vec2(outer.width(), STRIP_H),
    );

    // Full-width bottom divider — previously `ui.min_rect().max.x`
    // clipped the line to the content width (ending under "Files"),
    // leaving the right half of the panel with no underline.
    ui.painter().line_segment(
        [
            egui::pos2(strip_rect.min.x, strip_rect.max.y),
            egui::pos2(strip_rect.max.x, strip_rect.max.y),
        ],
        egui::Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
    );

    let mut strip_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(strip_rect.shrink2(egui::vec2(10.0, 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    tab_chip(&mut strip_ui, "Changes", app.right_tab == RightTab::Changes, || {
        app.right_tab = RightTab::Changes;
    });
    strip_ui.add_space(4.0);
    tab_chip(&mut strip_ui, "Files", app.right_tab == RightTab::Files, || {
        app.right_tab = RightTab::Files;
    });

    ui.allocate_rect(strip_rect, egui::Sense::hover());
    ui.add_space(2.0);

    match app.right_tab {
        RightTab::Changes => render_changes(ui, app),
        RightTab::Files => render_files(ui, app),
    }
}

fn tab_chip(ui: &mut egui::Ui, label: &str, active: bool, mut on_click: impl FnMut()) {
    let color = if active { text() } else { muted() };
    let resp = ui
        .scope(|ui| {
            let v = ui.visuals_mut();
            v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            v.widgets.active.bg_stroke = egui::Stroke::NONE;
            let r = ui.add(
                egui::Button::new(
                    RichText::new(label).size(12.5).color(color),
                )
                .min_size(egui::vec2(0.0, 26.0)),
            );
            if active {
                let rect = r.rect;
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.min.x + 6.0, rect.max.y),
                        egui::pos2(rect.max.x - 6.0, rect.max.y),
                    ],
                    egui::Stroke::new(2.0, accent()),
                );
            }
            r
        })
        .inner;
    if resp.clicked() {
        on_click();
    }
}

fn render_changes(ui: &mut egui::Ui, app: &mut App) {
    let repo_path = match app.active_workspace_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_row(ui, "No active worktree");
            return;
        }
    };
    let status = match app.active_workspace_mut().and_then(|w| w.git_status.clone()) {
        Some(s) => s,
        None => {
            dim_row(ui, "(not a git repo)");
            return;
        }
    };

    // Top toolbar: branch + ahead/behind + push/pull/fetch.
    // Replaces the dropdown-buried push/pull from the old design —
    // the most-used network ops are now first-class clickable
    // buttons. Each button shows a spinner while its op is in
    // flight (driven by `app.git_op_status`); other buttons
    // disable so a double-click can't queue a second op.
    let op_status = app.git_op_status.lock().clone();
    let any_op_running = matches!(op_status, GitOpStatus::Running(_));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(
            RichText::new(format!("{}  {}", icons::GIT_BRANCH, status.branch))
                .color(text())
                .size(12.0)
                .strong(),
        );
        if let Some(ab) = status.ahead_behind {
            if ab.ahead > 0 {
                ui.label(
                    RichText::new(format!("{} {}", icons::ARROW_UP, ab.ahead))
                        .color(muted())
                        .size(11.0),
                );
            }
            if ab.behind > 0 {
                ui.label(
                    RichText::new(format!("{} {}", icons::ARROW_DOWN, ab.behind))
                        .color(muted())
                        .size(11.0),
                );
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let fetch_running = matches!(op_status, GitOpStatus::Running(GitOpKind::Fetch));
            let push_running = matches!(op_status, GitOpStatus::Running(GitOpKind::Push));
            let pull_running = matches!(op_status, GitOpStatus::Running(GitOpKind::Pull));
            // Render right-to-left so insertion order is reverse:
            // Fetch, Pull, Push (visually).
            if toolbar_button(
                ui,
                if fetch_running { icons::ARROW_COUNTER_CLOCKWISE } else { icons::ARROW_COUNTER_CLOCKWISE },
                "Fetch",
                fetch_running,
                any_op_running,
            ) {
                app.dispatch_git_op(
                    GitOpKind::Fetch,
                    repo_path.clone(),
                    ui.ctx().clone(),
                    None,
                );
            }
            if toolbar_button(
                ui,
                icons::ARROW_DOWN,
                "Pull",
                pull_running,
                any_op_running,
            ) {
                app.dispatch_git_op(
                    GitOpKind::Pull,
                    repo_path.clone(),
                    ui.ctx().clone(),
                    None,
                );
            }
            if toolbar_button(
                ui,
                icons::ARROW_UP,
                "Push",
                push_running,
                any_op_running,
            ) {
                app.dispatch_git_op(
                    GitOpKind::Push,
                    repo_path.clone(),
                    ui.ctx().clone(),
                    None,
                );
            }
        });
    });
    ui.add_space(4.0);
    // While a network op runs, request a frame in ~150ms so the
    // spinner animates smoothly even without other input.
    if any_op_running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    }

    let mut stage_paths: Vec<String> = Vec::new();
    let mut unstage_paths: Vec<String> = Vec::new();
    let mut open_diff: Option<String> = None;
    let mut toggle_dir: Option<String> = None;
    let collapsed = app.collapsed_change_dirs.clone();

    // Pin the commit form to the bottom of the right panel. Reserve a
    // footer rect now, render the scrollable changes tree into the
    // space above it, then fill the footer with the commit UI. The
    // footer height grows when an error is present so the message
    // doesn't get clipped.
    let footer_base = 128.0;
    let err_h = if app.git_error.is_some() { 40.0 } else { 0.0 };
    let footer_h = footer_base + err_h;
    let outer = ui.available_rect_before_wrap();
    let footer_rect = egui::Rect::from_min_max(
        egui::pos2(outer.min.x, outer.max.y - footer_h),
        outer.max,
    );
    let scroll_rect = egui::Rect::from_min_max(
        outer.min,
        egui::pos2(outer.max.x, outer.max.y - footer_h),
    );

    let mut scroll_ui = ui.new_child(egui::UiBuilder::new().max_rect(scroll_rect));
    scroll_ui.set_clip_rect(scroll_rect);
    egui::ScrollArea::vertical()
        .id_salt("right_changes")
        .auto_shrink([false, false])
        .show(&mut scroll_ui, |ui| {
            let all_changes: Vec<&FileChange> = status.changes.iter().collect();

            if !all_changes.is_empty() {
                render_change_tree(
                    ui,
                    &all_changes,
                    &collapsed,
                    &mut unstage_paths,
                    &mut stage_paths,
                    &mut open_diff,
                    &mut toggle_dir,
                );
            } else {
                dim_row(ui, "working tree clean");
            }
        });

    // ---- Footer: commit message + primary Commit button + more menu ----
    let theme = crate::theme::current();
    let footer_fill = theme.sidebar_bg.to_color32();
    let divider_col = theme.divider.to_color32();
    ui.painter().rect_filled(footer_rect, 0.0, footer_fill);
    ui.painter().line_segment(
        [footer_rect.left_top(), footer_rect.right_top()],
        egui::Stroke::new(1.0, divider_col),
    );

    let staged_count = status.changes.iter().filter(|c| c.has_staged).count();
    let has_staged = staged_count > 0;
    let has_message = !app.commit_message.trim().is_empty();
    let can_commit = has_staged && has_message;

    let mut footer_ui = ui.new_child(
        egui::UiBuilder::new().max_rect(footer_rect.shrink2(egui::vec2(10.0, 10.0))),
    );
    footer_ui.set_clip_rect(footer_rect);

    let text_resp = footer_ui.add(
        egui::TextEdit::multiline(&mut app.commit_message)
            .hint_text(if has_staged {
                "Commit message"
            } else {
                "Stage files to commit"
            })
            .desired_rows(2)
            .desired_width(footer_ui.available_width())
            .font(egui::FontId::new(12.5, egui::FontFamily::Proportional)),
    );
    let mut keyboard_commit = false;
    if text_resp.has_focus() {
        let submit = footer_ui.input(|i| {
            i.key_pressed(egui::Key::Enter)
                && (i.modifiers.command || i.modifiers.mac_cmd)
        });
        if submit && can_commit && !any_op_running {
            keyboard_commit = true;
        }
    }

    footer_ui.add_space(8.0);

    let mut action_commit = false;

    let commit_running = matches!(
        op_status,
        GitOpStatus::Running(GitOpKind::Commit | GitOpKind::CommitAndPush)
    );
    let commit_enabled = can_commit && !any_op_running;
    let commit_label = if commit_running {
        format!("{}  Committing…", icons::ARROW_COUNTER_CLOCKWISE)
    } else {
        // Show "Commit to <branch>" so users see exactly where the
        // commit will land before clicking. Catches the "wait, am I
        // on main?" mistake at the bottom of long debug sessions.
        format!("{}  Commit to {}", icons::CHECK, status.branch)
    };

    let primary_w = footer_ui.available_width();
    footer_ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.widgets.inactive.weak_bg_fill = theme.accent.to_color32();
        v.widgets.inactive.bg_fill = theme.accent.to_color32();
        v.widgets.hovered.weak_bg_fill = theme.accent.to_color32().gamma_multiply(1.15);
        v.widgets.hovered.bg_fill = theme.accent.to_color32().gamma_multiply(1.15);
        v.widgets.active.weak_bg_fill = theme.accent.to_color32().gamma_multiply(0.9);
        v.widgets.active.bg_fill = theme.accent.to_color32().gamma_multiply(0.9);
        v.widgets.inactive.fg_stroke.color = Color32::WHITE;
        v.widgets.hovered.fg_stroke.color = Color32::WHITE;
        v.widgets.active.fg_stroke.color = Color32::WHITE;
        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        v.widgets.active.bg_stroke = egui::Stroke::NONE;
        v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        v.widgets.active.corner_radius = egui::CornerRadius::same(6);
        ui.add_enabled_ui(commit_enabled, |ui| {
            let r = ui.add(
                egui::Button::new(
                    RichText::new(commit_label)
                        .size(13.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .min_size(egui::vec2(primary_w, 30.0)),
            );
            if r.clicked() {
                action_commit = true;
            }
        });
    });

    // Status pill: shows in-flight op, last success, or error.
    // Wins over the legacy `git_error` pill since it carries the
    // op kind too (so users can tell if "auth failed" was Push or
    // Pull).
    match &op_status {
        GitOpStatus::Idle => {
            if let Some(err) = &app.git_error {
                footer_ui.add_space(6.0);
                footer_ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(err).color(DEL).size(11.0));
                });
            }
        }
        GitOpStatus::Running(kind) => {
            footer_ui.add_space(6.0);
            footer_ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}…", kind.label()))
                        .color(muted())
                        .size(11.0)
                        .italics(),
                );
            });
        }
        GitOpStatus::Done { kind, message } => {
            footer_ui.add_space(6.0);
            footer_ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}: {}", kind.label(), message))
                        .color(ADD)
                        .size(11.0),
                );
            });
        }
        GitOpStatus::Failed { kind, error } => {
            footer_ui.add_space(6.0);
            footer_ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} failed: {}", kind.label(), error))
                        .color(DEL)
                        .size(11.0),
                );
            });
        }
    }

    if let Some(dir) = toggle_dir
        && !app.collapsed_change_dirs.remove(&dir) {
            app.collapsed_change_dirs.insert(dir);
        }
    if !stage_paths.is_empty() {
        let mut err: Option<String> = None;
        for path in &stage_paths {
            if let Err(e) = git::stage(&repo_path, path) {
                err = Some(e);
                break;
            }
        }
        match err {
            Some(e) => app.git_error = Some(e),
            None => {
                app.git_error = None;
                force_status_refresh(app);
            }
        }
    }
    if !unstage_paths.is_empty() {
        let mut err: Option<String> = None;
        for path in &unstage_paths {
            if let Err(e) = git::unstage(&repo_path, path) {
                err = Some(e);
                break;
            }
        }
        match err {
            Some(e) => app.git_error = Some(e),
            None => {
                app.git_error = None;
                force_status_refresh(app);
            }
        }
    }
    if let Some(path) = open_diff {
        open_file_diff(app, &repo_path, &path);
    }
    if action_commit || keyboard_commit {
        let msg = app.commit_message.clone();
        app.dispatch_git_op(
            GitOpKind::Commit,
            repo_path.clone(),
            ui.ctx().clone(),
            Some(msg),
        );
        app.commit_message.clear();
    }
    // Refresh git_status whenever an async op transitions to Done
    // so the file list + ahead/behind reflect the new HEAD.
    if matches!(&op_status, GitOpStatus::Done { .. }) {
        force_status_refresh(app);
    }
}

fn force_status_refresh(app: &mut App) {
    if let Some(wt) = app.active_workspace_mut() {
        wt.last_status_refresh = None;
    }
}

fn open_file_diff(app: &mut App, repo: &std::path::Path, rel_path: &str) {
    let full = repo.join(rel_path);
    let right_text = std::fs::read_to_string(&full).unwrap_or_default();
    let left_text = git::head_content(repo, rel_path);
    let title = format!(
        "diff: {}",
        std::path::Path::new(rel_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(rel_path)
    );
    if let Some(ws) = app.active_layout() {
        ws.open_or_focus_diff(
            format!("HEAD:{rel_path}"),
            rel_path.to_string(),
            left_text,
            right_text,
            title,
        );
    }
}

fn dim_row(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text).color(muted()).size(11.5));
    });
}

#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: Vec<(String, FileChange)>,
}

/// Walk a DirNode subtree and collect every file's path — used by the
/// folder-level stage/unstage action to apply a single click across
/// the whole subtree.
fn collect_paths(node: &DirNode, out: &mut Vec<String>) {
    for (_, change) in &node.files {
        out.push(change.path.clone());
    }
    for child in node.dirs.values() {
        collect_paths(child, out);
    }
}

fn build_tree(changes: &[&FileChange]) -> DirNode {
    let mut root = DirNode::default();
    for c in changes {
        let parts: Vec<&str> = c.path.split('/').collect();
        let (file, dirs) = parts.split_last().unwrap_or((&"", &[]));
        let mut node = &mut root;
        for d in dirs {
            node = node.dirs.entry((*d).to_string()).or_default();
        }
        node.files.push(((*file).to_string(), (*c).clone()));
    }
    root
}

#[allow(clippy::too_many_arguments)]
fn render_change_tree(
    ui: &mut egui::Ui,
    changes: &[&FileChange],
    collapsed: &std::collections::HashSet<String>,
    unstage_paths: &mut Vec<String>,
    stage_paths: &mut Vec<String>,
    open_diff: &mut Option<String>,
    toggle_dir: &mut Option<String>,
) {
    let tree = build_tree(changes);
    render_change_node(
        ui,
        &tree,
        "",
        0,
        collapsed,
        unstage_paths,
        stage_paths,
        open_diff,
        toggle_dir,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_change_node(
    ui: &mut egui::Ui,
    node: &DirNode,
    prefix: &str,
    depth: usize,
    collapsed: &std::collections::HashSet<String>,
    unstage_paths: &mut Vec<String>,
    stage_paths: &mut Vec<String>,
    open_diff: &mut Option<String>,
    toggle_dir: &mut Option<String>,
) {
    for (dir_name, child) in &node.dirs {
        let child_prefix = if prefix.is_empty() {
            dir_name.clone()
        } else {
            format!("{prefix}/{dir_name}")
        };
        let key = child_prefix.clone();
        let is_collapsed = collapsed.contains(&key);
        let (all_staged, any_staged) = dir_staged_state(child);
        let check = if all_staged {
            CheckState::Checked
        } else if any_staged {
            CheckState::Indeterminate
        } else {
            CheckState::Unchecked
        };
        let row = draw_row(
            ui,
            RowConfig {
                depth,
                expanded: Some(!is_collapsed),
                leading: Some(icons::FOLDER),
                leading_color: Some(muted()),
                label: dir_name,
                label_color: Some(muted()),
                is_active: false,
                active_bar: false,
                badge: None,
                trailing_count: 0,
                tree_guides: false,
                checkbox: Some(check),
            },
        );
        if row.checkbox_clicked {
            let mut paths = Vec::new();
            collect_paths(child, &mut paths);
            if all_staged {
                unstage_paths.extend(paths);
            } else {
                stage_paths.extend(paths);
            }
        } else if row.main_clicked {
            *toggle_dir = Some(key.clone());
        }
        if !is_collapsed {
            render_change_node(
                ui,
                child,
                &child_prefix,
                depth + 1,
                collapsed,
                unstage_paths,
                stage_paths,
                open_diff,
                toggle_dir,
            );
        }
    }
    for (file_name, change) in &node.files {
        let (glyph, glyph_color) = match change.status {
            git::ChangeStatus::Added => ("A", ADD),
            git::ChangeStatus::Modified => ("M", accent()),
            git::ChangeStatus::Deleted => ("D", DEL),
            git::ChangeStatus::Renamed => ("R", accent()),
            git::ChangeStatus::Untracked => ("?", WARN),
        };
        let rename_label;
        let label: &str = if let Some(old) = change.old_path.as_ref() {
            let old_name = std::path::Path::new(old)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(old);
            rename_label = format!("{old_name} -> {file_name}");
            &rename_label
        } else {
            file_name
        };
        let check = if change.has_staged && !change.has_unstaged {
            CheckState::Checked
        } else if change.has_staged && change.has_unstaged {
            CheckState::Indeterminate
        } else {
            CheckState::Unchecked
        };
        let row = draw_row(
            ui,
            RowConfig {
                depth,
                expanded: None,
                leading: Some(glyph),
                leading_color: Some(glyph_color),
                label,
                label_color: None,
                is_active: false,
                active_bar: false,
                badge: None,
                trailing_count: 0,
                tree_guides: false,
                checkbox: Some(check),
            },
        );
        if row.checkbox_clicked {
            if change.has_staged && !change.has_unstaged {
                unstage_paths.push(change.path.clone());
            } else {
                stage_paths.push(change.path.clone());
            }
        } else if row.main_clicked {
            *open_diff = Some(change.path.clone());
        }
        let change_path = change.path.clone();
        let has_staged = change.has_staged;
        let has_unstaged = change.has_unstaged;
        row.response.context_menu(|ui| {
            if has_unstaged {
                if ui.button(format!("{}  Stage", icons::PLUS)).clicked() {
                    stage_paths.push(change_path.clone());
                    ui.close();
                }
            }
            if has_staged {
                if ui.button(format!("{}  Unstage", icons::MINUS)).clicked() {
                    unstage_paths.push(change_path.clone());
                    ui.close();
                }
            }
            if ui.button(format!("{}  Open Diff", icons::GIT_DIFF)).clicked() {
                *open_diff = Some(change_path.clone());
                ui.close();
            }
            ui.separator();
            if ui.button(format!("{}  Copy Path", icons::COPY)).clicked() {
                ui.ctx().copy_text(change_path.clone());
                ui.close();
            }
        });
    }
}

/// Walk a `DirNode` and return `(all_staged, any_staged)`.
/// `all_staged` = every file is fully staged (has_staged && !has_unstaged).
/// `any_staged` = at least one file has staged changes.
fn dir_staged_state(node: &DirNode) -> (bool, bool) {
    let mut total = 0usize;
    let mut fully_staged = 0usize;
    let mut any_staged = false;
    fn walk(n: &DirNode, total: &mut usize, fully_staged: &mut usize, any_staged: &mut bool) {
        for child in n.dirs.values() {
            walk(child, total, fully_staged, any_staged);
        }
        for (_, change) in &n.files {
            *total += 1;
            if change.has_staged && !change.has_unstaged {
                *fully_staged += 1;
            }
            if change.has_staged {
                *any_staged = true;
            }
        }
    }
    walk(node, &mut total, &mut fully_staged, &mut any_staged);
    (total > 0 && fully_staged == total, any_staged)
}

fn render_files(ui: &mut egui::Ui, app: &mut App) {
    let path = match app.active_workspace_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_row(ui, "No active worktree");
            return;
        }
    };
    // Build a map of relative-path → (ChangeStatus, has_staged, has_unstaged)
    // so file rows can show git status colors. Directories get the "worst"
    // status of any descendant.
    let git_status_map: HashMap<String, (git::ChangeStatus, bool, bool)> =
        app.active_workspace_mut()
            .and_then(|w| w.git_status.as_ref())
            .map(|s| {
                s.changes
                    .iter()
                    .map(|c| {
                        let status = c
                            .unstaged_status
                            .or(c.staged_status)
                            .unwrap_or(c.status);
                        (c.path.clone(), (status, c.has_staged, c.has_unstaged))
                    })
                    .collect()
            })
            .unwrap_or_default();

    let mut opened: Option<PathBuf> = None;
    let mut toggled: Option<PathBuf> = None;
    let mut new_entry: Option<(PathBuf, NewEntryKind)> = None;
    let mut delete_request: Option<PathBuf> = None;
    let mut drop_request: Option<(PathBuf, PathBuf)> = None;
    let mut commit_pending = false;
    let mut cancel_pending = false;
    let selected_snapshot = app.selected_file.clone();
    egui::ScrollArea::vertical()
        .id_salt("right_files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_fs_dir(
                ui,
                &path,
                0,
                &app.expanded_dirs,
                &mut opened,
                &mut toggled,
                &mut new_entry,
                &mut delete_request,
                &mut drop_request,
                selected_snapshot.as_deref(),
                app.pending_new_entry.as_mut(),
                &mut commit_pending,
                &mut cancel_pending,
                &path,
                &git_status_map,
            );
            // Sink for right-clicks on the empty space below entries
            // — `interact` claims the rest of the ScrollArea's height
            // so a context menu can fire even when no row sits under
            // the cursor. New entries created here go into the
            // workspace root.
            let avail = ui.available_size_before_wrap();
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(avail.x.max(1.0), avail.y.max(20.0)),
                egui::Sense::click(),
            );
            let _ = rect;
            let root_for_menu = path.clone();
            resp.context_menu(|ui| {
                if ui.button(format!("{}  New File…", icons::FILE)).clicked() {
                    new_entry = Some((root_for_menu.clone(), NewEntryKind::File));
                    ui.close();
                }
                if ui
                    .button(format!("{}  New Folder…", icons::FOLDER_PLUS))
                    .clicked()
                {
                    new_entry = Some((root_for_menu.clone(), NewEntryKind::Folder));
                    ui.close();
                }
            });
        });
    if cancel_pending {
        app.pending_new_entry = None;
    } else if commit_pending {
        try_commit_pending(app);
    }
    if let Some(p) = opened.as_ref() {
        // Clicking a row also marks it as the active selection so
        // Cmd+Delete (handled in shortcuts.rs) targets it.
        app.selected_file = Some(p.clone());
    }
    if let Some(p) = delete_request {
        app.pending_delete_file = Some(crate::state::PendingDeleteFile { path: p });
    }
    if let Some((src, dst_dir)) = drop_request {
        move_path(app, &src, &dst_dir);
    }
    if let Some(p) = toggled
        && !app.expanded_dirs.remove(&p) {
            app.expanded_dirs.insert(p);
        }
    if let Some(p) = opened {
        let path_str = p.to_string_lossy().to_string();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string();
        let content = std::fs::read_to_string(&p).unwrap_or_default();
        let ctx = ui.ctx().clone();
        app.open_file_into_active_layout(&ctx, path_str, name, content);
    }
    if let Some((parent, kind)) = new_entry {
        // Make sure the parent is expanded so the inline editor row
        // is visible immediately under it.
        app.expanded_dirs.insert(parent.clone());
        app.pending_new_entry = Some(PendingNewEntry {
            parent,
            kind,
            name: String::new(),
            error: None,
            focused_once: false,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_fs_dir(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    depth: usize,
    expanded: &std::collections::HashSet<PathBuf>,
    open_file: &mut Option<PathBuf>,
    toggle_dir: &mut Option<PathBuf>,
    new_entry: &mut Option<(PathBuf, NewEntryKind)>,
    delete_request: &mut Option<PathBuf>,
    drop_request: &mut Option<(PathBuf, PathBuf)>,
    selected: Option<&std::path::Path>,
    pending: Option<&mut PendingNewEntry>,
    commit: &mut bool,
    cancel: &mut bool,
    workspace_root: &std::path::Path,
    git_status_map: &HashMap<String, (git::ChangeStatus, bool, bool)>,
) {
    if depth > 6 {
        return;
    }
    // Pending-entry editor lives in exactly one directory at a time
    // (`pending.parent`). Split the `&mut` so children recursions
    // don't see it for unrelated subdirs — only the matching dir
    // renders the inline TextEdit row.
    let (pending_here, mut pending_for_children): (
        Option<&mut PendingNewEntry>,
        Option<&mut PendingNewEntry>,
    ) = match pending {
        Some(p) if p.parent == path => (Some(p), None),
        other => (None, other),
    };
    if let Some(p) = pending_here {
        render_pending_editor_row(ui, depth, p, commit, cancel);
    }
    let read = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| {
        (
            !e.path().is_dir(),
            e.file_name().to_string_lossy().to_string(),
        )
    });
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | "target" | "node_modules" | ".DS_Store"
        ) {
            continue;
        }
        let entry_path = e.path();
        let is_dir = entry_path.is_dir();
        let is_expanded = is_dir && expanded.contains(&entry_path);
        let is_selected = selected.is_some_and(|s| s == entry_path);
        // Resolve git status for this file/directory.
        let rel = entry_path.strip_prefix(workspace_root)
            .ok()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string());
        let git_info = rel.as_deref()
            .and_then(|r| git_status_map.get(r));
        // For directories, look for any descendant with changes
        let dir_has_changes = is_dir && git_status_map.keys().any(|k| {
            k.starts_with(rel.as_deref().unwrap_or(""))
                && k != rel.as_deref().unwrap_or("")
        });
        let (leading_icon, leading_col, label_col) = if is_dir {
            let col = if dir_has_changes { MODIFIED_BLUE } else { muted() };
            (icons::FOLDER, col, None)
        } else if let Some((status, _staged, _unstaged)) = git_info {
            (status_glyph(*status), status_color(*status), Some(status_color(*status)))
        } else {
            (icons::FILE, muted(), None)
        };
        let row = draw_row(
            ui,
            RowConfig {
                depth,
                expanded: if is_dir { Some(is_expanded) } else { None },
                leading: Some(leading_icon),
                leading_color: Some(leading_col),
                label: &name,
                label_color: label_col,
                is_active: is_selected,
                active_bar: false,
                badge: None,
                trailing_count: 0,
                        tree_guides: false, checkbox: None,
            },
        );
        if row.main_clicked {
            if is_dir {
                *toggle_dir = Some(entry_path.clone());
            } else {
                *open_file = Some(entry_path.clone());
            }
        }
        // Drag source: any row can be dragged. dnd_set_drag_payload
        // attaches the path to egui's global drag state; pointer
        // release on a folder row reads it back as a drop target.
        if row.response.dragged() {
            row.response.dnd_set_drag_payload(entry_path.clone());
        }
        // Drop target: only directories accept drops. We refuse a
        // drop when the source is the directory itself (no-op) or a
        // descendant of itself (would loop the filesystem). The
        // actual rename is deferred to the caller via `drop_request`.
        if is_dir {
            if let Some(payload) = row.response.dnd_release_payload::<PathBuf>() {
                let src: PathBuf = (*payload).clone();
                let same = src == entry_path;
                let into_self_or_descendant = entry_path.starts_with(&src);
                if !same && !into_self_or_descendant {
                    *drop_request = Some((src, entry_path.clone()));
                }
            }
        }
        let path_owned = entry_path.clone();
        // New entries land in the directory itself for folder rows,
        // and in the file's parent directory for file rows — same
        // affordance as VS Code so right-clicking any nearby row
        // creates the entry next to it.
        let create_parent: PathBuf = if is_dir {
            path_owned.clone()
        } else {
            path_owned
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| workspace_root.to_path_buf())
        };
        row.response.context_menu(|ui| {
            if !is_dir && ui.button(format!("{}  Open", icons::FILE)).clicked() {
                *open_file = Some(path_owned.clone());
                ui.close();
            }
            if ui.button(format!("{}  New File…", icons::FILE)).clicked() {
                *new_entry = Some((create_parent.clone(), NewEntryKind::File));
                ui.close();
            }
            if ui
                .button(format!("{}  New Folder…", icons::FOLDER_PLUS))
                .clicked()
            {
                *new_entry = Some((create_parent.clone(), NewEntryKind::Folder));
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{}  Reveal in File Manager", icons::FOLDER_OPEN))
                .clicked()
            {
                reveal_in_file_manager(&path_owned);
                ui.close();
            }
            if ui.button(format!("{}  Copy Path", icons::COPY)).clicked() {
                ui.ctx().copy_text(path_owned.to_string_lossy().to_string());
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{}  Move to Trash", icons::TRASH))
                .clicked()
            {
                *delete_request = Some(path_owned.clone());
                ui.close();
            }
        });
        if is_dir && is_expanded {
            let pending_reborrow = pending_for_children.as_deref_mut();
            render_fs_dir(
                ui,
                &entry_path,
                depth + 1,
                expanded,
                open_file,
                toggle_dir,
                new_entry,
                delete_request,
                drop_request,
                selected,
                pending_reborrow,
                commit,
                cancel,
                workspace_root,
                git_status_map,
            );
        }
    }
}

/// Move a path into a target directory via `std::fs::rename`. Only
/// works for same-filesystem moves; cross-filesystem (e.g. user
/// dragged a file from a mounted volume) returns EXDEV and we
/// surface the error instead of silently copying — copy-then-delete
/// across filesystems would change the move's atomicity guarantees.
/// Refuses to overwrite an existing entry at the destination.
fn move_path(app: &mut App, src: &std::path::Path, dst_dir: &std::path::Path) {
    let Some(name) = src.file_name() else { return; };
    let dst = dst_dir.join(name);
    if dst.exists() {
        app.git_error = Some(format!(
            "`{}` already exists in {}",
            name.to_string_lossy(),
            dst_dir.display()
        ));
        return;
    }
    if let Err(e) = std::fs::rename(src, &dst) {
        app.git_error = Some(format!("Move: {e}"));
        return;
    }
    if app.selected_file.as_deref() == Some(src) {
        app.selected_file = Some(dst.clone());
    }
    app.rename_file_tabs_for_path(src, &dst);
    app.expanded_dirs.insert(dst_dir.to_path_buf());
    push_file_op(
        app,
        FileOp::Move {
            from: src.to_path_buf(),
            to: dst,
        },
    );
}

/// Push an op onto the LIFO undo stack, evicting the oldest when
/// at capacity.
fn push_file_op(app: &mut App, op: FileOp) {
    if app.file_op_history.len() >= FILE_OP_HISTORY_CAP {
        app.file_op_history.pop_front();
    }
    app.file_op_history.push_back(op);
}

/// Inline TextEdit row for the pending new-file/folder editor.
/// Looks like a tree row at the right indent, no leading expander.
/// Enter commits via `*commit = true`, Escape cancels via
/// `*cancel = true`. Focus loss with empty input also cancels —
/// matches JetBrains.
fn render_pending_editor_row(
    ui: &mut egui::Ui,
    depth: usize,
    pending: &mut PendingNewEntry,
    commit: &mut bool,
    cancel: &mut bool,
) {
    let leading = match pending.kind {
        NewEntryKind::File => icons::FILE,
        NewEntryKind::Folder => icons::FOLDER,
    };
    let hint = match pending.kind {
        NewEntryKind::File => "filename.ext",
        NewEntryKind::Folder => "folder-name",
    };
    let indent = (depth as f32 + 1.0) * 14.0;
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.label(egui::RichText::new(leading).color(muted()));
        let edit_id = egui::Id::new(("pending_new_entry_edit", depth));
        let resp = ui.add(
            egui::TextEdit::singleline(&mut pending.name)
                .id(edit_id)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
        if !pending.focused_once {
            resp.request_focus();
            pending.focused_once = true;
        }
        // Enter on a singleline TextEdit drops focus first, so the
        // right detection is `lost_focus() + key_pressed(Enter)`.
        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if escape_pressed {
            *cancel = true;
        } else if resp.lost_focus() && enter_pressed {
            if pending.name.trim().is_empty() {
                *cancel = true;
            } else {
                *commit = true;
            }
        } else if resp.lost_focus() && pending.name.trim().is_empty() {
            // Clicked away with no name typed → cancel (JetBrains).
            *cancel = true;
        }
    });
    if let Some(err) = &pending.error {
        ui.horizontal(|ui| {
            ui.add_space(indent + 18.0);
            ui.label(
                egui::RichText::new(err)
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 100, 100)),
            );
        });
    }
}

/// Try to materialize the pending entry. On success, clears the
/// pending state. On failure, populates `error` so the inline row
/// displays it under the input.
fn try_commit_pending(app: &mut App) {
    let Some(pending) = app.pending_new_entry.as_ref() else {
        return;
    };
    let name = pending.name.trim().to_string();
    let parent = pending.parent.clone();
    let kind = pending.kind;
    if name.is_empty() {
        return;
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        if let Some(p) = app.pending_new_entry.as_mut() {
            p.error = Some("Name can't contain `/`, `\\`, `.`, or `..`".into());
        }
        return;
    }
    let target = parent.join(&name);
    if target.exists() {
        if let Some(p) = app.pending_new_entry.as_mut() {
            p.error = Some(format!("`{}` already exists", name));
        }
        return;
    }
    let result = match kind {
        NewEntryKind::File => std::fs::File::create(&target).map(|_| ()),
        NewEntryKind::Folder => std::fs::create_dir(&target),
    };
    match result {
        Ok(()) => {
            app.expanded_dirs.insert(parent);
            app.pending_new_entry = None;
        }
        Err(e) => {
            if let Some(p) = app.pending_new_entry.as_mut() {
                p.error = Some(format!("Couldn't create: {e}"));
                // Re-focus the editor so the user can fix and retry
                // without an extra click.
                p.focused_once = false;
            }
        }
    }
}


use crate::git::{self, FileChange};
use crate::state::{App, RightTab};
use crate::ui::util::{
    draw_row, section_header,
    RowConfig, accent, muted, text,
};
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;
use std::collections::BTreeMap;
use std::path::PathBuf;


const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);
const WARN: Color32 = Color32::from_rgb(220, 180, 110);

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

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!("{}  {}", icons::GIT_BRANCH, status.branch))
                .color(muted())
                .size(11.5),
        );
    });
    ui.add_space(4.0);

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
            let staged: Vec<&FileChange> = status.changes.iter().filter(|c| c.staged).collect();
            let unstaged: Vec<&FileChange> = status
                .changes
                .iter()
                .filter(|c| !c.staged && c.status != git::ChangeStatus::Untracked)
                .collect();
            let untracked: Vec<&FileChange> = status
                .changes
                .iter()
                .filter(|c| c.status == git::ChangeStatus::Untracked)
                .collect();

            if !staged.is_empty() {
                section_header(ui, "STAGED");
                render_change_tree(
                    ui,
                    "stg",
                    &staged,
                    true,
                    &collapsed,
                    &mut unstage_paths,
                    &mut stage_paths,
                    &mut open_diff,
                    &mut toggle_dir,
                );
            }
            if !unstaged.is_empty() {
                section_header(ui, "UNSTAGED");
                render_change_tree(
                    ui,
                    "unstg",
                    &unstaged,
                    false,
                    &collapsed,
                    &mut unstage_paths,
                    &mut stage_paths,
                    &mut open_diff,
                    &mut toggle_dir,
                );
            }
            if !untracked.is_empty() {
                section_header(ui, "UNTRACKED");
                render_change_tree(
                    ui,
                    "untr",
                    &untracked,
                    false,
                    &collapsed,
                    &mut unstage_paths,
                    &mut stage_paths,
                    &mut open_diff,
                    &mut toggle_dir,
                );
            }

            if status.changes.is_empty() {
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

    let staged_count = status.changes.iter().filter(|c| c.staged).count();
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
    if text_resp.has_focus() {
        let submit = footer_ui.input(|i| {
            i.key_pressed(egui::Key::Enter)
                && (i.modifiers.command || i.modifiers.mac_cmd)
        });
        if submit && can_commit {
            do_commit(app, &repo_path, false);
        }
    }

    footer_ui.add_space(8.0);

    let mut action_commit = false;
    let mut action_commit_push = false;
    let mut action_push = false;
    let mut action_pull = false;

    let row_w = footer_ui.available_width();
    let menu_w = 32.0;
    let gap = 6.0;
    let primary_w = row_w - menu_w - gap;

    footer_ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.scope(|ui| {
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
            ui.add_enabled_ui(can_commit, |ui| {
                let r = ui.add(
                    egui::Button::new(
                        RichText::new(format!("{}  Commit", icons::CHECK))
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

        let menu_resp = ui.add(
            egui::Button::new(RichText::new(icons::CARET_DOWN).size(12.0))
                .min_size(egui::vec2(menu_w, 30.0))
                .corner_radius(egui::CornerRadius::same(6)),
        );
        egui::Popup::menu(&menu_resp).show(|ui| {
            ui.set_min_width(180.0);
            let commit_btn = egui::Button::new(
                RichText::new(format!("{}  Commit", icons::CHECK)).size(12.0),
            )
            .min_size(egui::vec2(ui.available_width(), 24.0));
            if ui.add_enabled(can_commit, commit_btn).clicked() {
                action_commit = true;
            }
            let commit_push = egui::Button::new(
                RichText::new(format!("{}  Commit & Push", icons::ARROW_UP))
                    .size(12.0),
            )
            .min_size(egui::vec2(ui.available_width(), 24.0));
            if ui.add_enabled(can_commit, commit_push).clicked() {
                action_commit_push = true;
            }
            ui.separator();
            let push_btn = egui::Button::new(
                RichText::new(format!("{}  Push", icons::ARROW_UP)).size(12.0),
            )
            .min_size(egui::vec2(ui.available_width(), 24.0));
            if ui.add(push_btn).clicked() {
                action_push = true;
            }
            let pull_btn = egui::Button::new(
                RichText::new(format!("{}  Pull", icons::ARROW_DOWN)).size(12.0),
            )
            .min_size(egui::vec2(ui.available_width(), 24.0));
            if ui.add(pull_btn).clicked() {
                action_pull = true;
            }
        });
    });

    if let Some(err) = &app.git_error {
        footer_ui.add_space(6.0);
        footer_ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(err).color(DEL).size(11.0));
        });
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
    if action_commit {
        do_commit(app, &repo_path, false);
    } else if action_commit_push {
        do_commit(app, &repo_path, true);
    } else if action_push {
        do_push(app, &repo_path);
    } else if action_pull {
        match git::pull(&repo_path) {
            Ok(()) => {
                app.git_error = None;
                force_status_refresh(app);
            }
            Err(e) => app.git_error = Some(e),
        }
    }
}

fn do_commit(app: &mut App, repo: &std::path::Path, then_push: bool) {
    let msg = app.commit_message.trim().to_string();
    if msg.is_empty() {
        app.git_error = Some("Commit message is empty".into());
        return;
    }
    match git::commit(repo, &msg) {
        Ok(()) => {
            app.commit_message.clear();
            app.git_error = None;
            force_status_refresh(app);
            if then_push {
                do_push(app, repo);
            }
        }
        Err(e) => app.git_error = Some(e),
    }
}

fn do_push(app: &mut App, repo: &std::path::Path) {
    match git::push(repo) {
        Ok(()) => {
            app.git_error = None;
            force_status_refresh(app);
        }
        Err(e) => app.git_error = Some(e),
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
    section: &str,
    changes: &[&FileChange],
    staged: bool,
    collapsed: &std::collections::HashSet<String>,
    unstage_paths: &mut Vec<String>,
    stage_paths: &mut Vec<String>,
    open_diff: &mut Option<String>,
    toggle_dir: &mut Option<String>,
) {
    let tree = build_tree(changes);
    render_change_node(
        ui,
        section,
        &tree,
        "",
        0,
        staged,
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
    section: &str,
    node: &DirNode,
    prefix: &str,
    depth: usize,
    staged: bool,
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
        if node.dirs.len() == 1 && child.files.is_empty() && !child.dirs.is_empty() {
            render_change_node(
                ui,
                section,
                child,
                &child_prefix,
                depth,
                staged,
                collapsed,
                unstage_paths,
                stage_paths,
                open_diff,
                toggle_dir,
            );
            continue;
        }
        let key = format!("{section}:{child_prefix}");
        let is_collapsed = collapsed.contains(&key);
        // Folder checkbox mirrors the section: in a STAGED tree it's
        // always checked (since every file under it is staged), in an
        // UNSTAGED / UNTRACKED tree it's always unchecked. Click flips
        // every file in the subtree.
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
                checkbox: Some(staged),
            },
        );
        if row.checkbox_clicked {
            let mut paths = Vec::new();
            collect_paths(child, &mut paths);
            if staged {
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
                section,
                child,
                &child_prefix,
                depth + 1,
                staged,
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
        // For renames show "oldName → newName" on a single leaf so the
        // destination folder groups the row but the move is still
        // visible. Pure filename for everything else.
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
                checkbox: Some(staged),
            },
        );
        if row.checkbox_clicked {
            if staged {
                unstage_paths.push(change.path.clone());
            } else {
                stage_paths.push(change.path.clone());
            }
        } else if row.main_clicked {
            *open_diff = Some(change.path.clone());
        }
        // Right-click → stage / unstage / open diff / copy path.
        let change_path = change.path.clone();
        let staged_here = staged;
        row.response.context_menu(|ui| {
            if staged_here {
                if ui.button(format!("{}  Unstage", icons::MINUS)).clicked() {
                    unstage_paths.push(change_path.clone());
                    ui.close();
                }
            } else {
                if ui.button(format!("{}  Stage", icons::PLUS)).clicked() {
                    stage_paths.push(change_path.clone());
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

fn render_files(ui: &mut egui::Ui, app: &mut App) {
    let path = match app.active_workspace_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_row(ui, "No active worktree");
            return;
        }
    };
    let mut opened: Option<PathBuf> = None;
    let mut toggled: Option<PathBuf> = None;
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
            );
        });
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
}

fn render_fs_dir(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    depth: usize,
    expanded: &std::collections::HashSet<PathBuf>,
    open_file: &mut Option<PathBuf>,
    toggle_dir: &mut Option<PathBuf>,
) {
    if depth > 6 {
        return;
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
        let row = draw_row(
            ui,
            RowConfig {
                depth,
                expanded: if is_dir { Some(is_expanded) } else { None },
                leading: Some(if is_dir { icons::FOLDER } else { icons::FILE }),
                leading_color: Some(muted()),
                label: &name,
                label_color: None,
                is_active: false,
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
        let path_owned = entry_path.clone();
        row.response.context_menu(|ui| {
            if !is_dir && ui.button(format!("{}  Open", icons::FILE)).clicked() {
                *open_file = Some(path_owned.clone());
                ui.close();
            }
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
        });
        if is_dir && is_expanded {
            render_fs_dir(ui, &entry_path, depth + 1, expanded, open_file, toggle_dir);
        }
    }
}

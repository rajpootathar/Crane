use crate::git::{self, FileChange};
use crate::state::{App, RightTab};
use crate::ui_util::{
    draw_row, draw_trailing, section_header,
    RowConfig, accent, muted, text,
};
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const WIDTH: f32 = 300.0;

const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);
const WARN: Color32 = Color32::from_rgb(220, 180, 110);

pub fn render(ui: &mut egui::Ui, app: &mut App) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        tab_chip(ui, "Changes", app.right_tab == RightTab::Changes, || {
            app.right_tab = RightTab::Changes;
        });
        ui.add_space(4.0);
        tab_chip(ui, "Files", app.right_tab == RightTab::Files, || {
            app.right_tab = RightTab::Files;
        });
    });
    ui.add_space(6.0);
    ui.painter().line_segment(
        [
            egui::pos2(ui.min_rect().min.x, ui.cursor().min.y),
            egui::pos2(ui.min_rect().max.x, ui.cursor().min.y),
        ],
        egui::Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
    );
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

    let mut stage_path: Option<String> = None;
    let mut unstage_path: Option<String> = None;
    let mut open_diff: Option<String> = None;
    let mut toggle_dir: Option<String> = None;
    let collapsed = app.collapsed_change_dirs.clone();

    egui::ScrollArea::vertical()
        .id_salt("right_changes")
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 160.0)
        .show(ui, |ui| {
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
                    &mut unstage_path,
                    &mut stage_path,
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
                    &mut unstage_path,
                    &mut stage_path,
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
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                    &mut toggle_dir,
                );
            }

            if status.changes.is_empty() {
                dim_row(ui, "working tree clean");
            }
        });

    ui.add_space(6.0);
    ui.painter().line_segment(
        [
            egui::pos2(ui.min_rect().min.x, ui.cursor().min.y),
            egui::pos2(ui.min_rect().max.x, ui.cursor().min.y),
        ],
        egui::Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
    );
    ui.add_space(8.0);

    let staged_count = status.changes.iter().filter(|c| c.staged).count();
    let has_staged = staged_count > 0;
    let has_message = !app.commit_message.trim().is_empty();
    let can_commit = has_staged && has_message;

    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let text_resp = ui.add(
            egui::TextEdit::multiline(&mut app.commit_message)
                .hint_text("Commit message")
                .desired_rows(2)
                .desired_width(WIDTH - 28.0)
                .font(egui::FontId::new(12.0, egui::FontFamily::Proportional)),
        );
        if text_resp.has_focus() {
            let submit = ui.input(|i| {
                i.key_pressed(egui::Key::Enter)
                    && (i.modifiers.command || i.modifiers.mac_cmd)
            });
            if submit && can_commit {
                do_commit(app, &repo_path, false);
            }
        }
    });

    ui.add_space(6.0);

    let mut action_commit = false;
    let mut action_commit_push = false;
    let mut action_push = false;
    let mut action_pull = false;

    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 1.0;

        let width = ui.available_width() - 20.0;
        let primary_w = width - 30.0;

        ui.scope(|ui| {
            ui.add_enabled_ui(can_commit, |ui| {
                let r = ui.add(
                    egui::Button::new(
                        RichText::new(format!("{}  Commit", icons::CHECK)).size(12.5),
                    )
                    .min_size(egui::vec2(primary_w, 28.0)),
                );
                if r.clicked() {
                    action_commit = true;
                }
            });
        });

        let menu_resp = ui.add(
            egui::Button::new(RichText::new(icons::CARET_DOWN).size(12.0))
                .min_size(egui::vec2(30.0, 28.0)),
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
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(err).color(DEL).size(11.0));
        });
    }
    ui.add_space(8.0);

    if let Some(dir) = toggle_dir
        && !app.collapsed_change_dirs.remove(&dir) {
            app.collapsed_change_dirs.insert(dir);
        }
    if let Some(path) = stage_path {
        match git::stage(&repo_path, &path) {
            Ok(()) => {
                app.git_error = None;
                force_status_refresh(app);
            }
            Err(e) => app.git_error = Some(e),
        }
    }
    if let Some(path) = unstage_path {
        match git::unstage(&repo_path, &path) {
            Ok(()) => {
                app.git_error = None;
                force_status_refresh(app);
            }
            Err(e) => app.git_error = Some(e),
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
        ws.open_or_replace_diff(
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
    unstage_path: &mut Option<String>,
    stage_path: &mut Option<String>,
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
        unstage_path,
        stage_path,
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
    unstage_path: &mut Option<String>,
    stage_path: &mut Option<String>,
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
                unstage_path,
                stage_path,
                open_diff,
                toggle_dir,
            );
            continue;
        }
        let key = format!("{section}:{child_prefix}");
        let is_collapsed = collapsed.contains(&key);
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
            },
        );
        if row.main_clicked {
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
                unstage_path,
                stage_path,
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
        let row = draw_row(
            ui,
            RowConfig {
                depth,
                expanded: None,
                leading: Some(glyph),
                leading_color: Some(glyph_color),
                label: file_name,
                label_color: None,
                is_active: false,
                active_bar: false,
                badge: None,
                trailing_count: 1,
            },
        );
        let trailing_icon = if staged { icons::MINUS } else { icons::PLUS };
        let trailing_tip = if staged { "Unstage" } else { "Stage" };
        let flags = draw_trailing(
            ui,
            row.rect,
            row.hovered,
            &[(trailing_icon, trailing_tip, 0)],
        );
        if flags[0] {
            if staged {
                *unstage_path = Some(change.path.clone());
            } else {
                *stage_path = Some(change.path.clone());
            }
        } else if row.main_clicked {
            *open_diff = Some(change.path.clone());
        }
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
            },
        );
        if row.main_clicked {
            if is_dir {
                *toggle_dir = Some(entry_path.clone());
            } else {
                *open_file = Some(entry_path.clone());
            }
        }
        if is_dir && is_expanded {
            render_fs_dir(ui, &entry_path, depth + 1, expanded, open_file, toggle_dir);
        }
    }
}

use crate::git::{self, FileChange};
use crate::state::{App, RightTab};
use crate::ui_util::{
    draw_row, draw_trailing, full_width_primary_button, ghost_button, section_header,
    RowConfig, ACCENT, MUTED, TEXT,
};
use crate::workspace::{DiffPane, Dir, PaneContent};
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
    let color = if active { TEXT } else { MUTED };
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
                    egui::Stroke::new(2.0, ACCENT),
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
    let repo_path = match app.active_worktree_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_row(ui, "No active worktree");
            return;
        }
    };
    let status = match app.active_worktree_mut().and_then(|w| w.git_status.clone()) {
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
                .color(MUTED)
                .size(11.5),
        );
    });
    ui.add_space(4.0);

    let mut stage_path: Option<String> = None;
    let mut unstage_path: Option<String> = None;
    let mut open_diff: Option<String> = None;

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
                    &staged,
                    true,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                );
            }
            if !unstaged.is_empty() {
                section_header(ui, "UNSTAGED");
                render_change_tree(
                    ui,
                    &unstaged,
                    false,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                );
            }
            if !untracked.is_empty() {
                section_header(ui, "UNTRACKED");
                render_change_tree(
                    ui,
                    &untracked,
                    false,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
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
    ui.add_space(6.0);
    section_header(ui, "COMMIT");
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.add(
            egui::TextEdit::multiline(&mut app.commit_message)
                .hint_text("message")
                .desired_rows(2)
                .desired_width(WIDTH - 24.0),
        );
    });
    ui.add_space(4.0);
    let mut commit_clicked = false;
    let mut push_clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        commit_clicked = full_width_primary_button(
            ui,
            Some(icons::CHECK),
            "Commit",
            "Commit staged changes",
        )
        .clicked();
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        push_clicked = ghost_button(
            ui,
            Some(icons::ARROW_UP),
            "Push",
            "Push to origin",
        )
        .clicked();
    });
    if let Some(err) = &app.git_error {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(err).color(DEL).size(11.0));
        });
    }
    ui.add_space(6.0);

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
    if commit_clicked {
        let msg = app.commit_message.trim().to_string();
        if msg.is_empty() {
            app.git_error = Some("Commit message is empty".into());
        } else {
            match git::commit(&repo_path, &msg) {
                Ok(()) => {
                    app.commit_message.clear();
                    app.git_error = None;
                    force_status_refresh(app);
                }
                Err(e) => app.git_error = Some(e),
            }
        }
    }
    if push_clicked {
        match git::push(&repo_path) {
            Ok(()) => app.git_error = None,
            Err(e) => app.git_error = Some(e),
        }
    }
}

fn force_status_refresh(app: &mut App) {
    if let Some(wt) = app.active_worktree_mut() {
        wt.last_status_refresh = None;
    }
}

fn open_file_diff(app: &mut App, repo: &std::path::Path, rel_path: &str) {
    let full = repo.join(rel_path);
    let right_text = std::fs::read_to_string(&full).unwrap_or_default();
    let left_text = git::head_content(repo, rel_path);
    if let Some(ws) = app.active_workspace() {
        ws.add_pane(
            PaneContent::Diff(DiffPane {
                left_path: format!("HEAD:{rel_path}"),
                right_path: rel_path.to_string(),
                left_text,
                right_text,
                left_buf: String::new(),
                right_buf: String::new(),
                error: None,
            }),
            Some(Dir::Horizontal),
        );
        if let Some(focus) = ws.focus {
            if let Some(p) = ws.panes.get_mut(&focus) {
                p.title = format!(
                    "diff: {}",
                    std::path::Path::new(rel_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(rel_path)
                );
            }
        }
    }
}

fn dim_row(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text).color(MUTED).size(11.5));
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

fn render_change_tree(
    ui: &mut egui::Ui,
    changes: &[&FileChange],
    staged: bool,
    unstage_path: &mut Option<String>,
    stage_path: &mut Option<String>,
    open_diff: &mut Option<String>,
) {
    let tree = build_tree(changes);
    render_change_node(
        ui,
        &tree,
        "",
        0,
        staged,
        unstage_path,
        stage_path,
        open_diff,
    );
}

fn render_change_node(
    ui: &mut egui::Ui,
    node: &DirNode,
    prefix: &str,
    depth: usize,
    staged: bool,
    unstage_path: &mut Option<String>,
    stage_path: &mut Option<String>,
    open_diff: &mut Option<String>,
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
                child,
                &child_prefix,
                depth,
                staged,
                unstage_path,
                stage_path,
                open_diff,
            );
            continue;
        }
        draw_row(
            ui,
            RowConfig {
                depth,
                expanded: Some(true),
                leading: Some(icons::FOLDER),
                leading_color: Some(MUTED),
                label: dir_name,
                label_color: Some(MUTED),
                is_active: false,
                active_bar: false,
                badge: None,
                trailing_count: 0,
            },
        );
        render_change_node(
            ui,
            child,
            &child_prefix,
            depth + 1,
            staged,
            unstage_path,
            stage_path,
            open_diff,
        );
    }
    for (file_name, change) in &node.files {
        let (glyph, glyph_color) = match change.status {
            git::ChangeStatus::Added => ("A", ADD),
            git::ChangeStatus::Modified => ("M", ACCENT),
            git::ChangeStatus::Deleted => ("D", DEL),
            git::ChangeStatus::Renamed => ("R", ACCENT),
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
        if row.main_clicked {
            *open_diff = Some(change.path.clone());
        }
        if flags[0] {
            if staged {
                *unstage_path = Some(change.path.clone());
            } else {
                *stage_path = Some(change.path.clone());
            }
        }
    }
}

fn render_files(ui: &mut egui::Ui, app: &mut App) {
    let path = match app.active_worktree_path() {
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
    if let Some(p) = toggled {
        if !app.expanded_dirs.remove(&p) {
            app.expanded_dirs.insert(p);
        }
    }
    if let Some(p) = opened {
        let path_str = p.to_string_lossy().to_string();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string();
        let content = std::fs::read_to_string(&p).unwrap_or_default();
        if let Some(ws) = app.active_workspace() {
            ws.open_file_in_files_pane(path_str, name, content);
        }
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
                leading_color: Some(MUTED),
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

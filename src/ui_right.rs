use crate::git::{self, FileChange};
use crate::state::{App, RightTab};
use crate::workspace::{DiffPane, Dir, PaneContent};
use egui::{Color32, RichText};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const WIDTH: f32 = 280.0;

const HEADER: Color32 = Color32::from_rgb(180, 184, 196);
const DIM: Color32 = Color32::from_rgb(130, 136, 150);
const TEXT: Color32 = Color32::from_rgb(200, 204, 220);
const ADD: Color32 = Color32::from_rgb(110, 200, 130);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);
const UNTRACKED: Color32 = Color32::from_rgb(220, 180, 110);
const HOVER_BG: Color32 = Color32::from_rgb(30, 34, 48);

pub fn render(ui: &mut egui::Ui, app: &mut App) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let is_changes = app.right_tab == RightTab::Changes;
        let is_files = app.right_tab == RightTab::Files;
        if ui
            .selectable_label(is_changes, RichText::new("Changes").size(12.5))
            .clicked()
        {
            app.right_tab = RightTab::Changes;
        }
        ui.add_space(4.0);
        if ui
            .selectable_label(is_files, RichText::new("Files").size(12.5))
            .clicked()
        {
            app.right_tab = RightTab::Files;
        }
    });
    ui.add_space(4.0);
    ui.separator();

    match app.right_tab {
        RightTab::Changes => render_changes(ui, app),
        RightTab::Files => render_files(ui, app),
    }
}

fn render_changes(ui: &mut egui::Ui, app: &mut App) {
    let repo_path = match app.active_worktree_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_label(ui, "No active worktree");
            return;
        }
    };
    let status = match app.active_worktree_mut().and_then(|w| w.git_status.clone()) {
        Some(s) => s,
        None => {
            dim_label(ui, "(not a git repo)");
            return;
        }
    };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(
            RichText::new(format!("⎇ {}", status.branch))
                .color(DIM)
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
                render_tree(
                    ui,
                    "stg",
                    &staged,
                    true,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                );
            }
            if !unstaged.is_empty() {
                section_header(ui, "UNSTAGED");
                render_tree(
                    ui,
                    "unstg",
                    &unstaged,
                    false,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                );
            }
            if !untracked.is_empty() {
                section_header(ui, "UNTRACKED");
                render_tree(
                    ui,
                    "untr",
                    &untracked,
                    false,
                    &mut unstage_path,
                    &mut stage_path,
                    &mut open_diff,
                );
            }

            if status.changes.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("working tree clean").color(DIM).size(11.5));
                });
            }
        });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new("COMMIT").size(10.5).color(HEADER).strong());
    });
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.add(
            egui::TextEdit::multiline(&mut app.commit_message)
                .hint_text("message")
                .desired_rows(2)
                .desired_width(WIDTH - 24.0),
        );
    });
    ui.add_space(2.0);
    let mut commit_clicked = false;
    let mut push_clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        commit_clicked = ui
            .add(egui::Button::new(RichText::new("Commit").strong()))
            .clicked();
        push_clicked = ui.button("Push").clicked();
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

fn section_header(ui: &mut egui::Ui, label: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(label).size(10.5).color(HEADER).strong());
    });
}

fn dim_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(text).color(DIM).size(11.5));
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

fn render_tree(
    ui: &mut egui::Ui,
    salt: &str,
    changes: &[&FileChange],
    staged: bool,
    unstage_path: &mut Option<String>,
    stage_path: &mut Option<String>,
    open_diff: &mut Option<String>,
) {
    let tree = build_tree(changes);
    render_dir_node(ui, salt, &tree, "", 0, staged, unstage_path, stage_path, open_diff);
}

#[allow(clippy::too_many_arguments)]
fn render_dir_node(
    ui: &mut egui::Ui,
    salt: &str,
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
            render_dir_node(
                ui,
                salt,
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
        ui.horizontal(|ui| {
            ui.add_space(10.0 + depth as f32 * 12.0);
            ui.label(
                RichText::new(format!("▾ {}", dir_name))
                    .size(11.5)
                    .color(DIM),
            );
        });
        render_dir_node(
            ui,
            salt,
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
        ui.push_id((salt, &change.path), |ui| {
            let row = ui
                .horizontal(|ui| {
                    ui.add_space(10.0 + depth as f32 * 12.0);
                    if staged {
                        if ui
                            .small_button(RichText::new("−").color(DIM))
                            .on_hover_text("Unstage")
                            .clicked()
                        {
                            *unstage_path = Some(change.path.clone());
                        }
                    } else if ui
                        .small_button(RichText::new("+").color(DIM))
                        .on_hover_text("Stage")
                        .clicked()
                    {
                        *stage_path = Some(change.path.clone());
                    }
                    let glyph_color = match change.status {
                        git::ChangeStatus::Added => ADD,
                        git::ChangeStatus::Deleted => DEL,
                        git::ChangeStatus::Untracked => UNTRACKED,
                        _ => TEXT,
                    };
                    ui.label(
                        RichText::new(change.status.glyph())
                            .color(glyph_color)
                            .size(11.0)
                            .monospace(),
                    );
                    let name_response = ui.add(
                        egui::Label::new(RichText::new(file_name).size(11.5).color(TEXT))
                            .sense(egui::Sense::click()),
                    );
                    if name_response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if name_response.clicked() {
                        *open_diff = Some(change.path.clone());
                    }
                    name_response
                })
                .inner;
            if row.hovered() {
                let painter = ui.painter();
                painter.rect_filled(row.rect.expand2(egui::vec2(0.0, 1.0)), 0.0, HOVER_BG);
            }
        });
    }
}

fn render_files(ui: &mut egui::Ui, app: &mut App) {
    let path = match app.active_worktree_path() {
        Some(p) => p.to_path_buf(),
        None => {
            dim_label(ui, "No active worktree");
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
        let glyph = if is_dir {
            if is_expanded { "▾" } else { "▸" }
        } else {
            "·"
        };
        let color = if is_dir { TEXT } else { Color32::from_rgb(170, 176, 190) };
        let response = ui
            .horizontal(|ui| {
                ui.add_space(10.0 + depth as f32 * 12.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{glyph}  {}", name))
                            .color(color)
                            .size(11.5),
                    )
                    .sense(egui::Sense::click()),
                )
            })
            .inner;
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
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

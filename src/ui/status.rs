//! Global status bar pinned to the bottom of the window. Bound to the
//! active Workspace: shows the worktree's git branch on the left and the
//! workspace-relative path of the active file on the right. Per-file
//! concerns (diagnostics, cursor, indent, language) live in the Files
//! pane's own status strip — this bar is Workspace chrome.

use crate::state::layout::PaneContent;
use crate::state::App;
use crate::theme;
use egui::RichText;
use egui_phosphor::regular as icons;

pub const HEIGHT: f32 = 24.0;

pub fn render(ui: &mut egui::Ui, app: &mut App) {
    let t = theme::current();
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0.0, t.topbar_bg.to_color32());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y),
        ],
        egui::Stroke::new(1.0, t.divider.to_color32()),
    );

    let branch = app.active_repo_branch();
    let active_path = active_file_path(app);

    ui.allocate_ui_with_layout(
        rect.size(),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(10.0);
            if let Some(b) = branch {
                let resp = ui.add(
                    egui::Label::new(
                        RichText::new(format!("{}  {}", icons::GIT_BRANCH, b))
                            .size(11.0)
                            .color(t.text.to_color32()),
                    )
                    .sense(egui::Sense::click()),
                );
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    app.branch_picker.open = !app.branch_picker.open;
                    app.branch_picker.query.clear();
                    if app.branch_picker.open {
                        load_branch_picker(app, ui.ctx());
                        app.branch_picker.opened_at = Some(std::time::Instant::now());
                    } else {
                        app.branch_picker.opened_at = None;
                    }
                }
            }

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.add_space(10.0);
                    // Settings + Help pinned to the bar's far right.
                    // Frameless buttons so they blend with the bar.
                    let mk = |glyph: &'static str| {
                        egui::Button::new(
                            RichText::new(glyph)
                                .size(13.0)
                                .color(t.text_muted.to_color32()),
                        )
                        .frame(false)
                        .min_size(egui::vec2(22.0, 20.0))
                    };
                    if ui
                        .add(mk(icons::QUESTION))
                        .on_hover_text("Keyboard shortcuts")
                        .clicked()
                    {
                        app.show_help = !app.show_help;
                    }
                    if ui
                        .add(mk(icons::GEAR))
                        .on_hover_text("Settings")
                        .clicked()
                    {
                        app.show_settings = !app.show_settings;
                    }
                    ui.add_space(4.0);
                    // Divider between chrome buttons and the path.
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(1.0, 14.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().line_segment(
                        [rect.left_top(), rect.left_bottom()],
                        egui::Stroke::new(1.0, t.divider.to_color32()),
                    );
                    ui.add_space(6.0);
                    if let Some(path) = &active_path {
                        let shown = relative_to_workspace(app, path);
                        ui.label(
                            RichText::new(shown)
                                .size(11.0)
                                .color(t.text_muted.to_color32()),
                        );
                    }
                },
            );
        },
    );
}

/// Spawn a worker to discover repos + list their branches off the UI
/// thread. Results flow back via `App::branch_picker_rx`, which the
/// picker drains each frame. A monorepo with many submodules means
/// 1 + 2·N git subprocesses — this used to hitch the UI noticeably.
pub fn load_branch_picker(app: &mut App, ctx: &egui::Context) {
    let Some(ws) = app.active_workspace_path().map(|p| p.to_path_buf()) else {
        return;
    };
    app.branch_picker.loading = true;
    app.branch_picker.repos.clear();
    let (tx, rx) = std::sync::mpsc::channel();
    app.branch_picker.rx = Some(rx);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let roots = crate::git::discover_repos(&ws, 5);
        let data: Vec<_> = roots
            .into_iter()
            .map(|r| {
                let locals = crate::git::list_local_branches(&r);
                let remotes = crate::git::list_remote_branches(&r);
                (r, locals, remotes)
            })
            .collect();
        let _ = tx.send(data);
        ctx.request_repaint();
    });
}

/// Drain the worker's result once it finishes. Called once per picker
/// frame — non-blocking.
pub fn poll_branch_picker(app: &mut App) {
    let Some(rx) = app.branch_picker.rx.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok(data) => {
            let active = app.active_repo_root();
            app.branch_picker.repos = data;
            app.branch_picker.filter = active.filter(|a| {
                app.branch_picker.repos.iter().any(|(r, _, _)| r == a)
            });
            app.branch_picker.loading = false;
            app.branch_picker.rx = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.branch_picker.loading = false;
            app.branch_picker.rx = None;
        }
    }
}

fn active_file_path(app: &App) -> Option<String> {
    let layout = app.active_layout_ref()?;
    let focus = layout.focus;
    if let Some(id) = focus
        && let Some(p) = layout.panes.get(&id)
        && let PaneContent::Files(files) = &p.content
        && let Some(t) = files.tabs.get(files.active)
    {
        return Some(t.path.clone());
    }
    for (_, p) in &layout.panes {
        if let PaneContent::Files(files) = &p.content
            && let Some(t) = files.tabs.get(files.active)
        {
            return Some(t.path.clone());
        }
    }
    None
}

fn relative_to_workspace(app: &App, path: &str) -> String {
    if let Some(root) = app.active_workspace_path()
        && let Ok(rel) = std::path::Path::new(path).strip_prefix(root)
    {
        return rel.to_string_lossy().to_string();
    }
    path.to_string()
}

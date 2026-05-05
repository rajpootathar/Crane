mod details;
mod log;
mod refs;

use egui::{Color32, Pos2, Rect, Stroke};
use egui_phosphor::regular as icons;

use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

const HEADER_H: f32 = 28.0;

/// Effects bubbled up from the bottom-region render so the caller
/// (in main's render path) can apply them with `&mut App`.
#[derive(Default)]
pub struct ViewEffect {
    /// User clicked the close (×) button — caller should flip
    /// `tab.git_log_visible = false`.
    pub close: bool,
    /// User clicked a file in the details column — caller should open
    /// a Diff Pane in the active Layout for `(commit_sha, file_path)`.
    pub open_diff: Option<(String, std::path::PathBuf)>,
    /// User picked an item from the commit-row right-click menu.
    pub op: Option<crate::git_log::state::GitLogOp>,
    /// User confirmed the inline branch-from-commit prompt with a
    /// non-empty name. `(sha, branch_name)`.
    pub branch_from: Option<(String, String)>,
}

/// Render the Git Log bottom region inside `region`. Mutates `state`
/// (worker poll, header chrome, selection). `repo` is the active
/// workspace's repo path — used by the details column to fetch the
/// list of changed files for the selected commit.
pub fn render(
    ui: &mut egui::Ui,
    region: Rect,
    state: &mut GitLogState,
    repo: &std::path::Path,
) -> ViewEffect {
    let mut effect = ViewEffect::default();
    let mut request_close = false;
    state.poll_worker();
    state.maybe_reload(repo.to_path_buf(), ui.ctx());

    ui.painter()
        .rect_filled(region, 0.0, Color32::from_rgb(20, 22, 28));

    // Header strip
    let header = Rect::from_min_max(
        region.min,
        Pos2::new(region.max.x, region.min.y + HEADER_H),
    );
    let mut header_ui = ui.new_child(egui::UiBuilder::new().max_rect(header));
    header_ui.set_clip_rect(header);
    header_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Git Log").strong());
        ui.add_space(8.0);

        if state.is_loading() {
            ui.spinner();
            ui.label(
                egui::RichText::new("loading…")
                    .small()
                    .color(muted()),
            );
        } else if let Some(frame) = state.frame.as_ref() {
            ui.label(
                egui::RichText::new(format!("{} commits", frame.commits.len()))
                    .small()
                    .color(muted()),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui
                .button(icons::X)
                .on_hover_text("Close (Cmd+9)")
                .clicked()
            {
                request_close = true;
            }
            ui.add_space(4.0);
            if ui
                .button(icons::ARROW_COUNTER_CLOCKWISE)
                .on_hover_text("Refresh")
                .clicked()
            {
                state.worker_rx = None;
                state.reload(repo.to_path_buf(), ui.ctx());
            }
            ui.add_space(4.0);
            if state.is_fetching() {
                ui.spinner();
            } else if ui
                .button(icons::DOWNLOAD_SIMPLE)
                .on_hover_text("Fetch all (git fetch --all --prune --tags)")
                .clicked()
            {
                state.fetch_all(repo.to_path_buf(), ui.ctx());
            }
        });
    });

    // Body region — three columns separated by draggable splitters
    // with 6px hit areas. Each side column is collapsible to a thin
    // strip carrying just an expand chevron so the user can quickly
    // get more horizontal room for the log without losing the column.
    let body = Rect::from_min_max(
        Pos2::new(region.min.x, region.min.y + HEADER_H),
        region.max,
    );
    ui.painter().rect_stroke(
        body,
        0.0,
        Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
        egui::epaint::StrokeKind::Inside,
    );

    let refs_snapshot = state.frame.as_ref().map(|f| f.refs.clone());
    let head_snapshot = state
        .frame
        .as_ref()
        .and_then(|f| f.refs.head.clone());

    const SPLIT_W: f32 = 6.0;
    const COLLAPSED_W: f32 = 22.0;
    const MIN_COL_W: f32 = 140.0;
    const MIN_LOG_W: f32 = 240.0;

    let body_left = body.min.x;
    let body_right = body.max.x;
    let body_top = body.min.y;
    let body_bottom = body.max.y;

    let refs_w = if state.col_refs_collapsed {
        COLLAPSED_W
    } else {
        state.col_refs_width
    };
    let details_w = if state.col_details_collapsed {
        COLLAPSED_W
    } else {
        state.col_details_width
    };

    let refs_rect = Rect::from_min_max(
        Pos2::new(body_left, body_top),
        Pos2::new(body_left + refs_w, body_bottom),
    );
    let split1_rect = Rect::from_min_max(
        Pos2::new(refs_rect.max.x, body_top),
        Pos2::new(refs_rect.max.x + SPLIT_W, body_bottom),
    );
    let details_rect = Rect::from_min_max(
        Pos2::new(body_right - details_w, body_top),
        Pos2::new(body_right, body_bottom),
    );
    let split2_rect = Rect::from_min_max(
        Pos2::new(details_rect.min.x - SPLIT_W, body_top),
        Pos2::new(details_rect.min.x, body_bottom),
    );
    let log_rect = Rect::from_min_max(
        Pos2::new(split1_rect.max.x, body_top),
        Pos2::new(split2_rect.min.x, body_bottom),
    );

    // Splitter 1: refs ↔ log. Drag adjusts col_refs_width when not
    // collapsed; collapsing/expanding handled by the chevron in the
    // column's own toolbar (drawn below).
    if !state.col_refs_collapsed {
        let resp1 = ui.interact(
            split1_rect,
            egui::Id::new("git_log_split1"),
            egui::Sense::drag(),
        );
        if resp1.hovered() || resp1.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp1.dragged() {
            let max_refs = (body.width() - details_w - MIN_LOG_W - SPLIT_W * 2.0).max(MIN_COL_W);
            state.col_refs_width =
                (state.col_refs_width + resp1.drag_delta().x).clamp(MIN_COL_W, max_refs);
        }
    }
    ui.painter()
        .rect_filled(split1_rect, 0.0, Color32::from_rgb(36, 40, 52));

    // Splitter 2: log ↔ details. Same shape, mirrored.
    if !state.col_details_collapsed {
        let resp2 = ui.interact(
            split2_rect,
            egui::Id::new("git_log_split2"),
            egui::Sense::drag(),
        );
        if resp2.hovered() || resp2.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp2.dragged() {
            let max_details = (body.width() - refs_w - MIN_LOG_W - SPLIT_W * 2.0).max(MIN_COL_W);
            state.col_details_width =
                (state.col_details_width - resp2.drag_delta().x).clamp(MIN_COL_W, max_details);
        }
    }
    ui.painter()
        .rect_filled(split2_rect, 0.0, Color32::from_rgb(36, 40, 52));

    // Refs column.
    {
        let mut col_ui = ui.new_child(egui::UiBuilder::new().max_rect(refs_rect));
        col_ui.set_clip_rect(refs_rect);
        if state.col_refs_collapsed {
            // Vertical strip: just the expand chevron.
            col_ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                if ui
                    .button(icons::CARET_RIGHT)
                    .on_hover_text("Expand refs panel")
                    .clicked()
                {
                    state.col_refs_collapsed = false;
                }
            });
        } else {
            col_ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("REFS")
                        .small()
                        .color(muted())
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(2.0);
                    if ui
                        .small_button(icons::CARET_LEFT)
                        .on_hover_text("Collapse")
                        .clicked()
                    {
                        state.col_refs_collapsed = true;
                    }
                });
            });
            refs::render(
                &mut col_ui,
                refs_snapshot.as_ref(),
                head_snapshot.as_deref(),
                &mut state.filter,
            );
        }
    }

    // Log column.
    {
        let mut col_ui = ui.new_child(egui::UiBuilder::new().max_rect(log_rect));
        col_ui.set_clip_rect(log_rect);
        log::render(&mut col_ui, state);
    }

    // Details column.
    {
        let mut col_ui = ui.new_child(egui::UiBuilder::new().max_rect(details_rect));
        col_ui.set_clip_rect(details_rect);
        if state.col_details_collapsed {
            col_ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                if ui
                    .button(icons::CARET_LEFT)
                    .on_hover_text("Expand details panel")
                    .clicked()
                {
                    state.col_details_collapsed = false;
                }
            });
        } else {
            col_ui.horizontal(|ui| {
                ui.add_space(4.0);
                if ui
                    .small_button(icons::CARET_RIGHT)
                    .on_hover_text("Collapse")
                    .clicked()
                {
                    state.col_details_collapsed = true;
                }
                ui.label(
                    egui::RichText::new("DETAILS")
                        .small()
                        .color(muted())
                        .strong(),
                );
            });
            let cb = details::render(&mut col_ui, state, repo);
            if let Some(req) = cb.open_diff {
                effect.open_diff = Some(req);
            }
        }
    }

    effect.close = request_close;
    if let Some(op) = state.pending_op.take() {
        effect.op = Some(op);
    }

    // Inline branch-from-commit prompt. Floats above the body region.
    if let Some((sha, name)) = state.pending_branch_prompt.as_ref().cloned() {
        let prompt_w = 320.0;
        let prompt_h = 90.0;
        let prompt_rect = Rect::from_center_size(
            region.center(),
            egui::vec2(prompt_w, prompt_h),
        );
        ui.painter().rect_filled(
            prompt_rect.expand(3.0),
            6.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 100),
        );
        ui.painter().rect_filled(
            prompt_rect,
            6.0,
            Color32::from_rgb(28, 32, 42),
        );
        ui.painter().rect_stroke(
            prompt_rect,
            6.0,
            Stroke::new(1.0, Color32::from_rgb(80, 92, 130)),
            egui::epaint::StrokeKind::Inside,
        );
        let mut prompt_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(prompt_rect.shrink2(egui::vec2(12.0, 10.0))),
        );
        prompt_ui.set_clip_rect(prompt_rect);
        prompt_ui.label(
            egui::RichText::new(format!(
                "Create branch from {}",
                sha.chars().take(7).collect::<String>()
            ))
            .strong(),
        );
        prompt_ui.add_space(6.0);
        let mut buf = name;
        let resp = prompt_ui.add(
            egui::TextEdit::singleline(&mut buf)
                .hint_text("new branch name")
                .desired_width(prompt_w - 24.0),
        );
        resp.request_focus();
        let enter = resp.lost_focus()
            && prompt_ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = prompt_ui.input(|i| i.key_pressed(egui::Key::Escape));
        prompt_ui.add_space(6.0);
        prompt_ui.horizontal(|ui| {
            let create = ui.button("Create").clicked() || enter;
            let cancel = ui.button("Cancel").clicked() || esc;
            if create && !buf.trim().is_empty() {
                effect.branch_from = Some((sha.clone(), buf.trim().to_string()));
                state.pending_branch_prompt = None;
            } else if cancel {
                state.pending_branch_prompt = None;
            } else {
                state.pending_branch_prompt = Some((sha.clone(), buf));
            }
        });
    }

    effect
}

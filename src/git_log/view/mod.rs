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

    // Body region
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

    // Clone the refs snapshot (small) so the left column's
    // refs::render can hold an &mut on state.filter without
    // conflicting with the immut borrow on state.frame.
    let refs_snapshot = state.frame.as_ref().map(|f| f.refs.clone());
    let head_snapshot = state
        .frame
        .as_ref()
        .and_then(|f| f.refs.head.clone());
    let col_refs_w = state.col_refs_width;
    let col_details_w = state.col_details_width;

    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    body_ui.set_clip_rect(body);
    body_ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(
            egui::vec2(col_refs_w, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                refs::render(
                    ui,
                    refs_snapshot.as_ref(),
                    head_snapshot.as_deref(),
                    &mut state.filter,
                );
            },
        );
        ui.separator();

        let mid_w = (body.width() - col_refs_w - col_details_w - 24.0).max(160.0);
        ui.allocate_ui_with_layout(
            egui::vec2(mid_w, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                log::render(ui, state);
            },
        );

        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(col_details_w, body.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let cb = details::render(ui, state, repo);
                if let Some(req) = cb.open_diff {
                    effect.open_diff = Some(req);
                }
            },
        );
    });

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

use std::path::{Path, PathBuf};

use egui::{Color32, Sense};

use crate::git_log::state::GitLogState;
use crate::ui::util::muted;

/// Result returned by `render` so the caller can open a Diff Pane
/// for the clicked file.
#[derive(Default)]
pub struct DetailsCallback {
    pub open_diff: Option<(String, PathBuf)>,
}

pub fn render(ui: &mut egui::Ui, state: &mut GitLogState, repo: &Path) -> DetailsCallback {
    let mut cb = DetailsCallback::default();

    let Some(frame) = state.frame.as_ref() else {
        return cb;
    };
    let Some(sha) = state.selected_commit.clone() else {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Select a commit").color(muted()));
        return cb;
    };
    let Some(commit) = frame.commits.iter().find(|c| c.sha == sha) else {
        return cb;
    };

    let subject = commit.subject.clone();
    let author = commit.author.clone();
    let date = commit.date.clone();
    let short = sha.chars().take(12).collect::<String>();

    egui::ScrollArea::vertical()
        .id_salt("git_log_details")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&subject).strong().size(13.0));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("{}  ·  {}", author, date))
                    .small()
                    .color(muted()),
            );
            ui.add_space(2.0);
            let copy_resp = ui.add(
                egui::Label::new(
                    egui::RichText::new(&short)
                        .small()
                        .color(muted())
                        .monospace(),
                )
                .sense(Sense::click()),
            );
            if copy_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if copy_resp.clicked() {
                ui.ctx().copy_text(sha.clone());
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            let files = crate::git::commit_files(repo, &sha);
            if files.is_empty() {
                ui.label(egui::RichText::new("(no files)").small().color(muted()));
            }
            for (status, path) in files {
                let status_color = match status {
                    'A' => Color32::from_rgb(102, 187, 106),
                    'M' => Color32::from_rgb(255, 202, 40),
                    'D' => Color32::from_rgb(239, 83, 80),
                    'R' => Color32::from_rgb(66, 165, 245),
                    _ => muted(),
                };
                let is_selected = state.selected_file.as_deref() == Some(path.as_path());
                let row = ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(status.to_string())
                            .color(status_color)
                            .monospace()
                            .strong(),
                    );
                    let label_resp = ui.add(
                        egui::Label::new(
                            egui::RichText::new(path.to_string_lossy())
                                .size(12.0)
                                .color(if is_selected {
                                    Color32::from_rgb(220, 225, 232)
                                } else {
                                    Color32::from_rgb(180, 188, 200)
                                }),
                        )
                        .sense(Sense::click()),
                    );
                    label_resp
                });
                let label_resp = row.inner;
                if label_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if label_resp.clicked() {
                    state.selected_file = Some(path.clone());
                    cb.open_diff = Some((sha.clone(), path));
                }
            }
        });

    cb
}

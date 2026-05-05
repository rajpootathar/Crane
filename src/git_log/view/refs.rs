use egui::{Color32, Sense};
use egui_phosphor::regular as icons;

use crate::git_log::refs::{RefEntry, RefSet, WorktreeEntry};
use crate::ui::util::muted;

const HEADER_COLOR: Color32 = Color32::from_rgb(140, 146, 162);

/// Render the Local / Remote / Tags / Worktrees groups inside the
/// left column. `refs` is None while the first load is in flight.
pub fn render(ui: &mut egui::Ui, refs: Option<&RefSet>, head: Option<&str>) {
    egui::ScrollArea::vertical()
        .id_salt("git_log_refs")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(refs) = refs else {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("loading…").small().color(muted()));
                return;
            };

            ref_section(ui, "LOCAL", &refs.local, head, &|n| {
                n.trim_start_matches("refs/heads/").to_string()
            });
            ref_section(ui, "REMOTE", &refs.remote, head, &|n| {
                n.trim_start_matches("refs/remotes/").to_string()
            });
            ref_section(ui, "TAGS", &refs.tags, head, &|n| {
                n.trim_start_matches("refs/tags/").to_string()
            });
            wt_section(ui, "WORKTREES", &refs.worktrees);
        });
}

fn ref_section(
    ui: &mut egui::Ui,
    title: &str,
    entries: &[RefEntry],
    head: Option<&str>,
    strip: &dyn Fn(&str) -> String,
) {
    if entries.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title)
            .color(HEADER_COLOR)
            .size(10.5)
            .strong(),
    );
    for e in entries {
        let display = strip(&e.name);
        let is_head = head.is_some_and(|h| h == e.sha);
        let prefix = if is_head {
            format!("{}  ", icons::ASTERISK)
        } else {
            format!("{}  ", icons::GIT_BRANCH)
        };
        let mut text = egui::RichText::new(format!("{prefix}{display}")).size(12.5);
        if is_head {
            text = text.strong();
        }
        let resp = ui.add(egui::Label::new(text).sense(Sense::click()));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
}

fn wt_section(ui: &mut egui::Ui, title: &str, entries: &[WorktreeEntry]) {
    if entries.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title)
            .color(HEADER_COLOR)
            .size(10.5)
            .strong(),
    );
    for w in entries {
        let folder = w
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| w.path.to_string_lossy().to_string());
        let label = format!("{}  {}  ·  {}", icons::FOLDER, w.branch, folder);
        let resp = ui.add(
            egui::Label::new(egui::RichText::new(label).size(12.5))
                .sense(Sense::click()),
        );
        let _ = resp.on_hover_text(w.path.to_string_lossy().to_string());
    }
}

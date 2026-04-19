//! Status bar pinned to the bottom of the window. Shows the active
//! file's diagnostics summary, language, and the relative path under
//! the worktree — the app-wide equivalent of VSCode's status bar.

use crate::layout::PaneContent;
use crate::state::App;
use crate::theme;
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;

pub const HEIGHT: f32 = 24.0;

pub fn render(ui: &mut egui::Ui, app: &App) {
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

    let (active_path, active_lang) = active_file_info(app);
    let counts = active_path
        .as_deref()
        .map(|p| diag_counts(app, p))
        .unwrap_or((0, 0, 0));

    // All file-related info is right-aligned and grouped: diagnostics →
    // language → path. Reads from right-to-left visually since that's
    // where the user's attention lands for the active file.
    ui.allocate_ui_with_layout(
        rect.size(),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.add_space(10.0);
            if let Some(path) = &active_path {
                let shown = relative_to_workspace(app, path);
                ui.label(
                    RichText::new(shown)
                        .size(11.0)
                        .color(t.text_muted.to_color32()),
                );
                ui.add_space(12.0);
            }
            if let Some(lang) = active_lang {
                ui.label(
                    RichText::new(lang)
                        .size(11.0)
                        .color(t.text_muted.to_color32())
                        .monospace(),
                );
                ui.add_space(12.0);
            }
            let (errs, warns, infos) = counts;
            // Order when laid out right-to-left: info, warning, error
            // (so on-screen it reads error, warning, info — matching the
            // inline diag strip's ordering).
            ui.label(
                RichText::new(format!("{}  {infos}", icons::INFO))
                    .size(11.5)
                    .color(if infos > 0 {
                        t.accent.to_color32()
                    } else {
                        t.text_muted.to_color32()
                    }),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{}  {warns}", icons::WARNING))
                    .size(11.5)
                    .color(if warns > 0 {
                        Color32::from_rgb(226, 192, 80)
                    } else {
                        t.text_muted.to_color32()
                    }),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{}  {errs}", icons::X_CIRCLE))
                    .size(11.5)
                    .color(if errs > 0 {
                        t.error.to_color32()
                    } else {
                        t.text_muted.to_color32()
                    }),
            );
        },
    );
}

fn active_file_info(app: &App) -> (Option<String>, Option<String>) {
    let layout = app.active_layout_ref();
    let Some(layout) = layout else {
        return (None, None);
    };
    let focus = layout.focus;
    // Prefer the focused pane; fall back to the first Files pane with an
    // active tab.
    if let Some(id) = focus
        && let Some(p) = layout.panes.get(&id)
        && let PaneContent::Files(files) = &p.content
        && let Some(t) = files.tabs.get(files.active)
    {
        return (Some(t.path.clone()), language_for_path(&t.path));
    }
    for (_, p) in &layout.panes {
        if let PaneContent::Files(files) = &p.content
            && let Some(t) = files.tabs.get(files.active)
        {
            return (Some(t.path.clone()), language_for_path(&t.path));
        }
    }
    (None, None)
}

fn language_for_path(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path).extension()?.to_str()?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "Rust",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "py" => "Python",
        "go" => "Go",
        "css" => "CSS",
        "scss" => "SCSS",
        "html" | "htm" => "HTML",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "md" | "markdown" => "Markdown",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        _ => ext,
    }.to_string())
}

fn diag_counts(app: &App, path: &str) -> (usize, usize, usize) {
    let mut e = 0usize;
    let mut w = 0usize;
    let mut i = 0usize;
    for d in app.lsp.diagnostics(std::path::Path::new(path)) {
        match d.severity {
            1 => e += 1,
            2 => w += 1,
            _ => i += 1,
        }
    }
    (e, w, i)
}

fn relative_to_workspace(app: &App, path: &str) -> String {
    if let Some(root) = app.active_workspace_path()
        && let Ok(rel) = std::path::Path::new(path).strip_prefix(root)
    {
        return rel.to_string_lossy().to_string();
    }
    path.to_string()
}

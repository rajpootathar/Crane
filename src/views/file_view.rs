use crate::workspace::FilesPane;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}
fn themes() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

pub fn render(
    ui: &mut egui::Ui,
    pane_id: u64,
    pane: &mut FilesPane,
    font_size: f32,
    title: &mut String,
) {
    ui.push_id(("files_pane", pane_id), |ui| {
        render_inner(ui, pane, font_size, title);
    });
}

fn render_inner(ui: &mut egui::Ui, pane: &mut FilesPane, font_size: f32, title: &mut String) {
    let tab_count = pane.tabs.len();

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let mut close_idx: Option<usize> = None;
        let mut activate_idx: Option<usize> = None;
        for (idx, tab) in pane.tabs.iter().enumerate() {
            let is_active = idx == pane.active;
            ui.push_id(("file_tab", idx), |ui| {
                let label = RichText::new(&tab.name)
                    .size(11.5)
                    .color(if is_active {
                        Color32::from_rgb(200, 204, 220)
                    } else {
                        Color32::from_rgb(130, 136, 150)
                    });
                if ui.selectable_label(is_active, label).clicked() {
                    activate_idx = Some(idx);
                }
                if ui
                    .small_button(RichText::new("×").size(11.0).color(Color32::from_rgb(130, 136, 150)))
                    .on_hover_text("Close file")
                    .clicked()
                {
                    close_idx = Some(idx);
                }
            });
            ui.add_space(2.0);
        }
        if ui
            .small_button(RichText::new("+").size(11.0))
            .on_hover_text("Open another file by path")
            .clicked()
        {
            pane.active = tab_count;
        }
        if let Some(idx) = activate_idx {
            pane.active = idx;
        }
        if let Some(idx) = close_idx {
            pane.close(idx);
        }
    });
    ui.separator();

    if pane.tabs.is_empty() || pane.active >= pane.tabs.len() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Path:");
            let response = ui.text_edit_singleline(&mut pane.input_buf);
            let load = ui.button("Open").clicked()
                || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if load {
                let path = pane.input_buf.trim().to_string();
                if path.is_empty() {
                    pane.error = Some("Path is empty".into());
                } else {
                    match std::fs::read_to_string(&path) {
                        Ok(content) => {
                            let name = Path::new(&path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&path)
                                .to_string();
                            pane.open(path, content, name.clone());
                            pane.input_buf.clear();
                            pane.error = None;
                            *title = format!("Files · {name}");
                        }
                        Err(e) => pane.error = Some(e.to_string()),
                    }
                }
            }
        });
        if let Some(err) = &pane.error {
            ui.colored_label(Color32::from_rgb(220, 100, 100), err);
        } else {
            ui.label(
                RichText::new("No files open. Enter a path or click a file from the sidebar.")
                    .color(Color32::from_rgb(130, 136, 150)),
            );
        }
        return;
    }

    let active = &pane.tabs[pane.active];
    *title = format!("Files · {}", active.name);

    ScrollArea::both()
        .id_salt(("file_scroll", pane.active))
        .auto_shrink([false; 2])
        .show(ui, |ui| render_highlighted(ui, &active.content, &active.path, font_size));
}

fn render_highlighted(ui: &mut egui::Ui, content: &str, path: &str, font_size: f32) {
    let syntaxes = syntaxes();
    let themes = themes();
    let theme = &themes.themes["base16-ocean.dark"];
    let syntax = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| syntaxes.find_syntax_by_extension(e))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());

    let mut h = HighlightLines::new(syntax, theme);
    let font = FontId::new(font_size, FontFamily::Monospace);

    for line in LinesWithEndings::from(content) {
        let ranges: Vec<(Style, &str)> = match h.highlight_line(line, syntaxes) {
            Ok(r) => r,
            Err(_) => vec![(Style::default(), line)],
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (style, text) in ranges {
                let color = Color32::from_rgb(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                );
                ui.label(
                    RichText::new(text.trim_end_matches('\n'))
                        .color(color)
                        .font(font.clone()),
                );
            }
        });
    }
}

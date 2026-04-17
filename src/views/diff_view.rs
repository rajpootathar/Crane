use crate::layout::DiffPane;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use similar::{ChangeTag, TextDiff};

const ADD_BG: Color32 = Color32::from_rgb(25, 55, 35);
const DEL_BG: Color32 = Color32::from_rgb(60, 28, 32);
const CTX_FG: Color32 = Color32::from_rgb(180, 186, 198);
const ADD_FG: Color32 = Color32::from_rgb(140, 220, 150);
const DEL_FG: Color32 = Color32::from_rgb(230, 130, 130);
const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
const HEADER: Color32 = Color32::from_rgb(200, 204, 220);

pub fn render(ui: &mut egui::Ui, pane: &mut DiffPane, font_size: f32, _title: &mut String) {
    if pane.left_text.is_empty() && pane.right_text.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No diff loaded")
                    .size(14.0)
                    .color(HEADER),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Click a changed file in the Changes sidebar to view its diff here.")
                    .size(11.5)
                    .color(MUTED),
            );
        });
        if let Some(err) = &pane.error {
            ui.add_space(8.0);
            ui.colored_label(DEL_FG, err);
        }
        return;
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new(&pane.left_path)
                .size(11.0)
                .color(DEL_FG)
                .monospace(),
        );
        ui.label(RichText::new("→").size(11.0).color(MUTED));
        ui.label(
            RichText::new(&pane.right_path)
                .size(11.0)
                .color(ADD_FG)
                .monospace(),
        );
    });
    ui.add_space(4.0);
    ui.separator();

    let diff = TextDiff::from_lines(&pane.left_text, &pane.right_text);
    let font = FontId::new(font_size, FontFamily::Monospace);

    ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
        for change in diff.iter_all_changes() {
            let (sign, fg, bg) = match change.tag() {
                ChangeTag::Delete => ("-", DEL_FG, Some(DEL_BG)),
                ChangeTag::Insert => ("+", ADD_FG, Some(ADD_BG)),
                ChangeTag::Equal => (" ", CTX_FG, None),
            };
            let text = format!("{sign} {}", change.value().trim_end_matches('\n'));
            let mut r = RichText::new(text).font(font.clone()).color(fg);
            if let Some(bg) = bg {
                r = r.background_color(bg);
            }
            ui.label(r);
        }
    });
}

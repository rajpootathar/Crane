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
    let left_lines = pane.left_text.lines().count().max(1);
    let right_lines = pane.right_text.lines().count().max(1);
    let ldigits = left_lines.to_string().len().max(3);
    let rdigits = right_lines.to_string().len().max(3);
    let char_w = ui
        .fonts_mut(|f| f.layout_no_wrap("0".to_string(), font.clone(), Color32::WHITE))
        .size()
        .x;
    let gutter_old_w = char_w * ldigits as f32 + 10.0;
    let gutter_new_w = char_w * rdigits as f32 + 10.0;
    let sign_w = char_w * 2.0 + 8.0;

    ScrollArea::both()
        .auto_shrink([false; 2])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            for change in diff.iter_all_changes() {
                let (sign, fg, bg) = match change.tag() {
                    ChangeTag::Delete => ("-", DEL_FG, DEL_BG),
                    ChangeTag::Insert => ("+", ADD_FG, ADD_BG),
                    ChangeTag::Equal => (" ", CTX_FG, Color32::TRANSPARENT),
                };
                let old_ln = change
                    .old_index()
                    .map(|i| format!("{:>w$}", i + 1, w = ldigits))
                    .unwrap_or_else(|| " ".repeat(ldigits));
                let new_ln = change
                    .new_index()
                    .map(|i| format!("{:>w$}", i + 1, w = rdigits))
                    .unwrap_or_else(|| " ".repeat(rdigits));
                let content = change.value().trim_end_matches('\n');
                row(
                    ui,
                    &font,
                    fg,
                    bg,
                    &old_ln,
                    &new_ln,
                    sign,
                    content,
                    gutter_old_w,
                    gutter_new_w,
                    sign_w,
                );
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    font: &FontId,
    fg: Color32,
    bg: Color32,
    old_ln: &str,
    new_ln: &str,
    sign: &str,
    content: &str,
    gutter_old_w: f32,
    gutter_new_w: f32,
    sign_w: f32,
) {
    let content_galley = ui.fonts_mut(|f| {
        f.layout_no_wrap(content.to_string(), font.clone(), fg)
    });
    let row_h = content_galley.size().y.max(font.size * 1.25);
    let content_w = content_galley.size().x;
    let total_w = gutter_old_w + gutter_new_w + sign_w + content_w + 8.0;
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
    let painter = ui.painter();
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, bg);
    }
    // Left gutter — muted
    painter.text(
        egui::pos2(rect.min.x + gutter_old_w - 4.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        old_ln,
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w - 4.0,
            rect.center().y,
        ),
        egui::Align2::RIGHT_CENTER,
        new_ln,
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w + sign_w / 2.0,
            rect.center().y,
        ),
        egui::Align2::CENTER_CENTER,
        sign,
        font.clone(),
        fg,
    );
    painter.galley(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w + sign_w,
            rect.min.y + (row_h - content_galley.size().y) / 2.0,
        ),
        content_galley,
        fg,
    );
}

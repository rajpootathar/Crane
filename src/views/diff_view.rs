use crate::workspace::DiffPane;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

const ADD_BG: Color32 = Color32::from_rgb(25, 55, 35);
const DEL_BG: Color32 = Color32::from_rgb(60, 28, 32);
const CTX_FG: Color32 = Color32::from_rgb(180, 186, 198);
const ADD_FG: Color32 = Color32::from_rgb(140, 220, 150);
const DEL_FG: Color32 = Color32::from_rgb(230, 130, 130);

pub fn render(ui: &mut egui::Ui, pane: &mut DiffPane, font_size: f32, title: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Left:");
        ui.text_edit_singleline(&mut pane.left_buf);
        ui.label("Right:");
        ui.text_edit_singleline(&mut pane.right_buf);
        if ui.button("Diff").clicked() {
            load_diff(pane, title);
        }
    });
    if let Some(err) = &pane.error {
        ui.colored_label(Color32::from_rgb(220, 100, 100), err);
        return;
    }
    if pane.left_text.is_empty() && pane.right_text.is_empty() {
        ui.label("Enter two file paths and press Diff.");
        return;
    }
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

fn load_diff(pane: &mut DiffPane, title: &mut String) {
    let lp = pane.left_buf.trim();
    let rp = pane.right_buf.trim();
    if lp.is_empty() || rp.is_empty() {
        pane.error = Some("Both paths required".into());
        return;
    }
    let left = match std::fs::read_to_string(lp) {
        Ok(s) => s,
        Err(e) => {
            pane.error = Some(format!("left: {e}"));
            return;
        }
    };
    let right = match std::fs::read_to_string(rp) {
        Ok(s) => s,
        Err(e) => {
            pane.error = Some(format!("right: {e}"));
            return;
        }
    };
    pane.left_path = lp.to_string();
    pane.right_path = rp.to_string();
    pane.left_text = left;
    pane.right_text = right;
    pane.error = None;
    let lname = Path::new(lp).file_name().and_then(|n| n.to_str()).unwrap_or(lp);
    let rname = Path::new(rp).file_name().and_then(|n| n.to_str()).unwrap_or(rp);
    *title = format!("{lname} ↔ {rname}");
}

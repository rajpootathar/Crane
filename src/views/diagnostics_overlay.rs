//! Paint LSP diagnostic underlines on top of the editor's galley. We
//! keep this out of the incremental highlighter so that a new stream of
//! diagnostics doesn't invalidate the cached syntax galley.

use crate::lsp::Diagnostic;
use crate::theme;
use egui::Color32;
use std::sync::Arc;

pub fn paint(
    ui: &egui::Ui,
    galley: &Arc<egui::Galley>,
    origin: egui::Pos2,
    diagnostics: &[Diagnostic],
) {
    if diagnostics.is_empty() {
        return;
    }
    let text = galley.text();

    // Precompute char-index of each line start once per paint. Was walking
    // the full text per diagnostic — O(text × diag) every frame — which
    // destroyed typing latency on files with many diagnostics.
    let mut line_char_starts: Vec<usize> = vec![0];
    let mut char_idx: usize = 0;
    for ch in text.chars() {
        char_idx += 1;
        if ch == '\n' {
            line_char_starts.push(char_idx);
        }
    }
    let total_chars = char_idx;
    let ccursor_at = |line: u32, col: u32| -> egui::text::CCursor {
        let Some(base) = line_char_starts.get(line as usize).copied() else {
            return egui::text::CCursor::new(total_chars);
        };
        let next = line_char_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(total_chars);
        let line_len = next.saturating_sub(base).saturating_sub(1);
        let col_clamped = (col as usize).min(line_len);
        egui::text::CCursor::new(base + col_clamped)
    };

    let painter = ui.painter();
    for d in diagnostics {
        let start_rect = galley.pos_from_cursor(ccursor_at(d.line, d.col_start));
        let end_rect = galley.pos_from_cursor(ccursor_at(d.line, d.col_end));
        let y = origin.y + start_rect.max.y - 1.0;
        let x0 = origin.x + start_rect.min.x;
        let x1 = origin.x + end_rect.max.x;
        if x1 <= x0 {
            continue;
        }
        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x1, y)],
            egui::Stroke::new(1.5, severity_color(d.severity)),
        );
    }
}

pub fn severity_color(severity: u8) -> Color32 {
    let t = theme::current();
    match severity {
        1 => t.error.to_color32(),
        2 => Color32::from_rgb(226, 192, 80),
        3 => t.accent.to_color32(),
        _ => t.text_muted.to_color32(),
    }
}

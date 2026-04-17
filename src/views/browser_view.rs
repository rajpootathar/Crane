use crate::workspace::BrowserPane;
use egui::{Color32, RichText};

pub fn render(ui: &mut egui::Ui, pane: &mut BrowserPane, title: &mut String) {
    ui.horizontal(|ui| {
        ui.label("URL:");
        let response = ui.text_edit_singleline(&mut pane.input_buf);
        let load = ui.button("Load").clicked()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if load {
            pane.url = pane.input_buf.trim().to_string();
            if !pane.url.is_empty() {
                *title = pane.url.clone();
            }
        }
        if ui.button("Open in System Browser").clicked() && !pane.url.is_empty() {
            let _ = webbrowser::open(&pane.url);
        }
    });
    ui.separator();
    if pane.url.is_empty() {
        ui.label("Enter a URL and press Load.");
    } else {
        ui.label(
            RichText::new(format!("Browser: {}", pane.url))
                .color(Color32::from_rgb(200, 206, 220))
                .size(14.0),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Embedded webview (wry/WKWebView) integration is pending — \
                 tracked as task #38. For now, use \"Open in System Browser\".",
            )
            .color(Color32::from_rgb(140, 146, 160))
            .italics(),
        );
    }
}

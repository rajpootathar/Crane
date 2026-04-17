use crate::state::App;
use crate::ui_util::icon_button;
use crate::layout::{BrowserPane, Dir, PaneContent};
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;

const TOPBAR_H: f32 = 34.0;
const TOPBAR_BG: Color32 = Color32::from_rgb(20, 22, 32);
const DIVIDER: Color32 = Color32::from_rgb(36, 40, 52);
const DIM: Color32 = Color32::from_rgb(130, 136, 150);
const PRIMARY: Color32 = Color32::from_rgb(210, 214, 224);

pub const TOTAL_H: f32 = TOPBAR_H;

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let rect = ui.available_rect_before_wrap();
    let bar_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TOPBAR_H));

    ui.painter().rect_filled(bar_rect, 0.0, TOPBAR_BG);
    ui.painter().line_segment(
        [
            egui::pos2(bar_rect.min.x, bar_rect.max.y),
            egui::pos2(bar_rect.max.x, bar_rect.max.y),
        ],
        egui::Stroke::new(1.0, DIVIDER),
    );

    let mut bar_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar_rect.shrink2(egui::vec2(10.0, 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    let left_label = if app.show_left {
        icons::SIDEBAR_SIMPLE
    } else {
        icons::SIDEBAR
    };
    if icon_button(&mut bar_ui, left_label, 16.0, "Toggle Left Panel (Cmd+B)").clicked() {
        app.show_left = !app.show_left;
    }
    bar_ui.add_space(6.0);
    bar_ui.label(
        RichText::new(app.breadcrumb())
            .size(12.5)
            .color(PRIMARY),
    );

    bar_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let right_label = if app.show_right {
            icons::SIDEBAR_SIMPLE
        } else {
            icons::SIDEBAR
        };
        if icon_button(ui, right_label, 16.0, "Toggle Right Panel (Cmd+/)").clicked() {
            app.show_right = !app.show_right;
        }
        ui.separator();
        if icon_button(ui, icons::QUESTION, 16.0, "Keyboard shortcuts").clicked() {
            app.show_help = !app.show_help;
        }
        ui.separator();
        let mut split_content: Option<PaneContent> = None;
        if ui
            .button(RichText::new(format!("{}  Browser", icons::GLOBE)).size(12.5))
            .on_hover_text("Split active pane with browser")
            .clicked()
        {
            split_content = Some(PaneContent::Browser(BrowserPane {
                url: String::new(),
                input_buf: "https://".into(),
            }));
        }
        let split_terminal = ui
            .button(
                RichText::new(format!("{}  Terminal", icons::TERMINAL_WINDOW)).size(12.5),
            )
            .on_hover_text("Split active pane with terminal (Cmd+T or Cmd+D)")
            .clicked();

        if split_terminal
            && let Some(ws) = app.active_layout() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        if let Some(content) = split_content
            && let Some(ws) = app.active_layout() {
                ws.add_pane(content, Some(Dir::Horizontal));
            }
    });

    ui.advance_cursor_after_rect(bar_rect);
}

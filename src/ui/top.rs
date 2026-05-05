use crate::state::App;
use crate::ui::util::icon_button;
use crate::state::layout::{BrowserPane, Dir, PaneContent};
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;

pub const TOPBAR_H: f32 = 34.0;
fn topbar_bg() -> Color32 {
    crate::theme::current().topbar_bg.to_color32()
}
fn divider() -> Color32 {
    crate::theme::current().divider.to_color32()
}
fn primary() -> Color32 {
    crate::theme::current().text.to_color32()
}

pub const TOTAL_H: f32 = TOPBAR_H;

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let rect = ui.available_rect_before_wrap();
    let bar_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TOPBAR_H));

    ui.painter().rect_filled(bar_rect, 0.0, topbar_bg());
    ui.painter().line_segment(
        [
            egui::pos2(bar_rect.min.x, bar_rect.max.y),
            egui::pos2(bar_rect.max.x, bar_rect.max.y),
        ],
        egui::Stroke::new(1.0, divider()),
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
            .color(primary()),
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
        // Bottom-docked Git Log Pane toggle. Mirrors Cmd+9.
        if icon_button(ui, icons::GIT_BRANCH, 16.0, "Toggle Git Log (Cmd+9)").clicked() {
            app.toggle_git_log(ctx);
        }
        // Settings / Help live in the status bar bottom-right now;
        // top bar stays focused on layout chrome.
        ui.separator();
        let mut split_content: Option<PaneContent> = None;
        if ui
            .button(RichText::new(format!("{}  Browser", icons::GLOBE)).size(12.5))
            .on_hover_text("Split active pane with browser")
            .clicked()
        {
            split_content = Some(PaneContent::Browser(BrowserPane::new_with(
                String::new(),
                "https://".into(),
            )));
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

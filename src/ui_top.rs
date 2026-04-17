use crate::state::App;
use crate::workspace::{BrowserPane, DiffPane, Dir, FilesPane, MarkdownPane, PaneContent};
use egui::{Color32, RichText};

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

    let left_label = if app.show_left { "◧" } else { "▸" };
    if bar_ui
        .small_button(RichText::new(left_label).size(14.0))
        .on_hover_text("Toggle Left Panel (Cmd+B)")
        .clicked()
    {
        app.show_left = !app.show_left;
    }
    bar_ui.add_space(6.0);
    bar_ui.label(
        RichText::new(app.breadcrumb())
            .size(12.5)
            .color(PRIMARY),
    );

    bar_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let right_label = if app.show_right { "◨" } else { "◂" };
        if ui
            .small_button(RichText::new(right_label).size(14.0))
            .on_hover_text("Toggle Right Panel (Cmd+/)")
            .clicked()
        {
            app.show_right = !app.show_right;
        }
        ui.separator();
        ui.label(
            RichText::new(
                "Cmd+T split  ·  Cmd+Shift+T new tab  ·  Cmd+D split  ·  Cmd+W close",
            )
            .size(10.5)
            .color(DIM),
        );
        ui.separator();
        let mut split_content: Option<PaneContent> = None;
        if ui
            .small_button("+ Browser")
            .on_hover_text("Split active pane with browser")
            .clicked()
        {
            split_content = Some(PaneContent::Browser(BrowserPane {
                url: String::new(),
                input_buf: "https://".into(),
            }));
        }
        if ui
            .small_button("+ Diff")
            .on_hover_text("Split active pane with diff")
            .clicked()
        {
            split_content = Some(PaneContent::Diff(DiffPane {
                left_path: String::new(),
                right_path: String::new(),
                left_text: String::new(),
                right_text: String::new(),
                left_buf: String::new(),
                right_buf: String::new(),
                error: None,
            }));
        }
        if ui
            .small_button("+ Markdown")
            .on_hover_text("Split active pane with markdown")
            .clicked()
        {
            split_content = Some(PaneContent::Markdown(MarkdownPane {
                path: String::new(),
                content: String::new(),
                input_buf: String::new(),
                error: None,
            }));
        }
        if ui
            .small_button("+ Files")
            .on_hover_text("Split active pane with files pane")
            .clicked()
        {
            split_content = Some(PaneContent::Files(FilesPane::empty()));
        }
        let split_terminal = ui
            .button(RichText::new("+ Terminal").strong())
            .on_hover_text("Split active pane with terminal (⌘T or ⌘D)")
            .clicked();

        if split_terminal {
            if let Some(ws) = app.active_workspace() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        }
        if let Some(content) = split_content {
            if let Some(ws) = app.active_workspace() {
                ws.add_pane(content, Some(Dir::Horizontal));
            }
        }
    });

    ui.advance_cursor_after_rect(bar_rect);
}

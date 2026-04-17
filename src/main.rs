mod git;
mod pane_view;
mod state;
mod terminal;
mod terminal_view;
mod ui_left;
mod ui_right;
mod ui_top;
mod views;
mod workspace;

use eframe::egui;
use pane_view::PaneAction;
use state::App;
use workspace::{Dir, FilesPane, PaneContent};

const BG: egui::Color32 = egui::Color32::from_rgb(14, 16, 24);
const SIDEBAR_BG: egui::Color32 = egui::Color32::from_rgb(18, 20, 28);
const DIVIDER: egui::Color32 = egui::Color32::from_rgb(36, 40, 52);

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("Crane"),
        ..Default::default()
    };

    eframe::run_native(
        "Crane",
        options,
        Box::new(|cc| Ok(Box::new(CraneApp::new(cc)))),
    )
}

struct CraneApp {
    app: App,
    logo: Option<egui::TextureHandle>,
}

impl CraneApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx
            .request_repaint_after(std::time::Duration::from_millis(1500));
        let logo = load_logo(&cc.egui_ctx);
        Self {
            app: App::new(),
            logo,
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (split_terminal, new_tab, split_h, split_v, close_pane, next_pane, prev_pane,
             zoom_in, zoom_out, zoom_reset, toggle_left, toggle_right, close_tab) =
            ctx.input(|i| {
                let cmd = i.modifiers.command;
                let shift = i.modifiers.shift;
                (
                    cmd && !shift && i.key_pressed(egui::Key::T),
                    cmd && shift && i.key_pressed(egui::Key::T),
                    cmd && !shift && i.key_pressed(egui::Key::D),
                    cmd && shift && i.key_pressed(egui::Key::D),
                    cmd && !shift && i.key_pressed(egui::Key::W),
                    cmd && i.key_pressed(egui::Key::CloseBracket),
                    cmd && i.key_pressed(egui::Key::OpenBracket),
                    cmd && (i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)),
                    cmd && i.key_pressed(egui::Key::Minus),
                    cmd && i.key_pressed(egui::Key::Num0),
                    cmd && i.key_pressed(egui::Key::B),
                    cmd && i.key_pressed(egui::Key::Slash),
                    cmd && shift && i.key_pressed(egui::Key::W),
                )
            });

        if split_terminal {
            if let Some(ws) = self.app.active_workspace() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        }
        if new_tab {
            self.app.new_tab_in_active_worktree(ctx);
        }
        if split_h {
            if let Some(ws) = self.app.active_workspace() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        }
        if split_v {
            if let Some(ws) = self.app.active_workspace() {
                ws.split_focused_with_terminal(ctx, Dir::Vertical);
            }
        }
        if close_pane {
            if let Some(ws) = self.app.active_workspace() {
                ws.close_focused();
            }
        }
        if close_tab {
            self.app.close_active_tab();
        }
        if next_pane {
            if let Some(ws) = self.app.active_workspace() {
                ws.focus_next();
            }
        }
        if prev_pane {
            if let Some(ws) = self.app.active_workspace() {
                ws.focus_prev();
            }
        }
        if zoom_in {
            self.app.font_size = (self.app.font_size + 1.0).min(40.0);
        }
        if zoom_out {
            self.app.font_size = (self.app.font_size - 1.0).max(8.0);
        }
        if zoom_reset {
            self.app.font_size = 14.0;
        }
        if toggle_left {
            self.app.show_left = !self.app.show_left;
        }
        if toggle_right {
            self.app.show_right = !self.app.show_right;
        }
    }
}

impl eframe::App for CraneApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.app.ensure_initial(&ctx);
        self.handle_shortcuts(&ctx);
        self.app.refresh_active_git_status();

        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, BG);

        let left_w = if self.app.show_left { ui_left::WIDTH } else { 0.0 };
        let right_w = if self.app.show_right { ui_right::WIDTH } else { 0.0 };

        let left_rect = egui::Rect::from_min_size(full.min, egui::vec2(left_w, full.height()));
        let right_rect = egui::Rect::from_min_size(
            egui::pos2(full.max.x - right_w, full.min.y),
            egui::vec2(right_w, full.height()),
        );
        let center_rect = egui::Rect::from_min_max(
            egui::pos2(full.min.x + left_w, full.min.y),
            egui::pos2(full.max.x - right_w, full.max.y),
        );

        if self.app.show_left {
            ui.painter().rect_filled(left_rect, 0.0, SIDEBAR_BG);
            ui.painter().line_segment(
                [
                    egui::pos2(left_rect.max.x, left_rect.min.y),
                    egui::pos2(left_rect.max.x, left_rect.max.y),
                ],
                egui::Stroke::new(1.0, DIVIDER),
            );
            let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
            left_ui.set_clip_rect(left_rect);
            ui_left::render(&mut left_ui, &mut self.app, &ctx);
        }

        if self.app.show_right {
            ui.painter().rect_filled(right_rect, 0.0, SIDEBAR_BG);
            ui.painter().line_segment(
                [
                    egui::pos2(right_rect.min.x, right_rect.min.y),
                    egui::pos2(right_rect.min.x, right_rect.max.y),
                ],
                egui::Stroke::new(1.0, DIVIDER),
            );
            let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(right_rect));
            right_ui.set_clip_rect(right_rect);
            ui_right::render(&mut right_ui, &mut self.app);
        }

        let mut center_ui = ui.new_child(egui::UiBuilder::new().max_rect(center_rect));
        center_ui.set_clip_rect(center_rect);
        ui_top::render(&mut center_ui, &mut self.app, &ctx, self.logo.as_ref());

        let canvas_rect = egui::Rect::from_min_max(
            egui::pos2(center_rect.min.x, center_rect.min.y + ui_top::TOTAL_H),
            center_rect.max,
        );
        let font_size = self.app.font_size;
        let inset = canvas_rect.shrink(6.0);
        if self.app.active_workspace().is_some() {
            if let Some(ws) = self.app.active_workspace() {
                let action = pane_view::render_workspace(&mut center_ui, ws, font_size, inset);
                match action {
                    PaneAction::None => {}
                    PaneAction::Focus(id) => ws.focus = Some(id),
                    PaneAction::Close(id) => {
                        ws.focus = Some(id);
                        ws.close_focused();
                    }
                    PaneAction::ResizeSplit { path, ratio } => {
                        ws.set_split_ratio(&path, ratio);
                    }
                }
            }
        } else {
            render_empty_state(&mut center_ui, &mut self.app, &ctx, inset);
        }
    }
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let bytes = include_bytes!("../crane.png");
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture("crane_logo", color, egui::TextureOptions::LINEAR))
}

fn render_empty_state(
    ui: &mut egui::Ui,
    app: &mut state::App,
    ctx: &egui::Context,
    rect: egui::Rect,
) {
    let mut empty_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)),
    );
    empty_ui.set_clip_rect(rect);
    empty_ui.vertical_centered(|ui| {
        ui.add_space(rect.height() * 0.3);
        ui.label(
            egui::RichText::new("No tabs open")
                .size(18.0)
                .color(egui::Color32::from_rgb(200, 204, 220)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Cmd+T to create a new terminal tab")
                .size(12.0)
                .color(egui::Color32::from_rgb(130, 136, 150)),
        );
        ui.add_space(20.0);
        if ui
            .add_sized(
                [180.0, 32.0],
                egui::Button::new(egui::RichText::new("+ New Terminal Tab").size(13.0)),
            )
            .clicked()
        {
            app.new_tab_in_active_worktree(ctx);
        }
    });
}

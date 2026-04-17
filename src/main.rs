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

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("Crane");
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Crane",
        options,
        Box::new(|cc| Ok(Box::new(CraneApp::new(cc)))),
    )
}

fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    let surface_1 = egui::Color32::from_rgb(40, 45, 60);
    let surface_2 = egui::Color32::from_rgb(56, 62, 82);
    let surface_3 = egui::Color32::from_rgb(72, 80, 104);
    let border_subtle = egui::Color32::from_rgb(60, 66, 86);
    let border_strong = egui::Color32::from_rgb(96, 106, 132);
    let text_primary = egui::Color32::from_rgb(212, 216, 228);
    let text_hover = egui::Color32::from_rgb(234, 238, 248);
    let accent = egui::Color32::from_rgb(90, 135, 220);

    let corner = egui::CornerRadius::same(6);
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = corner;
    }

    style.visuals.widgets.inactive.weak_bg_fill = surface_1;
    style.visuals.widgets.inactive.bg_fill = surface_1;
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, border_subtle);
    style.visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, text_primary);

    style.visuals.widgets.hovered.weak_bg_fill = surface_2;
    style.visuals.widgets.hovered.bg_fill = surface_2;
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, border_strong);
    style.visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, text_hover);

    style.visuals.widgets.active.weak_bg_fill = surface_3;
    style.visuals.widgets.active.bg_fill = surface_3;
    style.visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, border_strong);
    style.visuals.widgets.active.fg_stroke =
        egui::Stroke::new(1.0, text_hover);

    style.visuals.selection.bg_fill =
        egui::Color32::from_rgba_unmultiplied(90, 135, 220, 70);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    style.visuals.window_corner_radius = egui::CornerRadius::same(10);
    style.visuals.window_fill = egui::Color32::from_rgb(22, 25, 36);
    style.visuals.window_stroke = egui::Stroke::new(1.0, border_subtle);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(8);

    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.menu_margin = egui::Margin::symmetric(6, 6);

    ctx.set_style(style);
}

fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../crane.png");
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

struct CraneApp {
    app: App,
}

impl CraneApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);
        apply_style(&cc.egui_ctx);
        cc.egui_ctx
            .request_repaint_after(std::time::Duration::from_millis(1500));
        Self { app: App::new() }
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
        self.app.refresh_active_git_status(&ctx);

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
        ui_top::render(&mut center_ui, &mut self.app, &ctx);

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

        render_new_workspace_modal(&ctx, &mut self.app);
        render_help_modal(&ctx, &mut self.app);
    }
}

fn render_help_modal(ctx: &egui::Context, app: &mut state::App) {
    if !app.show_help {
        return;
    }
    let mut open = true;
    egui::Window::new("Keyboard Shortcuts")
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let rows: &[(&str, &str)] = &[
                ("Cmd+T", "Split active Pane with new terminal"),
                ("Cmd+Shift+T", "New Tab in active Workspace"),
                ("Cmd+D", "Split Pane horizontally (side-by-side)"),
                ("Cmd+Shift+D", "Split Pane vertically (stacked)"),
                ("Cmd+W", "Close focused Pane"),
                ("Cmd+Shift+W", "Close active Tab"),
                ("Cmd+[ / Cmd+]", "Focus prev / next Pane"),
                ("Cmd+B", "Toggle Left Panel"),
                ("Cmd+/", "Toggle Right Panel"),
                ("Cmd+= / Cmd+-", "Increase / decrease font size"),
                ("Cmd+0", "Reset font size"),
                ("Ctrl+C / Ctrl+D", "Terminal: interrupt / EOF"),
            ];
            egui::Grid::new("shortcuts_grid")
                .num_columns(2)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for (key, desc) in rows {
                        ui.label(egui::RichText::new(*key).monospace().strong());
                        ui.label(*desc);
                        ui.end_row();
                    }
                });
        });
    if !open {
        app.show_help = false;
    }
}

fn render_new_workspace_modal(ctx: &egui::Context, app: &mut state::App) {
    let mut open = app.new_workspace_modal.is_some();
    if !open {
        return;
    }
    let mut create = false;
    let mut cancel = false;
    let mut browse: Option<String> = None;
    let modal_width = 460.0;
    egui::Window::new("New Workspace")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(modal_width, 240.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(modal_width - 20.0);
            let input_width = modal_width - 40.0;
            if let Some(modal) = app.new_workspace_modal.as_mut() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Branch").strong());
                ui.add(
                    egui::TextEdit::singleline(&mut modal.branch)
                        .hint_text("feature/my-branch")
                        .desired_width(input_width),
                );
                ui.checkbox(&mut modal.create_new_branch, "Create new branch");
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Location").strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut modal.path)
                            .hint_text("~/.crane-worktrees/<project>")
                            .desired_width(input_width - 88.0),
                    );
                    if ui.button("Browse…").clicked() {
                        browse = Some(modal.path.clone());
                    }
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "→ {}/{}",
                            modal.path.trim_end_matches('/'),
                            if modal.branch.is_empty() {
                                "<branch>"
                            } else {
                                &modal.branch
                            }
                        ))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(130, 136, 150)),
                    )
                    .truncate(),
                );
                if let Some(err) = &modal.error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Create").strong()).clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            }
        });
    if !open || cancel {
        app.new_workspace_modal = None;
    } else if let Some(current) = browse {
        let start = std::path::PathBuf::from(if current.is_empty() {
            std::env::var("HOME").unwrap_or_default()
        } else {
            current
        });
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Choose workspace parent folder")
            .set_directory(start)
            .pick_folder()
        {
            if let Some(modal) = app.new_workspace_modal.as_mut() {
                modal.path = p.to_string_lossy().to_string();
            }
        }
    } else if create {
        app.create_workspace_from_modal(ctx);
    }
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

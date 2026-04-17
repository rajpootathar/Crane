#![allow(dead_code)] // WIP: some helpers are staged for upcoming features (branch switch, wry browser)

mod git;
mod layout;
mod pane_view;
mod session;
mod state;
mod terminal;
mod terminal_view;
mod theme;
mod ui_left;
mod ui_right;
mod ui_top;
mod ui_util;
mod update_check;
mod views;

use eframe::egui;
use layout::Dir;
use pane_view::PaneAction;
use state::App;

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
    let t = theme::current();
    let light = t.bg.r as u32 + t.bg.g as u32 + t.bg.b as u32 > 128 * 3;
    ctx.set_visuals(if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    });

    let mut style = (*ctx.global_style()).clone();
    let surface_1 = t.surface.to_color32();
    let surface_2 = t.surface_alt.to_color32();
    let surface_3 = t.surface_hi.to_color32();
    let border_subtle = t.border.to_color32();
    let border_strong = t.border_strong.to_color32();
    let text_primary = t.text.to_color32();
    let text_hover = t.text_hover.to_color32();
    let accent = t.accent.to_color32();

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
        egui::Color32::from_rgba_unmultiplied(t.accent.r, t.accent.g, t.accent.b, 70);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    style.visuals.window_corner_radius = egui::CornerRadius::same(10);
    style.visuals.window_fill = t.surface.to_color32();
    style.visuals.window_stroke = egui::Stroke::new(1.0, border_subtle);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(8);

    // These are the colours TextEdit, ScrollArea and inline code rely on.
    // Without them, the Files Pane's editor background ignored the theme.
    style.visuals.panel_fill = t.bg.to_color32();
    style.visuals.extreme_bg_color = t.bg.to_color32();
    style.visuals.code_bg_color = t.surface.to_color32();
    style.visuals.faint_bg_color = t.row_hover.to_color32();
    style.visuals.override_text_color = Some(text_primary);

    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.menu_margin = egui::Margin::symmetric(6, 6);

    ctx.set_global_style(style);
}

/// One-shot migration: if the old `~/.config/crane/` dir exists and the new
/// `~/.crane/` doesn't, move it over so users on earlier builds don't lose
/// their session or custom themes.
fn migrate_config_dir() {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let old_dir = std::path::PathBuf::from(format!("{home}/.config/crane"));
    let new_dir = std::path::PathBuf::from(format!("{home}/.crane"));
    if old_dir.is_dir() && !new_dir.exists() {
        let _ = std::fs::rename(&old_dir, &new_dir);
    }
}

fn open_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
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
    last_saved_snapshot: String,
    last_save_at: std::time::Instant,
}

impl CraneApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx
            .request_repaint_after(std::time::Duration::from_millis(1500));
        let mut app = match session::load() {
            Some(s) => s.restore(&cc.egui_ctx),
            None => App::new(),
        };
        migrate_config_dir();
        theme::ensure_builtin_tomls_on_disk();
        let initial = theme::find_by_name(&app.selected_theme)
            .unwrap_or_else(theme::Theme::dark);
        theme::init(initial);
        apply_style(&cc.egui_ctx);
        app.update_check.spawn_check(cc.egui_ctx.clone());
        Self {
            app,
            last_saved_snapshot: String::new(),
            last_save_at: std::time::Instant::now(),
        }
    }

    fn maybe_save(&mut self) {
        if self.last_save_at.elapsed() < session::SAVE_DEBOUNCE {
            return;
        }
        // Serialise on the render thread (fast — bytes for a modest
        // session ≈ tens of KB), diff against the last snapshot, then
        // hand the bytes to a background thread to write. The UI never
        // blocks on fsync.
        let session_value = session::Session::from_app(&self.app);
        let bytes = match serde_json::to_vec_pretty(&session_value) {
            Ok(b) => b,
            Err(_) => return,
        };
        let snapshot = match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(_) => return,
        };
        if snapshot == self.last_saved_snapshot {
            self.last_save_at = std::time::Instant::now();
            return;
        }
        let path = session::session_file();
        std::thread::spawn(move || {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
        self.last_saved_snapshot = snapshot;
        self.last_save_at = std::time::Instant::now();
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

        if split_terminal
            && let Some(ws) = self.app.active_layout() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        if new_tab {
            self.app.new_tab_in_active_workspace(ctx);
        }
        if split_h
            && let Some(ws) = self.app.active_layout() {
                ws.split_focused_with_terminal(ctx, Dir::Horizontal);
            }
        if split_v
            && let Some(ws) = self.app.active_layout() {
                ws.split_focused_with_terminal(ctx, Dir::Vertical);
            }
        if close_pane
            && let Some(ws) = self.app.active_layout() {
                ws.close_focused();
            }
        if close_tab {
            self.app.close_active_tab();
        }
        if next_pane
            && let Some(ws) = self.app.active_layout() {
                ws.focus_next();
            }
        if prev_pane
            && let Some(ws) = self.app.active_layout() {
                ws.focus_prev();
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
        let t = theme::current();
        let bg = t.bg.to_color32();
        let sidebar_bg = t.sidebar_bg.to_color32();
        let divider = t.divider.to_color32();
        ui.painter().rect_filled(full, 0.0, bg);

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
            ui.painter().rect_filled(left_rect, 0.0, sidebar_bg);
            ui.painter().line_segment(
                [
                    egui::pos2(left_rect.max.x, left_rect.min.y),
                    egui::pos2(left_rect.max.x, left_rect.max.y),
                ],
                egui::Stroke::new(1.0, divider),
            );
            let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
            left_ui.set_clip_rect(left_rect);
            ui_left::render(&mut left_ui, &mut self.app, &ctx);
        }

        if self.app.show_right {
            ui.painter().rect_filled(right_rect, 0.0, sidebar_bg);
            ui.painter().line_segment(
                [
                    egui::pos2(right_rect.min.x, right_rect.min.y),
                    egui::pos2(right_rect.min.x, right_rect.max.y),
                ],
                egui::Stroke::new(1.0, divider),
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
        if self.app.active_layout().is_some() {
            if let Some(ws) = self.app.active_layout() {
                let action = pane_view::render_layout(&mut center_ui, ws, font_size, inset);
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
                    PaneAction::SwapPanes { a, b } => {
                        ws.swap_panes(a, b);
                    }
                    PaneAction::DockPane { src, target, edge } => {
                        ws.dock_pane(src, target, edge);
                    }
                }
            }
        } else {
            render_empty_state(&mut center_ui, &mut self.app, &ctx, inset);
        }

        render_new_workspace_modal(&ctx, &mut self.app);
        render_help_modal(&ctx, &mut self.app);
        render_settings_modal(&ctx, &mut self.app);
        self.app.update_check.drain();
        render_update_toast(&ctx, &mut self.app);
        self.maybe_save();
    }
}

fn render_settings_modal(ctx: &egui::Context, app: &mut state::App) {
    if !app.show_settings {
        return;
    }
    let mut open = true;
    let mut selected_now: Option<String> = None;
    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(420.0, 380.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Theme").strong());
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for theme in theme::load_all() {
                        let is_active = app.selected_theme == theme.name;
                        let label = format!(
                            "{}{}",
                            if is_active { "● " } else { "  " },
                            theme.name
                        );
                        let resp = ui.add(
                            egui::Button::new(
                                egui::RichText::new(label).size(13.0),
                            )
                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                        );
                        if resp.clicked() && !is_active {
                            selected_now = Some(theme.name.clone());
                        }
                    }
                });
            ui.add_space(8.0);
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "Drop custom themes (*.toml) at: {}",
                    theme::themes_dir().display()
                ))
                .size(10.5)
                .color(theme::current().text_muted.to_color32()),
            );
            if ui.small_button("Open themes folder").clicked() {
                let dir = theme::themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_in_file_manager(&dir);
            }
        });
    if !open {
        app.show_settings = false;
    }
    if let Some(name) = selected_now
        && let Some(t) = theme::find_by_name(&name)
    {
        theme::set(t);
        app.selected_theme = name;
        apply_style(ctx);
        ctx.request_repaint();
    }
}

fn render_update_toast(ctx: &egui::Context, app: &mut state::App) {
    if !app.update_check.should_show() {
        return;
    }
    let version = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.version.clone())
        .unwrap_or_default();
    let url = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.url.clone())
        .unwrap_or_default();

    let screen = ctx.content_rect();
    let toast_w = 440.0_f32.min(screen.width() - 40.0);
    egui::Area::new(egui::Id::new("update_toast"))
        .order(egui::Order::Tooltip)
        .fixed_pos(egui::pos2(
            screen.max.x - toast_w - 20.0,
            screen.max.y - 140.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(egui::Color32::from_rgb(28, 32, 44))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 66, 86)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(toast_w - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::ARROW_CIRCLE_UP)
                                .size(18.0)
                                .color(egui::Color32::from_rgb(96, 140, 220)),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Crane v{version} is available"))
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(212, 216, 228))
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "You're on v{}. Grab the new build?",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .size(11.5)
                                .color(egui::Color32::from_rgb(150, 156, 172)),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!(
                                    "{}  Download",
                                    egui_phosphor::regular::DOWNLOAD_SIMPLE
                                ))
                                .size(12.0)
                                .strong(),
                            )
                            .clicked()
                        {
                            let _ = webbrowser::open(&url);
                            app.update_check.dismiss_forever();
                        }
                        if ui
                            .button(egui::RichText::new("Not now").size(12.0))
                            .clicked()
                        {
                            app.update_check.dismiss_session();
                        }
                        if ui
                            .button(egui::RichText::new("Remind in 7 days").size(12.0))
                            .clicked()
                        {
                            app.update_check.remind_later();
                        }
                    });
                });
        });
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
    let modal_width = 480.0;
    let project_info = app.new_workspace_modal.as_ref().and_then(|m| {
        app.projects
            .iter()
            .find(|p| p.id == m.project_id)
            .map(|p| (p.path.clone(), p.name.clone()))
    });

    egui::Window::new("New Worktree")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(modal_width, 280.0))
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
                    ui.selectable_value(
                        &mut modal.mode,
                        state::LocationMode::Global,
                        "Global",
                    )
                    .on_hover_text("~/.crane-worktrees/<project>/<branch>");
                    ui.selectable_value(
                        &mut modal.mode,
                        state::LocationMode::ProjectLocal,
                        "Project-local",
                    )
                    .on_hover_text("<project>/.crane-worktrees/<branch>");
                    ui.selectable_value(
                        &mut modal.mode,
                        state::LocationMode::Custom,
                        "Custom",
                    )
                    .on_hover_text("Pick any folder");
                });
                if modal.mode == state::LocationMode::Custom {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut modal.custom_path)
                                .hint_text("/path/to/parent")
                                .desired_width(input_width - 88.0),
                        );
                        if ui.button("Browse…").clicked() {
                            browse = Some(modal.custom_path.clone());
                        }
                    });
                }
                let preview = project_info
                    .as_ref()
                    .map(|(p, n)| modal.resolved_parent(p, n))
                    .unwrap_or_default();
                let preview_str = format!(
                    "→ {}/{}",
                    preview.display().to_string().trim_end_matches('/'),
                    if modal.branch.is_empty() {
                        "<branch>"
                    } else {
                        &modal.branch
                    }
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(preview_str)
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
            .set_title("Choose worktree parent folder")
            .set_directory(start)
            .pick_folder()
            && let Some(modal) = app.new_workspace_modal.as_mut() {
                modal.custom_path = p.to_string_lossy().to_string();
                modal.mode = state::LocationMode::Custom;
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
        ui.add_space(rect.height() * 0.25);
        let has_project = !app.projects.is_empty();
        let (title, hint) = if has_project {
            ("No tabs open", "Cmd+T to create a new terminal tab")
        } else {
            ("Welcome to Crane", "Add a project from the Left Panel to get started")
        };
        ui.label(
            egui::RichText::new(title)
                .size(18.0)
                .color(egui::Color32::from_rgb(200, 204, 220)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(hint)
                .size(12.0)
                .color(egui::Color32::from_rgb(130, 136, 150)),
        );
        ui.add_space(20.0);
        if has_project {
            if ui
                .add_sized(
                    [180.0, 32.0],
                    egui::Button::new(egui::RichText::new("+ New Terminal Tab").size(13.0)),
                )
                .clicked()
            {
                app.new_tab_in_active_workspace(ctx);
            }
        } else if ui
            .add_sized(
                [180.0, 32.0],
                egui::Button::new(
                    egui::RichText::new(format!("{}  Add Project…", egui_phosphor::regular::FOLDER_PLUS))
                        .size(13.0),
                ),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose project folder")
                .pick_folder()
            {
                app.add_project_from_path(path, ctx);
            }
    });
}

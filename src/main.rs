mod format;
mod git;
mod lsp;
mod modals;
mod state;
mod terminal;
mod theme;
mod ui;
mod update;
mod util;
mod views;

use modals::{
    render_empty_state, render_help_modal, render_lsp_download_toast,
    render_lsp_install_prompt, render_new_workspace_modal, render_settings_modal,
    render_update_toast,
};

use eframe::egui;
use state::App;
use state::layout::Dir;
use ui::pane_view::PaneAction;

fn main() -> eframe::Result {
    env_logger::init();
    fix_path_for_gui_launch();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("Crane");
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        // Persist the window size / position across launches. Backed by
        // eframe's storage (ron file under the OS' app-data dir). Requires
        // the `persistence` feature.
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "Crane",
        options,
        Box::new(|cc| Ok(Box::new(CraneApp::new(cc)))),
    )
}

/// When a macOS .app is double-clicked from Finder, the process gets a stripped
/// PATH (typically just `/usr/bin:/bin:/usr/sbin:/sbin`). This means LSP
/// servers installed via cargo / brew / npm are not found, and terminals feel
/// broken. Mirror what VSCode does: run the user's login shell once and
/// copy its PATH + common env vars.
fn fix_path_for_gui_launch() {
    // Only do the shell dance when launched from a GUI context. A quick
    // heuristic: PATH lacks `/usr/local/bin` AND `HOME` is set.
    let current = std::env::var("PATH").unwrap_or_default();
    let looks_gui = !current.contains("/usr/local/bin")
        && !current.contains("/opt/homebrew/bin")
        && !current.contains(".cargo/bin")
        && std::env::var("HOME").is_ok();
    if !looks_gui {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    // Login-only (`-l`) — `-i` used to be here, but interactive shells
    // source `.zshrc`/`.bashrc` which triggers nvm/brew-shellenv/banners
    // and can add several seconds to startup. Login mode sources the
    // profile files (`.zprofile`, `.bash_profile`), which is the
    // standard place to set PATH.
    let output = std::process::Command::new(&shell)
        .arg("-l")
        .arg("-c")
        .arg("echo __CRANE_PATH__:$PATH")
        .output();
    if let Ok(out) = output {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().find(|l| l.starts_with("__CRANE_PATH__:")) {
            let path = line.trim_start_matches("__CRANE_PATH__:").to_string();
            if !path.is_empty() {
                // SAFETY: called from main() before any threads spawn.
                unsafe { std::env::set_var("PATH", &path) };
                return;
            }
        }
    }
    // Fallback: sprinkle the usual suspects onto whatever PATH we have.
    let home = std::env::var("HOME").unwrap_or_default();
    let extras = [
        format!("{home}/.cargo/bin"),
        format!("{home}/.local/bin"),
        format!("{home}/go/bin"),
        format!("{home}/.volta/bin"),
        format!("{home}/.fnm/aliases/default/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    let mut parts: Vec<String> = extras.into_iter().collect();
    if !current.is_empty() {
        parts.push(current);
    }
    // SAFETY: called from main() before any threads spawn.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
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

    // egui exposes debug paint flags only in debug builds. Release builds
    // never show them anyway, so we just zero them when available.
    #[cfg(debug_assertions)]
    {
        style.debug = egui::style::DebugOptions::default();
        style.debug.debug_on_hover = false;
        style.debug.debug_on_hover_with_all_modifiers = false;
        style.debug.show_expand_width = false;
        style.debug.show_expand_height = false;
        style.debug.show_resize = false;
        style.debug.show_interactive_widgets = false;
        style.debug.show_widget_hits = false;
    }

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
    last_saved_settings_snapshot: String,
    last_save_at: std::time::Instant,
    pending_close: Option<state::layout::PaneId>,
}

fn terminal_is_running(app: &App, id: state::layout::PaneId) -> bool {
    let Some(layout) = app.active_layout_ref() else {
        return false;
    };
    let Some(pane) = layout.panes.get(&id) else {
        return false;
    };
    match &pane.content {
        state::layout::PaneContent::Terminal(t) => t.has_foreground_process(),
        _ => false,
    }
}

/// JetBrains Mono Regular — bundled (~264 KB). OFL 1.1 licensed. Used as the
/// default Monospace font because egui's built-in Hack doesn't cover braille
/// patterns (U+2800–U+28FF) or block elements, which breaks TUI apps like
/// nvitop / btop / htop that draw with those glyphs.
const JETBRAINS_MONO_TTF: &[u8] =
    include_bytes!("../assets/JetBrainsMono-Regular.ttf");

pub fn load_fonts(ctx: &egui::Context, custom_mono: Option<&str>) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    // Always install JetBrains Mono as a Monospace entry ahead of egui
    // defaults. Gives us braille + box-drawing + block-element glyphs for
    // free, so nvitop / btop render correctly out of the box.
    fonts.font_data.insert(
        "jetbrains_mono".to_string(),
        std::sync::Arc::new(egui::FontData::from_static(JETBRAINS_MONO_TTF)),
    );
    if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        mono.insert(0, "jetbrains_mono".to_string());
    }

    // A user-selected font takes priority over the bundled default.
    if let Some(path) = custom_mono
        && let Ok(bytes) = std::fs::read(path)
    {
        let name = "user_mono".to_string();
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            mono.insert(0, name);
        }
    }

    ctx.set_fonts(fonts);
}

impl CraneApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx
            .request_repaint_after(std::time::Duration::from_millis(1500));
        migrate_config_dir();
        let mut app = match state::session::load() {
            Some(s) => s.restore(&cc.egui_ctx),
            None => App::new(),
        };
        // settings.json (user prefs) takes precedence over any matching
        // keys that may still live in session.json from older installs.
        state::settings::Settings::load().apply(&mut app);
        theme::ensure_builtin_tomls_on_disk();
        let initial = theme::find_by_name(&app.selected_theme)
            .unwrap_or_else(theme::Theme::dark);
        theme::init(initial);
        load_fonts(&cc.egui_ctx, app.custom_mono_font.as_deref());
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        apply_style(&cc.egui_ctx);
        app.update_check.spawn_check(cc.egui_ctx.clone());
        Self {
            app,
            last_saved_snapshot: String::new(),
            last_saved_settings_snapshot: String::new(),
            last_save_at: std::time::Instant::now(),
            pending_close: None,
        }
    }

    /// Land a goto-definition result: open the file in the Files Pane if
    /// it's not already open, and stash the target line/column so the next
    /// render moves the cursor there.
    fn goto_location(&mut self, ctx: &egui::Context, loc: lsp::Location) {
        let path_str = loc.path.to_string_lossy().to_string();
        let mut placed = false;
        if let Some(layout) = self.app.active_layout_ref() {
            for (_, p) in &layout.panes {
                if matches!(&p.content, state::layout::PaneContent::Files(_)) {
                    placed = true;
                    break;
                }
            }
        }
        if !placed {
            let name = loc
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&path_str)
                .to_string();
            let content = std::fs::read_to_string(&loc.path).unwrap_or_default();
            self.app
                .open_file_into_active_layout(ctx, path_str.clone(), name, content);
        }
        if let Some(layout) = self.app.active_layout() {
            for (_, pane) in layout.panes.iter_mut() {
                if let state::layout::PaneContent::Files(files) = &mut pane.content {
                    // Make sure the target file is a tab in this pane.
                    let idx = files.tabs.iter().position(|t| t.path == path_str);
                    let idx = match idx {
                        Some(i) => i,
                        None => {
                            let content =
                                std::fs::read_to_string(&loc.path).unwrap_or_default();
                            let name = loc
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&path_str)
                                .to_string();
                            files.open(path_str.clone(), content, name);
                            files.tabs.len() - 1
                        }
                    };
                    files.active = idx;
                    files.tabs[idx].pending_cursor = Some((loc.line, loc.character));
                    break;
                }
            }
        }
    }

    fn render_confirm_close(&mut self, ctx: &egui::Context) {
        let Some(id) = self.pending_close else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Terminal is still running")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(340.0);
                ui.add_space(4.0);
                ui.label("A process is running in this terminal. Closing it will kill the process.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Close terminal").clicked() {
                        confirm = true;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if cancel {
            self.pending_close = None;
        } else if confirm {
            if let Some(ws) = self.app.active_layout() {
                ws.focus = Some(id);
                ws.close_focused();
            }
            self.pending_close = None;
        }
    }

    fn maybe_save(&mut self) {
        if self.last_save_at.elapsed() < state::session::SAVE_DEBOUNCE {
            return;
        }
        // Serialise on the render thread (fast — bytes for a modest
        // session ≈ tens of KB), diff against the last snapshot, then
        // hand the bytes to a background thread to write. The UI never
        // blocks on fsync.
        let session_value = state::session::Session::from_app(&self.app);
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
        let path = state::session::session_file();
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

        // User prefs live in a separate file (~/.crane/settings.json) so
        // they stay intact even when the session gets wiped.
        let settings = state::settings::Settings::from_app(&self.app);
        if let Ok(s_bytes) = serde_json::to_vec_pretty(&settings) {
            let s_snap = String::from_utf8_lossy(&s_bytes).to_string();
            if s_snap != self.last_saved_settings_snapshot {
                std::thread::spawn(move || {
                    let _ = settings.save();
                });
                self.last_saved_settings_snapshot = s_snap;
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // When any modal is open, Cmd+W closes the modal instead of the
        // pane underneath. Also absorb Escape. Everything else falls
        // through so Cmd+S etc. still work inside modals.
        let modal_open = self.app.show_settings
            || self.app.show_help
            || self.app.new_workspace_modal.is_some()
            || self.pending_close.is_some();
        if modal_open {
            let (cmd_w, esc) = ctx.input(|i| {
                let cmd = i.modifiers.command || i.modifiers.mac_cmd;
                (cmd && i.key_pressed(egui::Key::W), i.key_pressed(egui::Key::Escape))
            });
            if cmd_w || esc {
                if self.app.show_settings {
                    self.app.show_settings = false;
                }
                if self.app.show_help {
                    self.app.show_help = false;
                }
                if esc && self.app.new_workspace_modal.is_some() {
                    self.app.new_workspace_modal = None;
                }
                if esc && self.pending_close.is_some() {
                    self.pending_close = None;
                }
                return;
            }
        }

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
        if close_pane {
            let focus = self
                .app
                .active_layout_ref()
                .and_then(|l| l.focus);
            if let Some(id) = focus {
                // Special case: in a Files pane with multiple open tabs,
                // Cmd+W closes the active file tab instead of the whole
                // pane. The pane-close still fires when it's the last
                // (or only) tab — user's expectation.
                let closed_file_tab = if let Some(ws) = self.app.active_layout()
                    && let Some(pane) = ws.panes.get_mut(&id)
                    && let state::layout::PaneContent::Files(files) = &mut pane.content
                    && files.tabs.len() > 1
                {
                    let idx = files.active.min(files.tabs.len() - 1);
                    files.close(idx);
                    true
                } else {
                    false
                };
                if !closed_file_tab {
                    if terminal_is_running(&self.app, id) {
                        self.pending_close = Some(id);
                    } else if let Some(ws) = self.app.active_layout() {
                        ws.close_focused();
                    }
                }
            }
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

        let left_w = if self.app.show_left {
            self.app.left_panel_w
        } else {
            0.0
        };
        let right_w = if self.app.show_right {
            self.app.right_panel_w
        } else {
            0.0
        };

        // Reserve a strip along the very bottom for the status bar. Panels
        // and center content compute their height above it.
        let status_bar_rect = egui::Rect::from_min_max(
            egui::pos2(full.min.x, full.max.y - ui::status::HEIGHT),
            full.max,
        );
        let content_bottom = status_bar_rect.min.y;

        let left_rect = egui::Rect::from_min_max(
            full.min,
            egui::pos2(full.min.x + left_w, content_bottom),
        );
        let right_rect = egui::Rect::from_min_max(
            egui::pos2(full.max.x - right_w, full.min.y),
            egui::pos2(full.max.x, content_bottom),
        );
        let center_rect = egui::Rect::from_min_max(
            egui::pos2(full.min.x + left_w, full.min.y),
            egui::pos2(full.max.x - right_w, content_bottom),
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
            ui::left::render(&mut left_ui, &mut self.app, &ctx);

            // 6 px drag handle straddling the right edge of the Left Panel.
            let handle = egui::Rect::from_min_max(
                egui::pos2(left_rect.max.x - 3.0, left_rect.min.y),
                egui::pos2(left_rect.max.x + 3.0, left_rect.max.y),
            );
            let resp = ui.interact(
                handle,
                egui::Id::new("left_panel_resize"),
                egui::Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                ui.ctx()
                    .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.app.left_panel_w =
                    (pos.x - full.min.x).clamp(180.0, full.width() * 0.45);
            }
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
            ui::right::render(&mut right_ui, &mut self.app);

            let handle = egui::Rect::from_min_max(
                egui::pos2(right_rect.min.x - 3.0, right_rect.min.y),
                egui::pos2(right_rect.min.x + 3.0, right_rect.max.y),
            );
            let resp = ui.interact(
                handle,
                egui::Id::new("right_panel_resize"),
                egui::Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                ui.ctx()
                    .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.app.right_panel_w =
                    (full.max.x - pos.x).clamp(200.0, full.width() * 0.5);
            }
        }

        let mut center_ui = ui.new_child(egui::UiBuilder::new().max_rect(center_rect));
        center_ui.set_clip_rect(center_rect);
        ui::top::render(&mut center_ui, &mut self.app, &ctx);

        let canvas_rect = egui::Rect::from_min_max(
            egui::pos2(center_rect.min.x, center_rect.min.y + ui::top::TOTAL_H),
            center_rect.max,
        );
        let font_size = self.app.font_size;
        let inset = canvas_rect.shrink(6.0);
        // Snapshot diagnostics for every open file in the active layout — this
        // avoids borrowing `self.app.lsp` while `self.app.active_layout()`
        // holds a mutable borrow.
        let mut diag_map: std::collections::HashMap<String, Vec<lsp::Diagnostic>> =
            std::collections::HashMap::new();
        if let Some(layout_ref) = self.app.active_layout_ref() {
            for (_, p) in &layout_ref.panes {
                if let state::layout::PaneContent::Files(f) = &p.content {
                    for t in &f.tabs {
                        diag_map.insert(
                            t.path.clone(),
                            self.app.lsp.diagnostics(std::path::Path::new(&t.path)),
                        );
                    }
                }
            }
        }
        let diag_fn = |path: &str| diag_map.get(path).cloned().unwrap_or_default();
        let save_queue: std::cell::RefCell<Vec<(String, String)>> =
            std::cell::RefCell::new(Vec::new());
        let notify_saved = |path: &str, text: &str| {
            save_queue.borrow_mut().push((path.to_string(), text.to_string()));
        };
        // Snapshot configs so the format closure doesn't need to borrow self.
        let cfg_snapshot = self.app.language_configs.clone();
        let format_before_save = |content: &str, path: &str| -> Option<String> {
            let p = std::path::Path::new(path);
            for key in lsp::server::keys_for_path(p) {
                let cfg = cfg_snapshot.get_or_default(key);
                if cfg.enabled && cfg.format_on_save {
                    return format::format_text(key, p, content);
                }
            }
            None
        };
        // Goto-definition: queue the request here; resolve + navigate
        // AFTER render finishes (where we can freely borrow self.app).
        let goto_queue: std::cell::RefCell<Vec<(String, u32, u32)>> =
            std::cell::RefCell::new(Vec::new());
        let goto_request = |path: &str, line: u32, character: u32| {
            goto_queue
                .borrow_mut()
                .push((path.to_string(), line, character));
        };
        let syntax_override = self.app.syntax_theme_override.clone();
        let workspace_root = self.app.active_workspace_path().map(|p| p.to_path_buf());
        let editor_prefs = views::file_view::EditorPrefs {
            word_wrap: self.app.editor_word_wrap,
            trim_on_save: self.app.editor_trim_on_save,
        };
        if self.app.active_layout().is_some() {
            if let Some(ws) = self.app.active_layout() {
                let action = ui::pane_view::render_layout(
                    &mut center_ui,
                    ws,
                    font_size,
                    inset,
                    syntax_override.as_deref(),
                    &diag_fn,
                    &notify_saved,
                    &format_before_save,
                    &goto_request,
                    workspace_root.as_deref(),
                    editor_prefs,
                );
                match action {
                    PaneAction::None => {}
                    PaneAction::Focus(id) => ws.focus = Some(id),
                    PaneAction::Close(id) => {
                        let running = matches!(
                            ws.panes.get(&id).map(|p| &p.content),
                            Some(state::layout::PaneContent::Terminal(t)) if t.has_foreground_process()
                        );
                        if running {
                            self.pending_close = Some(id);
                        } else {
                            ws.focus = Some(id);
                            ws.close_focused();
                        }
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
        let settings_effect = render_settings_modal(&ctx, &mut self.app, apply_style);
        if matches!(settings_effect, modals::settings::SettingsEffect::ReloadFonts) {
            load_fonts(&ctx, self.app.custom_mono_font.as_deref());
        }
        self.render_confirm_close(&ctx);
        render_lsp_install_prompt(&ctx, &mut self.app);
        render_lsp_download_toast(&ctx, &self.app);
        self.app.update_check.drain();
        render_update_toast(&ctx, &mut self.app);
        for (path, text) in save_queue.into_inner() {
            self.app.lsp.did_save(
                std::path::Path::new(&path),
                &text,
                &self.app.language_configs,
            );
        }
        // Dispatch any goto-definition requests queued this frame
        // without blocking on a response. The LSP reader thread will
        // wake us via ctx.request_repaint() when the result arrives.
        for (path, line, character) in goto_queue.into_inner() {
            let tokens = self.app.lsp.goto_dispatch(
                std::path::Path::new(&path),
                line,
                character,
            );
            for (server, request_id) in tokens {
                self.app.pending_gotos.push(state::PendingGoto {
                    server,
                    request_id,
                    dispatched_at: std::time::Instant::now(),
                });
            }
        }

        // Drain ready goto results. Jump to the first successful
        // location and drop any of its siblings (multiple LSPs per
        // file) so we don't double-navigate. 5s watchdog prunes
        // requests that never resolve.
        let mut landed = false;
        let mut pending = std::mem::take(&mut self.app.pending_gotos);
        pending.retain(|p| {
            if landed {
                return false;
            }
            if p.dispatched_at.elapsed() > std::time::Duration::from_secs(5) {
                return false;
            }
            match self.app.lsp.take_goto_result(p.server, p.request_id) {
                Some(Some(loc)) => {
                    self.goto_location(&ctx, loc);
                    landed = true;
                    false
                }
                Some(None) => false,
                None => true,
            }
        });
        self.app.pending_gotos = pending;

        // Global status bar — active file's diagnostics, language, path.
        let mut status_ui = ui.new_child(egui::UiBuilder::new().max_rect(status_bar_rect));
        status_ui.set_clip_rect(status_bar_rect);
        ui::status::render(&mut status_ui, &mut self.app);
        ui::branch_picker::render(&ctx, &mut self.app);
        self.app.sync_lsp_changes(&ctx);
        self.maybe_save();
    }
}


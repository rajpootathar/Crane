#[cfg(any(target_os = "macos", target_os = "linux"))]
mod browser;
mod format;
#[cfg(target_os = "macos")]
mod mac_keys;
mod git;
mod lsp;
mod modals;
mod platform_menu;
mod shortcuts;
mod startup;
mod state;
mod terminal;
mod theme;
mod ui;
mod update;
mod util;
mod views;

use modals::{
    render_empty_state, render_help_modal, render_lsp_download_toast,
    render_lsp_install_prompt, render_missing_project_modal, render_new_workspace_modal,
    render_settings_modal, render_update_toast,
};

use eframe::egui;
use state::App;
use ui::pane_view::PaneAction;

fn main() -> eframe::Result {
    env_logger::init();
    startup::fix_path_for_gui_launch();
    // GTK has to be initialised on the main thread BEFORE any
    // gtk/gdk/webkit2gtk object is constructed. wry's Linux backend
    // creates its GTK window in `build_as_child`, so this has to
    // happen before the first Browser pane is built. `gtk::init()` is
    // idempotent-safe but we still only want to pay it once. Failure
    // is non-fatal — the rest of the app doesn't depend on GTK, we
    // just lose the Browser pane backend until the user installs the
    // missing deps (libgtk-3 / libwebkit2gtk-4.1).
    #[cfg(target_os = "linux")]
    if let Err(e) = gtk::init() {
        eprintln!(
            "[crane] gtk::init failed: {e}. Browser pane will be unavailable. \
             Install libgtk-3 + libwebkit2gtk-4.1 and relaunch."
        );
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("Crane");
    if let Some(icon) = startup::load_app_icon() {
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
        Box::new(|cc| {
            // macOS menu bar must be installed from the main thread
            // after NSApp exists. eframe's creation_context callback
            // fires right after window init, which is late enough.
            platform_menu::install();
            #[cfg(target_os = "macos")]
            mac_keys::install_cmd_v_monitor();
            Ok(Box::new(CraneApp::new(cc)))
        }),
    )
}

/// Tab-switcher key dispatch. Cmd+Backtick = next, Cmd+Shift+Backtick
/// = previous. Consumes the key when fired so it doesn't bleed into
/// other shortcut handlers. Opens the overlay or advances the
/// highlight if already open. Returns true if we should skip the
/// generic shortcut handler this frame (i.e. the overlay is active
/// or was just opened).
fn handle_tab_switcher_keys(_ctx: &egui::Context, app: &mut state::App) -> bool {
    // macOS routes Cmd+` / Cmd+~ to its native "cycle windows in app"
    // handler before winit/egui sees the key, so we can't observe it
    // via `ctx.input`. An NSEvent local monitor catches it at the OS
    // level and queues a signed delta here. +N = N forward taps, -N =
    // N backward taps. Sign cancels when the user rocks back-and-
    // forth quickly — the net intent is what ends up on screen.
    #[cfg(target_os = "macos")]
    {
        let delta = mac_keys::drain_pending_tab_cycle();
        if delta != 0 {
            let steps = delta.unsigned_abs() as usize;
            let backward = delta < 0;
            for _ in 0..steps {
                modals::tab_switcher::advance_or_open(app, backward);
            }
            return true;
        }
    }
    app.tab_switcher.is_some()
}

struct CraneApp {
    app: App,
    last_saved_snapshot: String,
    last_saved_settings_snapshot: String,
    last_save_at: std::time::Instant,
    pending_close: Option<state::layout::PaneId>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    browser_host: browser::BrowserHost,
}

impl CraneApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx
            .request_repaint_after(std::time::Duration::from_millis(1500));
        startup::migrate_config_dir();
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
        startup::load_fonts(&cc.egui_ctx, app.custom_mono_font.as_deref());
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        startup::apply_style(&cc.egui_ctx);
        app.update_check.spawn_check(cc.egui_ctx.clone());
        Self {
            app,
            last_saved_snapshot: String::new(),
            last_saved_settings_snapshot: String::new(),
            last_save_at: std::time::Instant::now(),
            pending_close: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            browser_host: browser::BrowserHost::new(),
        }
    }

    /// Land a goto-definition result: open the file in the Files Pane if
    /// it's not already open, and stash the target line/column so the next
    /// render moves the cursor there.
    fn goto_location(&mut self, ctx: &egui::Context, loc: lsp::Location) {
        let path_str = loc.path.to_string_lossy().to_string();
        let mut placed = false;
        if let Some(layout) = self.app.active_layout_ref() {
            for p in layout.panes.values() {
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
            use std::io::Write;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Keep a .bak of the previous good session.json so a
            // corrupt-or-truncated write doesn't wipe every project the
            // user has registered. Rotation: last-known-good stays in
            // .bak; anything older is overwritten.
            if path.exists() {
                let bak = path.with_extension("json.bak");
                let _ = std::fs::copy(&path, &bak);
            }
            // Atomic write: open + write + fsync(tmp) + rename +
            // fsync(parent). Without the fsyncs a crash between the
            // write and the kernel's periodic flush can leave the
            // renamed file zero-length or holding the previous
            // version's data — either way the user loses their
            // session silently.
            let tmp = path.with_extension("json.tmp");
            let written = (|| -> std::io::Result<()> {
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&bytes)?;
                f.sync_all()?;
                Ok(())
            })();
            if written.is_ok()
                && std::fs::rename(&tmp, &path).is_ok()
                && let Some(parent) = path.parent()
                && let Ok(dir) = std::fs::File::open(parent)
            {
                let _ = dir.sync_all();
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

}

impl eframe::App for CraneApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Reset the NSEvent Tab-swallowing gate each frame. The terminal
        // view re-sets it to true later if it still owns focus; otherwise
        // plain Tab / Shift+Tab reach egui normally (file editor indent,
        // focus navigation, etc.). Without this reset, a frame where the
        // terminal pane isn't rendered leaves the previous frame's
        // `true` value stuck and Tab gets eaten in the file editor.
        #[cfg(target_os = "macos")]
        crate::mac_keys::set_terminal_focused(false);
        // Native menu events (macOS). On other platforms this returns
        // an empty Vec and does nothing.
        for id in platform_menu::drain_events() {
            match id.as_str() {
                platform_menu::ID_SETTINGS => self.app.show_settings = true,
                platform_menu::ID_SHORTCUTS => self.app.show_help = true,
                platform_menu::ID_CHECK_UPDATES => {
                    self.app.update_check.dismissed_this_session = None;
                    self.app.update_check.available = None;
                    self.app.update_check.manual_check = true;
                    self.app.update_check.spawn_check(ctx.clone());
                }
                _ => {}
            }
        }
        self.app.ensure_initial(&ctx);
        self.app.sync_tab_mru();
        // Tab switcher (Cmd+~ / Cmd+Shift+~) runs before the generic
        // shortcut handler — the overlay owns the key while it's open
        // so no other Cmd-chord fires during cycling.
        let switcher_consumed = handle_tab_switcher_keys(&ctx, &mut self.app);
        if !switcher_consumed {
            shortcuts::handle(&ctx, &mut self.app, &mut self.pending_close);
        }
        // Shell exited (user typed `exit`, Ctrl-D, kill, etc.) — drop
        // dead tabs from each Terminal Pane. If the last tab in a pane
        // dies, close the whole pane. No confirm prompt: the process
        // is already gone.
        if let Some(ws) = self.app.active_layout() {
            let mut dead_panes: Vec<state::layout::PaneId> = Vec::new();
            for (id, pane) in ws.panes.iter_mut() {
                if let state::layout::PaneContent::Terminal(tp) = &mut pane.content {
                    tp.tabs.retain(|t| t.terminal.is_alive());
                    if tp.tabs.is_empty() {
                        dead_panes.push(*id);
                    } else if tp.active >= tp.tabs.len() {
                        tp.active = tp.tabs.len() - 1;
                    }
                }
            }
            for id in dead_panes {
                ws.focus = Some(id);
                ws.close_focused();
            }
        }
        self.app.refresh_active_git_status(&ctx);
        // If the user just turned off a language in Settings, stop its
        // server process now instead of waiting for app exit. Cheap no-op
        // when every running server is still enabled.
        self.app.lsp.shutdown_disabled(&self.app.language_configs);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            // Fold any URL changes the webview reported (redirects,
            // link clicks, history manipulation) back into each tab's
            // state so the URL bar tracks in-page navigation.
            let updates = self.browser_host.drain_url_updates();
            if !updates.is_empty() {
                browser::apply_url_updates_to_app(&mut self.app, &updates);
            }
            // Seed per-frame loading snapshot so the egui tab chips can
            // show a spinner on tabs whose webview is mid-load.
            browser::set_loading_snapshot(self.browser_host.loading_set());
            // Also surface the current webview memory usage so the
            // Browser pane can warn the user about heavy tabs.
            browser::set_memory_snapshot(self.browser_host.memory.snapshot());
            // Page-load callbacks fire on a background thread; without
            // an explicit repaint the spinner wouldn't animate until
            // the next user event.
            if !self.browser_host.loading_set().is_empty() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        // Pump GTK's event loop so webkit2gtk's async callbacks
        // (page-load events, navigation deltas, IPC from injected
        // scripts) actually dispatch. winit's loop doesn't drive GTK,
        // so without this the webview would appear frozen — pages
        // would load but the URL bar / spinner / nav handlers would
        // never fire. `events_pending + iteration_do(false)` drains
        // without blocking; we rely on ctx.request_repaint* elsewhere
        // to wake the UI for async callback-driven redraws.
        #[cfg(target_os = "linux")]
        {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
        }

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
            ui::projects::render(&mut left_ui, &mut self.app, &ctx);

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
            ui::explorer::render(&mut right_ui, &mut self.app);

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
            for p in layout_ref.panes.values() {
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
        // When any modal is open, disable the panes underneath so
        // clicks/keys don't leak through to terminals or editors.
        let modal_open = self.app.show_settings
            || self.app.show_help
            || self.app.new_workspace_modal.is_some()
            || !self.app.missing_project_modals.is_empty()
            || self.pending_close.is_some();
        if modal_open {
            center_ui.disable();
        }
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
                            Some(state::layout::PaneContent::Terminal(t)) if t.tabs.iter().any(|x| x.terminal.has_foreground_process())
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
                    PaneAction::ToggleMaximize(id) => {
                        ws.maximized = if ws.maximized == Some(id) {
                            None
                        } else {
                            ws.focus = Some(id);
                            Some(id)
                        };
                    }
                    PaneAction::ReplaceWithTerminal(id) => {
                        // Focus the pane first so replace_focused_content
                        // operates on the right slot, then spawn the PTY
                        // against the layout's cwd (the workspace root).
                        ws.focus = Some(id);
                        let cwd = ws.cwd.clone();
                        if let Ok(term) = crate::terminal::Terminal::spawn(
                            ctx.clone(),
                            80,
                            24,
                            Some(&cwd),
                        ) {
                            ws.replace_focused_content(
                                state::layout::PaneContent::Terminal(
                                    state::layout::TerminalPane::single(term),
                                ),
                                "Terminal".into(),
                            );
                        }
                    }
                    PaneAction::ReplaceWithBrowser(id) => {
                        ws.focus = Some(id);
                        let browser =
                            state::layout::BrowserPane::new_with(
                                String::new(),
                                "https://".into(),
                            );
                        ws.replace_focused_content(
                            state::layout::PaneContent::Browser(browser),
                            "Browser".into(),
                        );
                    }
                    PaneAction::ShowFilesPanel => {
                        self.app.show_right = true;
                    }
                }
            }
        } else {
            render_empty_state(&mut center_ui, &mut self.app, &ctx, inset);
        }

        render_missing_project_modal(&ctx, &mut self.app);
        render_new_workspace_modal(&ctx, &mut self.app);
        render_help_modal(&ctx, &mut self.app);
        let settings_effect = render_settings_modal(&ctx, &mut self.app, startup::apply_style);
        if matches!(settings_effect, modals::settings::SettingsEffect::ReloadFonts) {
            startup::load_fonts(&ctx, self.app.custom_mono_font.as_deref());
        }
        self.render_confirm_close(&ctx);
        modals::render_confirm_remove_worktree(&ctx, &mut self.app);
        modals::render_confirm_close_tab(&ctx, &mut self.app);
        modals::render_confirm_delete_file(&ctx, &mut self.app);
        let _ = modals::tab_switcher::render(&ctx, &mut self.app);
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
            // Any open Diff pane targeting this file needs its right-side
            // text refreshed so the shown diff reflects the new content
            // instead of the stale snapshot we read when the diff was
            // first opened. Left side (HEAD) is re-read too in case the
            // user committed between opens.
            self.app.refresh_diff_panes_for_path(&path, &text);
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

        // Drive embedded webviews: drain whatever the Browser panes
        // reported during render_layout, then reconcile webview
        // positions / creations / destructions against the hosting
        // OS window (NSWindow on macOS, X11 child window on Linux).
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let bridge = browser::take_bridge();
            // WKWebView always paints above egui. Any overlay (modal,
            // tooltip, popup, combo menu, drag ghost) would render
            // behind the webview — hide the webviews while any non-
            // background egui layer is visible this frame.
            let overlay_visible = self.app.show_settings
                || self.app.show_help
                || self.app.new_workspace_modal.is_some()
                || self.app.pending_remove_worktree.is_some()
                || self.app.pending_close_tab.is_some()
                || self.app.pending_delete_file.is_some()
                || !self.app.missing_project_modals.is_empty()
                || self.pending_close.is_some()
                || ctx.memory(|m| {
                    // Only count overlays that actually painted this
                    // frame — top_layer_id alone picks up stale, hidden
                    // Areas and would keep the webview forever hidden.
                    let areas = m.areas();
                    let check = |order: egui::Order| {
                        areas
                            .top_layer_id(order)
                            .map(|lid| areas.visible_last_frame(&lid))
                            .unwrap_or(false)
                    };
                    check(egui::Order::Tooltip) || check(egui::Order::Foreground)
                });
            let all_keys = browser::collect_all_keys(&self.app);
            self.browser_host
                .sync(frame, &ctx, bridge, overlay_visible, &all_keys);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = frame;
        }

        self.maybe_save();
    }
}


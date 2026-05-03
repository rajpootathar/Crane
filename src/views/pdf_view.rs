//! PDF viewer — file tab inside the Files Pane.
//!
//! Renders via libpdfium.dylib (bound at runtime through pdfium-render).
//! Mirrors the markdown-preview / image-tab pattern: extension routes
//! the file_view dispatcher here instead of the syntect text path.
//!
//! v1 scope: page navigation, zoom, single-page text select + Cmd+C,
//! Open Externally button. No find, no cross-page selection, no
//! annotations. Encrypted/corrupt PDFs surface an error and the
//! Open Externally button as the only action.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use egui::{
    Color32, ColorImage, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use pdfium_render::prelude::*;

use crate::theme;

/// Zoom presets the user cycles through with Cmd+= / Cmd+-.
const ZOOM_PRESETS: &[f32] = &[0.50, 0.75, 1.00, 1.25, 1.50, 2.00, 3.00, 4.00];
const DEFAULT_ZOOM: f32 = 1.00;
const PAGE_GAP: f32 = 12.0;
/// Keep textures within this many pages of the current viewport top.
const TEXTURE_KEEP_RADIUS: usize = 5;

pub struct PdfTabState {
    pub path: PathBuf,
    pub doc: Option<PdfDocument<'static>>,
    pub page_count: usize,
    pub current_page: usize,
    pub zoom: f32,
    /// Texture cache keyed by (page_idx, zoom_bucket). The bucket is
    /// `(zoom * 100.0) as u32` so two textures at the same zoom share
    /// the same cache slot, while distinct zooms don't.
    pub texture_cache: HashMap<(usize, u32), TextureHandle>,
    /// Per-page char rects, lazy-loaded for selection hit-testing.
    pub page_text: HashMap<usize, PageText>,
    /// Page sizes in PDF points (1 point = 1/72 inch).
    pub page_size_pts: Vec<(f32, f32)>,
    pub selection: Option<Selection>,
    /// In-progress drag — (page_idx, image-relative pos in pixels).
    pub drag: Option<(usize, Pos2)>,
    pub error: Option<String>,
}

pub struct Selection {
    pub page: usize,
    pub start_char: usize,
    pub end_char: usize,
}

pub struct PageText {
    pub chars: Vec<CharInfo>,
}

pub struct CharInfo {
    pub unicode: char,
    /// Bounds in PDF points, origin bottom-left.
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
}

impl PdfTabState {
    pub fn new(path: PathBuf) -> Self {
        let mut state = Self {
            path,
            doc: None,
            page_count: 0,
            current_page: 0,
            zoom: DEFAULT_ZOOM,
            texture_cache: HashMap::new(),
            page_text: HashMap::new(),
            page_size_pts: Vec::new(),
            selection: None,
            drag: None,
            error: None,
        };
        state.try_load();
        state
    }

    fn try_load(&mut self) {
        let pdfium = match get_pdfium() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(format!("PDF rendering unavailable: {e}"));
                return;
            }
        };
        let path_str = self.path.to_string_lossy().into_owned();
        match pdfium.load_pdf_from_file(&path_str, None) {
            Ok(doc) => {
                let pages = doc.pages();
                let n = pages.len() as usize;
                let mut sizes = Vec::with_capacity(n);
                for i in 0..n {
                    if let Ok(p) = pages.get(i as i32) {
                        sizes.push((p.width().value, p.height().value));
                    } else {
                        sizes.push((612.0, 792.0)); // Letter fallback
                    }
                }
                self.page_count = n;
                self.page_size_pts = sizes;
                self.doc = Some(doc);
            }
            Err(e) => {
                let msg = format!("{e}");
                self.error = Some(if msg.to_lowercase().contains("password") {
                    "This PDF is password-protected.".into()
                } else {
                    format!("Could not open PDF: {msg}")
                });
            }
        }
    }
}

/// Lazy global pdfium binding. Tries the bundled framework first
/// (production), then the dev `vendor/pdfium/<arch>/` path, then the
/// system default. Fails closed — every caller pattern-matches the
/// Result and falls back to the Open Externally UI.
fn get_pdfium() -> Result<&'static Pdfium, String> {
    static INSTANCE: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    let outcome = INSTANCE.get_or_init(|| {
        for path in candidate_pdfium_paths() {
            if path.exists() {
                let path_str = path.to_string_lossy().into_owned();
                if let Ok(bindings) = Pdfium::bind_to_library(&path_str) {
                    log::info!("pdfium: bound to {}", path.display());
                    return Ok(Pdfium::new(bindings));
                }
            }
        }
        Pdfium::bind_to_system_library()
            .map(Pdfium::new)
            .map_err(|e| format!("could not bind to libpdfium: {e}"))
    });
    match outcome {
        Ok(p) => Ok(p),
        Err(e) => Err(e.clone()),
    }
}

fn candidate_pdfium_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Production bundle: Crane.app/Contents/MacOS/crane
            //   → Crane.app/Contents/Frameworks/libpdfium.dylib
            paths.push(dir.join("../Frameworks/libpdfium.dylib"));
        }
    }
    // Dev: relative to CWD.
    paths.push(PathBuf::from(format!(
        "vendor/pdfium/{}/libpdfium.dylib",
        host_arch()
    )));
    paths
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    }
}

/// Top-level render entry point. Called from `file_view::render_files`
/// when the active tab's extension is `.pdf`.
pub fn render_pdf(ui: &mut Ui, state: &mut PdfTabState) {
    render_toolbar(ui, state);
    ui.add_space(4.0);

    if state.error.is_some() {
        render_error_panel(ui, state);
        return;
    }
    if state.doc.is_none() {
        ui.label("Loading...");
        return;
    }

    handle_keyboard(ui, state);

    let avail_h = ui.available_height().max(80.0);
    let scroll_id = ("pdf_scroll", state.path.to_string_lossy().into_owned());
    ScrollArea::both()
        .id_salt(scroll_id)
        .auto_shrink([false; 2])
        .max_height(avail_h)
        .show(ui, |ui| {
            for page_idx in 0..state.page_count {
                render_page(ui, state, page_idx);
                ui.add_space(PAGE_GAP);
            }
        });

    evict_textures(state);
}

fn render_toolbar(ui: &mut Ui, state: &mut PdfTabState) {
    use egui_phosphor::regular as ph;
    let t = theme::current();

    ui.horizontal(|ui| {
        // Prev / page indicator / next.
        let can_nav = state.error.is_none() && state.page_count > 0;

        let prev = ui.add_enabled(
            can_nav && state.current_page > 0,
            egui::Button::new(RichText::new(ph::CARET_LEFT).size(13.0))
                .min_size(Vec2::new(28.0, 24.0)),
        );
        if prev.clicked() && state.current_page > 0 {
            state.current_page -= 1;
        }

        if can_nav {
            ui.label(
                RichText::new(format!("{} / {}", state.current_page + 1, state.page_count))
                    .size(11.5)
                    .color(t.text.to_color32()),
            );
        } else {
            ui.label(
                RichText::new("—")
                    .size(11.5)
                    .color(t.text_muted.to_color32()),
            );
        }

        let next = ui.add_enabled(
            can_nav && state.current_page + 1 < state.page_count,
            egui::Button::new(RichText::new(ph::CARET_RIGHT).size(13.0))
                .min_size(Vec2::new(28.0, 24.0)),
        );
        if next.clicked() && state.current_page + 1 < state.page_count {
            state.current_page += 1;
        }

        ui.add_space(8.0);

        // Zoom out / value / zoom in.
        let zoom_out = ui.add_enabled(
            can_nav,
            egui::Button::new(RichText::new(ph::MINUS).size(13.0))
                .min_size(Vec2::new(28.0, 24.0)),
        );
        if zoom_out.clicked() {
            state.zoom = step_zoom(state.zoom, -1);
        }
        ui.label(
            RichText::new(format!("{}%", (state.zoom * 100.0).round() as i32))
                .size(11.5)
                .color(t.text.to_color32()),
        );
        let zoom_in = ui.add_enabled(
            can_nav,
            egui::Button::new(RichText::new(ph::PLUS).size(13.0))
                .min_size(Vec2::new(28.0, 24.0)),
        );
        if zoom_in.clicked() {
            state.zoom = step_zoom(state.zoom, 1);
        }

        // Right-anchored: Open Externally.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let open_btn = ui.add(
                egui::Button::new(
                    RichText::new(format!("{}  Open Externally", ph::ARROW_SQUARE_OUT))
                        .size(11.5),
                )
                .min_size(Vec2::new(0.0, 24.0)),
            );
            if open_btn.clicked() {
                open_externally(&state.path);
            }
        });
    });
}

fn render_error_panel(ui: &mut Ui, state: &PdfTabState) {
    let t = theme::current();
    let msg = state.error.as_deref().unwrap_or("Unknown error.");
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(msg)
                .size(13.0)
                .color(t.text.to_color32()),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("Use \"Open Externally\" above to view in your system PDF reader.")
                .size(11.0)
                .color(t.text_muted.to_color32()),
        );
    });
}

fn render_page(ui: &mut Ui, state: &mut PdfTabState, page_idx: usize) {
    let (page_w_pts, page_h_pts) = state
        .page_size_pts
        .get(page_idx)
        .copied()
        .unwrap_or((612.0, 792.0));
    let zoom = state.zoom;
    // 1 point = 1/72 inch; render at 96 DPI base × zoom for screen-resolution.
    let scale = 96.0 / 72.0 * zoom;
    let render_w = (page_w_pts * scale).max(1.0) as u32;
    let render_h = (page_h_pts * scale).max(1.0) as u32;
    let bucket = (zoom * 100.0).round() as u32;

    // Lazy-render the page texture.
    if !state.texture_cache.contains_key(&(page_idx, bucket)) {
        if let Some(tex) = render_page_to_texture(
            ui.ctx(),
            state.doc.as_ref(),
            page_idx,
            render_w,
            render_h,
            &state.path,
        ) {
            state.texture_cache.insert((page_idx, bucket), tex);
        }
    }

    let display_size = Vec2::new(render_w as f32, render_h as f32);
    let (rect, response) = ui.allocate_exact_size(display_size, Sense::click_and_drag());

    // Paint a placeholder background so missing textures are visible.
    let t = theme::current();
    let bg = if t.is_dark() {
        Color32::from_rgb(40, 40, 44)
    } else {
        Color32::from_rgb(245, 245, 248)
    };
    ui.painter().rect_filled(rect, 2.0, bg);

    if let Some(tex) = state.texture_cache.get(&(page_idx, bucket)) {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // Subtle frame
    ui.painter().rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, t.border.to_color32()),
        egui::StrokeKind::Inside,
    );

    // Selection drag handling.
    handle_selection(ui, state, page_idx, rect, &response);

    // Paint selection highlights for this page.
    paint_selection_for_page(ui, state, page_idx, rect, scale);

    // Track viewport-top page for current_page indicator.
    let viewport = ui.clip_rect();
    if rect.intersects(viewport) && rect.top() <= viewport.center().y {
        state.current_page = page_idx;
    }
}

/// Convert a screen position inside a page rect to PDF-point coords
/// (origin bottom-left). `scale` is pixels-per-point.
fn screen_to_pdf_pts(screen: Pos2, page_rect: Rect, scale: f32, page_h_pts: f32) -> (f32, f32) {
    let local_x = (screen.x - page_rect.min.x) / scale;
    // PDF origin is bottom-left; egui screen is top-left.
    let local_y_top_down = (screen.y - page_rect.min.y) / scale;
    let local_y_pdf = page_h_pts - local_y_top_down;
    (local_x, local_y_pdf)
}

fn handle_selection(
    _ui: &mut Ui,
    state: &mut PdfTabState,
    page_idx: usize,
    rect: Rect,
    response: &egui::Response,
) {
    if !response.hovered() && !response.dragged() && !response.drag_started() {
        return;
    }
    let scale = current_scale(state, page_idx);
    let page_h_pts = state.page_size_pts.get(page_idx).copied().unwrap_or((612.0, 792.0)).1;

    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            ensure_page_text(state, page_idx);
            let (px, py) = screen_to_pdf_pts(pos, rect, scale, page_h_pts);
            if let Some(idx) = char_hit_test(state.page_text.get(&page_idx), px, py) {
                state.drag = Some((page_idx, pos));
                state.selection = Some(Selection {
                    page: page_idx,
                    start_char: idx,
                    end_char: idx,
                });
            }
        }
    } else if response.dragged() && state.drag.map(|(p, _)| p) == Some(page_idx) {
        if let Some(pos) = response.interact_pointer_pos() {
            let (px, py) = screen_to_pdf_pts(pos, rect, scale, page_h_pts);
            if let Some(idx) = char_hit_test(state.page_text.get(&page_idx), px, py) {
                if let Some(sel) = state.selection.as_mut() {
                    if sel.page == page_idx {
                        sel.end_char = idx;
                    }
                }
            }
        }
    } else if response.drag_stopped() {
        state.drag = None;
    }
}

fn current_scale(state: &PdfTabState, _page_idx: usize) -> f32 {
    96.0 / 72.0 * state.zoom
}

fn ensure_page_text(state: &mut PdfTabState, page_idx: usize) {
    if state.page_text.contains_key(&page_idx) {
        return;
    }
    let Some(doc) = state.doc.as_ref() else {
        return;
    };
    let Ok(page) = doc.pages().get(page_idx as i32) else {
        return;
    };
    let Ok(text) = page.text() else {
        return;
    };
    let mut chars = Vec::new();
    for ch in text.chars().iter() {
        let Some(unicode) = ch.unicode_char() else {
            continue;
        };
        if let Ok(b) = ch.tight_bounds() {
            chars.push(CharInfo {
                unicode,
                left: b.left().value,
                right: b.right().value,
                bottom: b.bottom().value,
                top: b.top().value,
            });
        }
    }
    state.page_text.insert(page_idx, PageText { chars });
}

/// Find the character whose bbox contains the point, falling back to
/// the closest character on the same line if no exact hit.
fn char_hit_test(page_text: Option<&PageText>, x_pts: f32, y_pts: f32) -> Option<usize> {
    let pt = page_text?;
    if pt.chars.is_empty() {
        return None;
    }
    // Direct hit
    for (i, c) in pt.chars.iter().enumerate() {
        if x_pts >= c.left && x_pts <= c.right && y_pts >= c.bottom && y_pts <= c.top {
            return Some(i);
        }
    }
    // Closest by squared distance to char center
    let mut best = (f32::INFINITY, 0usize);
    for (i, c) in pt.chars.iter().enumerate() {
        let cx = (c.left + c.right) * 0.5;
        let cy = (c.bottom + c.top) * 0.5;
        let dx = x_pts - cx;
        let dy = y_pts - cy;
        let d = dx * dx + dy * dy;
        if d < best.0 {
            best = (d, i);
        }
    }
    Some(best.1)
}

fn paint_selection_for_page(ui: &mut Ui, state: &PdfTabState, page_idx: usize, rect: Rect, scale: f32) {
    let Some(sel) = state.selection.as_ref() else {
        return;
    };
    if sel.page != page_idx {
        return;
    }
    let Some(pt) = state.page_text.get(&page_idx) else {
        return;
    };
    let page_h_pts = state.page_size_pts.get(page_idx).copied().unwrap_or((612.0, 792.0)).1;
    let (lo, hi) = (sel.start_char.min(sel.end_char), sel.start_char.max(sel.end_char));
    let highlight = Color32::from_rgba_unmultiplied(80, 140, 240, 80);
    for c in pt.chars.iter().skip(lo).take(hi.saturating_sub(lo) + 1) {
        // PDF bottom-left → egui top-left
        let x0 = rect.min.x + c.left * scale;
        let x1 = rect.min.x + c.right * scale;
        let y0 = rect.min.y + (page_h_pts - c.top) * scale;
        let y1 = rect.min.y + (page_h_pts - c.bottom) * scale;
        let r = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        ui.painter().rect_filled(r, 1.0, highlight);
    }
}

fn handle_keyboard(ui: &mut Ui, state: &mut PdfTabState) {
    let mods_cmd = ui.input(|i| i.modifiers.command);

    if ui.input(|i| i.key_pressed(egui::Key::PageDown)) && state.current_page + 1 < state.page_count {
        state.current_page += 1;
    }
    if ui.input(|i| i.key_pressed(egui::Key::PageUp)) && state.current_page > 0 {
        state.current_page -= 1;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Home)) {
        state.current_page = 0;
    }
    if ui.input(|i| i.key_pressed(egui::Key::End)) && state.page_count > 0 {
        state.current_page = state.page_count - 1;
    }
    if mods_cmd && ui.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
        state.zoom = step_zoom(state.zoom, 1);
    }
    if mods_cmd && ui.input(|i| i.key_pressed(egui::Key::Minus)) {
        state.zoom = step_zoom(state.zoom, -1);
    }
    if mods_cmd && ui.input(|i| i.key_pressed(egui::Key::Num0)) {
        state.zoom = DEFAULT_ZOOM;
    }
    if mods_cmd && ui.input(|i| i.key_pressed(egui::Key::C)) {
        if let Some(text) = selection_text(state) {
            ui.ctx().copy_text(text);
        }
    }
    if mods_cmd && ui.input(|i| i.key_pressed(egui::Key::A)) && state.page_count > 0 {
        let page = state.current_page;
        ensure_page_text(state, page);
        if let Some(pt) = state.page_text.get(&page) {
            if !pt.chars.is_empty() {
                state.selection = Some(Selection {
                    page,
                    start_char: 0,
                    end_char: pt.chars.len() - 1,
                });
            }
        }
    }
}

fn selection_text(state: &PdfTabState) -> Option<String> {
    let sel = state.selection.as_ref()?;
    let pt = state.page_text.get(&sel.page)?;
    let lo = sel.start_char.min(sel.end_char);
    let hi = sel.start_char.max(sel.end_char);
    let mut out = String::new();
    for c in pt.chars.iter().skip(lo).take(hi.saturating_sub(lo) + 1) {
        out.push(c.unicode);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn step_zoom(current: f32, direction: i32) -> f32 {
    // Find the closest preset, then step by `direction`.
    let mut idx = 0usize;
    let mut best = f32::INFINITY;
    for (i, &p) in ZOOM_PRESETS.iter().enumerate() {
        let d = (p - current).abs();
        if d < best {
            best = d;
            idx = i;
        }
    }
    let next = if direction > 0 {
        (idx + 1).min(ZOOM_PRESETS.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    ZOOM_PRESETS[next]
}

fn render_page_to_texture(
    ctx: &egui::Context,
    doc: Option<&PdfDocument<'static>>,
    page_idx: usize,
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Option<TextureHandle> {
    let doc = doc?;
    let page = doc.pages().get(page_idx as i32).ok()?;
    let cfg = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_target_height(height as i32);
    let bitmap = page.render_with_config(&cfg).ok()?;
    let img = bitmap.as_image().ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let name = format!("crane_pdf:{}:{}:{}", path.display(), page_idx, width);
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

fn evict_textures(state: &mut PdfTabState) {
    if state.texture_cache.len() <= 16 {
        return;
    }
    let cur = state.current_page;
    let lo = cur.saturating_sub(TEXTURE_KEEP_RADIUS);
    let hi = cur.saturating_add(TEXTURE_KEEP_RADIUS);
    state
        .texture_cache
        .retain(|(page, _), _| *page >= lo && *page <= hi);
    state
        .page_text
        .retain(|page, _| *page >= lo && *page <= hi);
}

fn open_externally(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";

    if let Err(e) = std::process::Command::new(cmd).arg(path).spawn() {
        log::warn!("open externally failed: {e}");
    }
}

pub fn is_pdf_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

//! `WarpPdfView` — a self-contained warpui `View` that renders a PDF through
//! libpdfium (bound at runtime via `pdfium-render`). The warpui port of old
//! Crane's `views/pdf_view.rs`, following the `WarpImageView` precedent: each
//! page is rendered to an encoded PNG in memory, registered into warpui's
//! asset cache under a stable id, and drawn with the shipped `Image` element.
//!
//! Layout matches old Crane: every page is stacked in a vertical
//! `ClippedScrollable` (pixel-smooth scroll) with `PAGE_GAP` between pages, so
//! the whole document scrolls continuously rather than one page at a time.
//! Zoom re-renders every page at the new scale.
//!
//! v1 registers every page up front (and on zoom). For very large PDFs that is
//! a noticeable open/zoom cost; lazy per-viewport registration (old Crane's
//! `TEXTURE_KEEP_RADIUS`) is the follow-on. Drag text-selection is also
//! deferred — both are the largest pieces and independent of viewing.
//!
//! Registration constraint: `AssetCache::insert_raw_asset_bytes` needs a
//! `&mut ModelContext`, so it only runs from an action/event handler (`new` /
//! `handle_action`), never from `render` (which gets `&AppContext`).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use warpui::assets::asset_cache::{AssetCache, AssetSource};
use warpui::elements::{
    Align, CacheOption, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Expanded, Fill,
    Flex, Image, ParentElement, Radius, Rect, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::vec2f;
use warpui::image_cache::ImageType;
use warpui::{AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext};

use crate::warpui::rect_probe::RectProbe;
use crate::warpui::{icons, theme};

/// Zoom presets the user steps through with the toolbar +/- buttons. Identical
/// to the pre-warpui build.
const ZOOM_PRESETS: &[f32] = &[0.50, 0.75, 1.00, 1.25, 1.50, 2.00, 3.00, 4.00];
const DEFAULT_ZOOM: f32 = 1.00;
/// Vertical gap between stacked pages (old Crane's `PAGE_GAP`).
const PAGE_GAP: f32 = 12.0;
/// Font size for the error / status message.
const BASE: f32 = 14.0;

pub struct WarpPdfView {
    /// Proportional UI font for page indicator / error text.
    ui_font: FamilyId,
    /// Phosphor icon font for the toolbar glyphs.
    icon_font: FamilyId,
    /// Source PDF this view renders.
    path: PathBuf,
    /// Display title (file name).
    title: String,
    /// Page count, resolved once at construction.
    page_count: usize,
    /// Per-page size in PDF points (1 pt = 1/72"), resolved once at
    /// construction and used to size each page's render target + layout box.
    page_sizes: Vec<(f32, f32)>,
    /// Current zoom factor (one of `ZOOM_PRESETS`).
    zoom: f32,
    /// Pixel-smooth vertical scroll position for the stacked-pages column.
    /// Owned per view so each open PDF keeps its own scroll offset.
    scroll_state: ClippedScrollStateHandle,
    /// The scroll viewport's window-space rect, recorded each paint by a
    /// `RectProbe`. `page_view` reads its WIDTH (one frame stale, which is
    /// fine) to fit each page to the pane — otherwise `ConstrainedBox` clamps a
    /// natural-width page to the pane and `.contain()` letterboxes it
    /// vertically, showing large dark gaps between pages.
    viewport_rect: Rc<Cell<RectF>>,
    /// Set when the document can't be opened (missing dylib, encryption,
    /// corruption). When present, `render` shows the error panel and no page
    /// is ever rendered.
    error: Option<String>,
}

/// Toolbar actions. All are unit variants, dispatched from the toolbar's
/// buttons and handled by `TypedActionView::handle_action`, which has the
/// `&mut ViewContext` needed to (re-)register page assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfViewAction {
    ZoomIn,
    ZoomOut,
    OpenExternally,
}

impl WarpPdfView {
    /// Open the PDF at `path`. Resolves page count + sizes once (or records an
    /// error), then renders + registers every page at the default zoom. Both
    /// the metadata read and the page renders happen here, off the render hot
    /// path.
    pub fn new(
        ctx: &mut ViewContext<Self>,
        path: PathBuf,
        ui_font: FamilyId,
        icon_font: FamilyId,
    ) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut view = Self {
            ui_font,
            icon_font,
            path,
            title,
            page_count: 0,
            page_sizes: Vec::new(),
            zoom: DEFAULT_ZOOM,
            scroll_state: ClippedScrollStateHandle::new(),
            viewport_rect: Rc::new(Cell::new(RectF::new(vec2f(0.0, 0.0), vec2f(0.0, 0.0)))),
            error: None,
        };
        view.load_metadata();
        view.register_all_pages(ctx);
        view
    }

    /// Source PDF this view renders. Always `Some` (kept as `Option` to match
    /// the shared pane-persistence interface the other document panes expose).
    pub fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Open the document once to read its page count and per-page sizes. On
    /// failure records a user-facing `error` and leaves `page_count` at 0.
    fn load_metadata(&mut self) {
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
                        sizes.push((612.0, 792.0)); // US Letter fallback
                    }
                }
                self.page_count = n;
                self.page_sizes = sizes;
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

    /// The pixel scale from PDF points at the current zoom (1 pt = 1/72"; render
    /// at 96 DPI base × zoom for screen resolution — matches old Crane).
    fn scale(&self) -> f32 {
        96.0 / 72.0 * self.zoom
    }

    /// Zoom, bucketed to an integer percent so the asset id is stable within a
    /// preset and a different preset is a deliberate cache miss.
    fn zoom_bucket(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }

    /// Stable asset-cache id for the current zoom's `page_idx`.
    fn page_asset_id(&self, page_idx: usize) -> String {
        pdf_asset_id(&self.path, page_idx, self.zoom_bucket())
    }

    /// Render one page to encoded PNG bytes. Re-opens the document each call
    /// (pdfium loads pages lazily, and this only runs on open / zoom, never per
    /// frame), so no non-`Send` document handle is held on the view. Returns
    /// `None` on any pdfium/encode failure — the caller skips registration and
    /// `Image` paints its load-failure fallback.
    fn render_page_to_png(&self, page_idx: usize) -> Option<Vec<u8>> {
        let (w_pts, h_pts) = self.page_sizes.get(page_idx).copied()?;
        let scale = self.scale();
        let w = (w_pts * scale).max(1.0) as i32;
        let h = (h_pts * scale).max(1.0) as i32;

        let pdfium = get_pdfium().ok()?;
        let path_str = self.path.to_string_lossy().into_owned();
        let doc = pdfium.load_pdf_from_file(&path_str, None).ok()?;
        let page = doc.pages().get(page_idx as i32).ok()?;
        let cfg = PdfRenderConfig::new()
            .set_target_width(w)
            .set_target_height(h);
        let bitmap = page.render_with_config(&cfg).ok()?;
        let img = bitmap.as_image().ok()?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        Some(png)
    }

    /// Render + register `page_idx`'s asset at the current zoom. A no-op in an
    /// error state or out of range. Always re-renders (never trusts a cached
    /// flag) because the global asset cache evicts Raw assets over its byte
    /// budget — so re-registering is what keeps a scrolled-back page from going
    /// permanently blank.
    fn register_page(&mut self, page_idx: usize, ctx: &mut ViewContext<Self>) {
        if self.error.is_some() || page_idx >= self.page_count {
            return;
        }
        let id = self.page_asset_id(page_idx);
        let Some(png) = self.render_page_to_png(page_idx) else {
            return;
        };
        AssetCache::handle(ctx).update(ctx, move |cache, cctx| {
            cache.insert_raw_asset_bytes::<ImageType>(id, &png, cctx);
        });
    }

    /// Register every page at the current zoom. Called on open and after a zoom
    /// change (page dimensions changed, so every cached bitmap is stale).
    fn register_all_pages(&mut self, ctx: &mut ViewContext<Self>) {
        for i in 0..self.page_count {
            self.register_page(i, ctx);
        }
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    /// A single toolbar icon button (min 24×22 hit box per the icon-button
    /// rule). Dispatches its `action` to this view's `handle_action`.
    fn icon_button(&self, glyph: &str, action: PdfViewAction) -> Box<dyn Element> {
        EventHandler::new(
            ConstrainedBox::new(
                Container::new(
                    Align::new(
                        Text::new(glyph.to_string(), self.icon_font, 13.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .with_width(24.0)
            .with_height(22.0)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action);
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    /// A labelled icon+text button (Open Externally), with a subtle accent
    /// background so it reads as an action rather than a bare glyph.
    fn labelled_button(&self, glyph: &str, label: &str, action: PdfViewAction) -> Box<dyn Element> {
        EventHandler::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Container::new(
                            Text::new(glyph.to_string(), self.icon_font, 12.0)
                                .with_color(theme::text())
                                .finish(),
                        )
                        .with_padding_right(5.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new(label.to_string(), self.ui_font, 11.5)
                            .with_color(theme::text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(theme::accent_soft())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action);
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    /// A plain label (zoom percent / page count) with fixed side padding.
    fn label(&self, text: String) -> Box<dyn Element> {
        Container::new(
            Text::new(text, self.ui_font, 11.5)
                .with_color(theme::text())
                .finish(),
        )
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .finish()
    }

    /// A fixed-size, transparent spacer. A bare `Rect::new()` here would be a
    /// NON-flex row child and echo an unbounded main-axis constraint → infinite
    /// rect; the `ConstrainedBox` pins it finite.
    fn spacer(w: f32, h: f32) -> Box<dyn Element> {
        ConstrainedBox::new(Container::new(Rect::new().finish()).finish())
            .with_width(w)
            .with_height(h)
            .finish()
    }

    /// The toolbar row: zoom-out · % · zoom-in · gap · page count · (flex
    /// spacer) · Open Externally.
    fn toolbar(&self) -> Box<dyn Element> {
        let pages_label = if self.page_count == 1 {
            "1 page".to_string()
        } else {
            format!("{} pages", self.page_count)
        };
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.icon_button(icons::MINUS, PdfViewAction::ZoomOut))
            .with_child(self.label(format!("{}%", (self.zoom * 100.0).round() as i32)))
            .with_child(self.icon_button(icons::PLUS, PdfViewAction::ZoomIn))
            .with_child(Self::spacer(10.0, 1.0))
            .with_child(self.label(pages_label))
            // Flexible spacer pushes Open Externally to the right edge.
            .with_child(Expanded::new(1.0, Container::new(Rect::new().finish()).finish()).finish())
            .with_child(self.labelled_button(
                icons::ARROW_SQUARE_OUT,
                "Open Externally",
                PdfViewAction::OpenExternally,
            ));
        // Fixed height: the toolbar is a NON-flex child of `render`'s column, so
        // it would otherwise receive an unbounded main-axis constraint that the
        // flexible spacer's `Rect` echoes back to infinity.
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(theme::topbar_bg())
                .finish(),
        )
        .with_height(32.0)
        .finish()
    }

    /// One page, drawn from its registered `Raw` asset. Sized at its natural
    /// render width, but capped to the measured viewport width so it never
    /// exceeds the pane — the height is then set PROPORTIONAL to that display
    /// width, so the box aspect matches the bitmap and `.contain()` fills it
    /// with no vertical letterbox (the dark inter-page gap bug). A page wider
    /// than the pane is fit-to-width rather than clipped; horizontal scroll for
    /// zoom-beyond-pane is a follow-on.
    fn page_view(&self, page_idx: usize) -> Box<dyn Element> {
        let (w_pts, h_pts) = self
            .page_sizes
            .get(page_idx)
            .copied()
            .unwrap_or((612.0, 792.0));
        let aspect = if w_pts > 0.0 { h_pts / w_pts } else { 792.0 / 612.0 };
        let natural_w = (w_pts * self.scale()).max(1.0);
        // Leave room for the overlay scrollbar + a little breathing space. On
        // the very first frame the probe hasn't recorded yet (width 0) — fall
        // back to natural width; the next frame corrects it.
        let avail = self.viewport_rect.get().width() - 16.0;
        let disp_w = if avail > 1.0 { natural_w.min(avail) } else { natural_w };
        let disp_h = (disp_w * aspect).max(1.0);
        let id = self.page_asset_id(page_idx);
        let img = Image::new(AssetSource::Raw { id }, CacheOption::Original)
            .contain()
            .on_load_failure(self.centered_message("Couldn't render this page."))
            .finish();
        ConstrainedBox::new(img)
            .with_width(disp_w)
            .with_height(disp_h)
            .finish()
    }

    /// The full document: every page stacked in a column with `PAGE_GAP`
    /// between, centred on the cross axis so narrow pages sit mid-pane and
    /// wide (high-zoom) pages centre-clip.
    fn pages_column(&self) -> Box<dyn Element> {
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for i in 0..self.page_count {
            if i > 0 {
                col = col.with_child(Self::spacer(1.0, PAGE_GAP));
            }
            col = col.with_child(self.page_view(i));
        }
        col.finish()
    }

    /// The error panel: the failure message plus an Open Externally button so a
    /// document Crane can't render is still reachable in the system viewer.
    fn error_panel(&self, msg: &str) -> Box<dyn Element> {
        Flex::column()
            .with_child(
                Container::new(
                    Text::new(msg.to_string(), self.ui_font, BASE)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_uniform_padding(16.0)
                .finish(),
            )
            .with_child(
                Container::new(self.labelled_button(
                    icons::ARROW_SQUARE_OUT,
                    "Open Externally",
                    PdfViewAction::OpenExternally,
                ))
                .with_padding_left(16.0)
                .finish(),
            )
            .finish()
    }

    /// A muted, padded message (used for the no-pages and per-page-failure
    /// states).
    fn centered_message(&self, msg: &str) -> Box<dyn Element> {
        Container::new(
            Text::new(msg.to_string(), self.ui_font, BASE)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_uniform_padding(16.0)
        .finish()
    }

    /// Outer panel: a background Rect under the content (mirrors
    /// `WarpImageView::panel`).
    fn panel(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(content)
            .finish()
    }
}

impl Entity for WarpPdfView {
    type Event = ();
}

impl TypedActionView for WarpPdfView {
    type Action = PdfViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            PdfViewAction::ZoomIn => {
                let z = step_zoom(self.zoom, 1);
                if z != self.zoom {
                    self.zoom = z;
                    self.register_all_pages(ctx);
                    ctx.notify();
                }
            }
            PdfViewAction::ZoomOut => {
                let z = step_zoom(self.zoom, -1);
                if z != self.zoom {
                    self.zoom = z;
                    self.register_all_pages(ctx);
                    ctx.notify();
                }
            }
            PdfViewAction::OpenExternally => open_externally(&self.path),
        }
    }
}

impl View for WarpPdfView {
    fn ui_name() -> &'static str {
        "WarpPdfView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        let body: Box<dyn Element> = if let Some(err) = &self.error {
            self.error_panel(err)
        } else if self.page_count == 0 {
            self.centered_message("This PDF has no pages.")
        } else {
            // All pages stacked in a pixel-smooth vertical scroll view (old
            // Crane's `ScrollArea` of stacked pages). The `RectProbe` records
            // the viewport width each paint so `page_view` can fit pages to the
            // pane (see its doc) — one frame stale, which is imperceptible.
            let scroll = ClippedScrollable::vertical(
                self.scroll_state.clone(),
                self.pages_column(),
                ScrollbarWidth::Auto,
                Fill::Solid(theme::border()),
                Fill::Solid(theme::text_muted()),
                Fill::None,
            )
            .finish();
            Box::new(RectProbe::new(scroll, self.viewport_rect.clone()))
        };
        let content = Flex::column()
            .with_child(self.toolbar())
            .with_child(Expanded::new(1.0, body).finish())
            .finish();
        self.panel(content)
    }
}

// ── pdfium binding (ported from d578f1a:src/views/pdf_view.rs) ────────────────

/// Lazy global pdfium binding. Tries the bundled framework first (production),
/// then the dev `vendor/pdfium/<arch>/` path, then the system default. Fails
/// closed — every caller pattern-matches the `Result` and falls back to the
/// error panel / Open Externally UI, never panicking.
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
    // Dev: relative to CWD (repo root during `cargo run` / `cargo test`).
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

/// Stable asset-cache id for a `(path, page, zoom-bucket)`. Keyed on the path
/// so two PDFs can't collide, on page so pages don't share a bitmap, and on the
/// zoom bucket so a zoom change is a deliberate cache miss re-rendered at scale.
/// Free function so the cache-key invariant is unit-testable without a live view.
fn pdf_asset_id(path: &Path, page_idx: usize, zoom_bucket: u32) -> String {
    format!("crane_pdf:{}:{}:{}", path.display(), page_idx, zoom_bucket)
}

/// Step to the adjacent zoom preset in `direction` (+1 / -1), snapping to the
/// nearest preset first. Pure — unit-tested below.
fn step_zoom(current: f32, direction: i32) -> f32 {
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

fn open_externally(path: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure logic ────────────────────────────────────────────────────────────

    #[test]
    fn step_zoom_walks_the_presets_and_clamps_at_the_ends() {
        assert_eq!(step_zoom(1.00, 1), 1.25);
        assert_eq!(step_zoom(1.00, -1), 0.75);
        // An off-preset value snaps to the nearest before stepping.
        assert_eq!(step_zoom(1.10, 1), 1.25);
        // Clamps at both ends rather than wrapping or overflowing.
        assert_eq!(step_zoom(0.50, -1), 0.50);
        assert_eq!(step_zoom(4.00, 1), 4.00);
    }

    #[test]
    fn asset_id_is_distinct_per_page_and_zoom() {
        // Guards the cache-key invariant (calls the real `pdf_asset_id`, which
        // `page_asset_id` delegates to): a different page OR zoom OR path must
        // key a different asset, so a change is a deliberate cache miss.
        let a = Path::new("/a.pdf");
        let b = Path::new("/b.pdf");
        assert_ne!(pdf_asset_id(a, 0, 100), pdf_asset_id(a, 1, 100)); // page
        assert_ne!(pdf_asset_id(a, 0, 100), pdf_asset_id(a, 0, 150)); // zoom
        assert_ne!(pdf_asset_id(a, 0, 100), pdf_asset_id(b, 0, 100)); // path
        assert_eq!(pdf_asset_id(a, 0, 100), pdf_asset_id(a, 0, 100)); // stable
    }

    // ── Layout (headless scene) ───────────────────────────────────────────────
    //
    // Mirrors `image_view.rs`'s `build_image_scene`: builds a REAL scene through
    // warpui's test platform and runs the full layout+paint pass, whose
    // `Scene::validate_rect` debug-asserts finite rects. NOTE (per the plan):
    // `Image::paint` swallows a failed rect by painting nothing, so "lays out
    // finitely" is NOT "renders visibly" — pixel rendering (and rendering inside
    // a real .app bundle) needs manual verification.

    /// A minimal, valid single-page PDF with a correct xref table, generated so
    /// the fixture lives in the repo, not on disk outside it.
    fn minimal_pdf() -> Vec<u8> {
        let objs = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R]/Count 1>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>",
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj{}endobj\n", i + 1, body));
        }
        let xref_pos = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n", objs.len() + 1));
        pdf.push_str("0000000000 65535 f \n");
        for off in &offsets {
            pdf.push_str(&format!("{:010} 00000 n \n", off));
        }
        pdf.push_str(&format!(
            "trailer<</Size {}/Root 1 0 R>>\nstartxref\n{}\n%%EOF",
            objs.len() + 1,
            xref_pos
        ));
        pdf.into_bytes()
    }

    fn build_pdf_scene(path: PathBuf) {
        use std::collections::HashSet;

        use warpui::geometry::vector::vec2f;
        use warpui::platform::WindowStyle;
        use warpui::{App, Presenter, WindowInvalidation};

        App::test((), move |mut app| async move {
            let app = &mut app;
            let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                let (ui, icon) = test_fonts(ctx);
                WarpPdfView::new(ctx, path, ui, icon)
            });
            let mut presenter = Presenter::new(window_id);
            let mut updated = HashSet::new();
            updated.insert(app.root_view_id(window_id).unwrap());
            let invalidation = WindowInvalidation { updated, ..Default::default() };
            app.update(move |ctx| {
                presenter.invalidate(invalidation, ctx);
                let _ = presenter.build_scene(vec2f(900.0, 600.0), 1.0, None, ctx);
            });
        });
    }

    /// Load the two fonts the view needs, inside the test window's context.
    fn test_fonts(ctx: &mut ViewContext<WarpPdfView>) -> (FamilyId, FamilyId) {
        let ui = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |c, _| crate::warpui::bundled_fonts::ui(c));
        let icon = warpui::fonts::Cache::handle(ctx).update(ctx, |c, _| {
            c.load_family_from_bytes("phosphor", vec![include_bytes!("assets/Phosphor.ttf").to_vec()])
                .expect("load phosphor")
        });
        (ui, icon)
    }

    #[test]
    fn new_records_path_and_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.pdf");
        std::fs::write(&file, minimal_pdf()).expect("write fixture pdf");

        use warpui::platform::WindowStyle;
        use warpui::App;

        let file_for_view = file.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                let (ui, icon) = test_fonts(ctx);
                WarpPdfView::new(ctx, file_for_view.clone(), ui, icon)
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert_eq!(v.path(), Some(file_for_view.as_path()));
                    assert_eq!(v.title(), "report.pdf");
                });
            });
        });
    }

    #[test]
    fn a_valid_pdf_lays_out_finitely() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, minimal_pdf()).expect("write fixture pdf");
        build_pdf_scene(file);
    }

    #[test]
    fn a_corrupt_pdf_lays_out_finitely_and_does_not_panic() {
        // Same extension, garbage bytes: must hit the error panel, not panic
        // and not produce an infinite rect.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("corrupt.pdf");
        std::fs::write(&file, b"this is not a pdf at all").expect("write garbage file");
        build_pdf_scene(file);
    }

    #[test]
    fn a_missing_pdf_lays_out_finitely_and_does_not_panic() {
        build_pdf_scene(PathBuf::from(
            "/nonexistent/definitely-not-here-crane-pdf-test.pdf",
        ));
    }
}

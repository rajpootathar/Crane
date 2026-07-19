//! `WarpImageView` — a self-contained warpui `View` that renders an image
//! file (png/jpg/gif/bmp/webp/ico/svg) through warpui's shipped `Image`
//! element. The warpui port of old Crane's image-preview behavior from the
//! image-viewer plan. Like `markdown_view.rs`, elements are transient —
//! rebuilt from `render` every frame — but this view's persistent state is
//! trivial: just the source path and a content fingerprint resolved once.
//!
//! The one thing that must never happen: `LocalFileContentVersion::for_path`
//! (`vendor/warp/crates/warpui_core/src/assets/asset_cache.rs`) performs
//! blocking filesystem I/O and its own doc comment says it must only run off
//! the render hot path. `render` here takes `&self` (no mutable access), and
//! only ever reads the already-resolved `self.content_version` — it has no
//! way to call `for_path` even transiently. The single call site is `new`,
//! below.

use std::path::{Path, PathBuf};

use instant::Instant;

use warpui::assets::asset_cache::{AssetSource, LocalFileContentVersion};
use warpui::elements::{
    CacheOption, Container, Element, Expanded, Flex, Image, ParentElement, Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext};

use crate::warpui::theme;

/// Prose font size for the decode-failure fallback message.
const BASE: f32 = 14.0;

pub struct WarpImageView {
    /// Proportional font for the decode-failure fallback text.
    prose: FamilyId,
    /// Source file this view renders.
    path: PathBuf,
    /// Display title (file name).
    title: String,
    /// Content fingerprint, resolved ONCE at construction (see `new`).
    /// `LocalFileContentVersion::for_path` performs blocking filesystem I/O
    /// and must never run on the render path — see the module doc above.
    content_version: Option<LocalFileContentVersion>,
    /// When this view opened — drives animated GIF/WebP playback via
    /// `Image::enable_animation_with_start_time`.
    opened_at: Instant,
}

impl WarpImageView {
    /// Open the image file at `path`. Resolves the content-version
    /// fingerprint exactly once, here — never inside `render`, which warpui
    /// calls every frame (see the module doc).
    pub fn new(ctx: &mut ViewContext<Self>, path: PathBuf) -> Self {
        let prose = Self::font(ctx);
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let content_version = LocalFileContentVersion::for_path(&path);
        Self {
            prose,
            path,
            title,
            content_version,
            opened_at: Instant::now(),
        }
    }

    /// Source file this view renders. Always `Some` — unlike
    /// `WarpMarkdownView`, `WarpImageView` has no in-memory constructor —
    /// kept as `Option` to match the shared pane-persistence interface other
    /// document panes expose.
    pub fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Load the proportional UI font used for the decode-failure message.
    /// Mirrors `WarpMarkdownView::fonts`.
    fn font(ctx: &mut ViewContext<Self>) -> FamilyId {
        warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| crate::warpui::bundled_fonts::ui(cache))
    }

    /// The `Image` element itself, sized by whatever bounded constraint its
    /// wrapping `Expanded` hands it (see `render`) — never given a fixed
    /// width/height here, since the pane can resize.
    fn image_element(&self) -> Box<dyn Element> {
        let source = AssetSource::LocalFile {
            path: self.path.to_string_lossy().into_owned(),
            content_version: self.content_version,
        };
        Image::new(source, CacheOption::Original)
            .contain()
            .enable_animation_with_start_time(self.opened_at)
            .on_load_failure(self.failure_element())
            .finish()
    }

    /// Shown by `Image` in place of the picture when the asset fails to
    /// decode (missing file, corrupt data, unsupported format). Copy matches
    /// the pre-warpui build exactly.
    fn failure_element(&self) -> Box<dyn Element> {
        Container::new(
            Text::new("Couldn't decode image".to_string(), self.prose, BASE)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_uniform_padding(16.0)
        .finish()
    }

    /// Outer panel: a background Rect under the content (mirrors
    /// `WarpMarkdownView::panel`).
    fn panel(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(content)
            .finish()
    }
}

impl Entity for WarpImageView {
    type Event = ();
}

/// No interactive actions in v1 — the image is `.contain()`-fit to the pane
/// with no pan/zoom/scroll. An uninhabited enum still satisfies
/// `App::add_window`'s `T: View + TypedActionView` bound (every warpui view
/// needs one), and can never actually be dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageViewAction {}

impl TypedActionView for WarpImageView {
    type Action = ImageViewAction;
}

impl View for WarpImageView {
    fn ui_name() -> &'static str {
        "WarpImageView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        // `Image::layout` returns `constraint.max` verbatim (unlike
        // `layout_using_paint_bounds`, which we don't set) — the exact same
        // shape as the bare-`Rect`-in-`Flex::column` bug that made
        // `WarpMarkdownView::quote_element` crash (see that method's doc
        // comment): a NON-flexible child of a `Flex::column` gets an
        // UNBOUNDED main-axis constraint, so an element that echoes
        // `constraint.max` straight back goes infinite. Wrapping the image
        // in `Expanded` as the column's one flex child forces a bounded
        // share of the pane's own (always-finite) height instead.
        let content = Flex::column()
            .with_child(Expanded::new(1.0, self.image_element()).finish())
            .finish();
        self.panel(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal, valid 1x1 RGB PNG (69 bytes) — generated once with Python's
    // stdlib zlib/struct and inlined here so the fixture lives in the repo,
    // not on disk outside it.
    const TINY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 218, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 247, 3, 65, 67, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn new_from_a_real_path_records_that_path_for_persistence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("photo.png");
        std::fs::write(&file, TINY_PNG).expect("write fixture png");

        use warpui::platform::WindowStyle;
        use warpui::App;

        let file_for_view = file.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpImageView::new(ctx, file_for_view.clone())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert_eq!(
                        v.path(),
                        Some(file_for_view.as_path()),
                        "a view built via `new` must report its source path, or the pane can \
                         never be found again by the save-side lookup"
                    );
                });
            });
        });
    }

    #[test]
    fn title_is_the_file_name_not_the_full_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("vacation-photo.png");
        std::fs::write(&file, TINY_PNG).expect("write fixture png");

        use warpui::platform::WindowStyle;
        use warpui::App;

        let file_for_view = file.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpImageView::new(ctx, file_for_view.clone())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert_eq!(
                        v.title(),
                        "vacation-photo.png",
                        "title must be the bare file name, not the full path"
                    );
                });
            });
        });
    }

    #[test]
    fn content_version_is_resolved_once_at_construction_for_an_existing_file() {
        // The hot-path guard: `new` must eagerly resolve a content-version
        // fingerprint (blocking `stat`) for a file that actually exists on
        // disk, and never leave it as `None` (`None` means "no invalidation
        // on disk change" — silently stale caching). `render` takes `&self`
        // and can only ever read this already-resolved field.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("photo.png");
        std::fs::write(&file, TINY_PNG).expect("write fixture png");

        use warpui::platform::WindowStyle;
        use warpui::App;

        let file_for_view = file.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpImageView::new(ctx, file_for_view.clone())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert!(
                        v.content_version.is_some(),
                        "an existing file must resolve a Some(_) content version at \
                         construction, or an edited-on-disk file would be served stale"
                    );
                });
            });
        });
    }

    #[test]
    fn content_version_is_none_for_a_path_that_does_not_exist() {
        // `for_path` returns `None` when metadata can't be read (missing
        // file) — must not panic and must not fabricate a version.
        use warpui::platform::WindowStyle;
        use warpui::App;

        let missing = PathBuf::from("/nonexistent/definitely-not-here-crane-image-test.png");
        let missing_for_view = missing.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpImageView::new(ctx, missing_for_view.clone())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert!(
                        v.content_version.is_none(),
                        "a nonexistent path must resolve to no content version, not panic or \
                         fabricate one"
                    );
                });
            });
        });
    }

    // ── Layout regression tests ──────────────────────────────────────────────
    //
    // Mirrors `markdown_view.rs`'s `build_markdown_scene`: builds a REAL scene
    // headlessly (warpui's test platform: stub window manager + stub font
    // DB), running the full layout + paint pass over the rendered element
    // tree. `Scene::validate_rect`
    // (vendor/warp/crates/warpui_core/src/scene.rs:550-574) debug-asserts
    // that no painted rect has a non-finite origin or size — this is the
    // harness that caught the infinite-height crash on this branch.
    fn build_image_scene(path: PathBuf) {
        use std::collections::HashSet;

        use warpui::geometry::vector::vec2f;
        use warpui::platform::WindowStyle;
        use warpui::{App, Presenter, WindowInvalidation};

        App::test((), |mut app| async move {
            let app = &mut app;
            let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpImageView::new(ctx, path)
            });
            let mut presenter = Presenter::new(window_id);
            let mut updated = HashSet::new();
            updated.insert(app.root_view_id(window_id).unwrap());
            let invalidation = WindowInvalidation { updated, ..Default::default() };
            app.update(move |ctx| {
                presenter.invalidate(invalidation, ctx);
                // A concrete, finite window — the pane a Pane image view lives
                // in is always finitely sized; an infinity would be produced
                // INSIDE the view, by `Image` echoing an unbounded incoming
                // constraint straight back (see `render`'s doc comment).
                let _ = presenter.build_scene(vec2f(900.0, 600.0), 1.0, None, ctx);
            });
        });
    }

    #[test]
    fn a_valid_image_lays_out_finitely() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("photo.png");
        std::fs::write(&file, TINY_PNG).expect("write fixture png");
        build_image_scene(file);
    }

    #[test]
    fn an_undecodable_file_lays_out_finitely_and_does_not_panic() {
        // Same extension, garbage bytes: must hit `on_load_failure`'s
        // fallback element (or at worst stay in a loading state), not panic
        // and not produce an infinite rect.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("corrupt.png");
        std::fs::write(&file, b"this is not a png file at all").expect("write garbage file");
        build_image_scene(file);
    }

    #[test]
    fn a_nonexistent_path_lays_out_finitely_and_does_not_panic() {
        build_image_scene(PathBuf::from(
            "/nonexistent/definitely-not-here-crane-image-test.png",
        ));
    }
}

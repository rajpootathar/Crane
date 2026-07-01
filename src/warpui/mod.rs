//! warpui frontend for Crane — the GPU-rendered UI that reuses the rest of the
//! `crane` crate's logic (`crate::git`, `crate::state`, `crate::format`, …) and
//! replaces only the egui rendering layer. Launched from `main()` when the
//! `CRANE_WARP` env var is set; egui remains the default until parity, then the
//! egui path is removed.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use warpui::geometry::vector::vec2f;
use warpui::{platform, AddWindowOptions, AssetProvider};

pub mod color;
pub mod controller;
pub mod editor_view;
pub mod file_pane;
pub mod file_tree;
pub mod git;
pub mod grid_element;
pub mod gutter_element;
pub mod scrollbar_element;
pub mod icons;
pub mod input;
pub mod layout;
pub mod persist;
pub mod projects;
pub mod rect_probe;
pub mod shell;
pub mod split;
pub mod theme;
pub mod view;

use shell::CraneShellView;

#[derive(Clone, Copy, RustEmbed)]
#[folder = "src/warpui/assets"]
pub struct Assets;

pub static ASSETS: Assets = Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|f| f.data)
            .ok_or_else(|| anyhow!("no asset exists at path {}", path))
    }
}

/// Set the macOS Dock / app icon from the bundled `crane.png`. warpui exposes no
/// window-icon API and a bare `cargo run` binary isn't an `.app` bundle, so the
/// Dock shows a generic icon; set it directly on `NSApplication` at startup.
#[cfg(target_os = "macos")]
fn set_app_icon() {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bytes: &[u8] = include_bytes!("../../crane.png");
    let data = NSData::with_bytes(bytes);
    unsafe {
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let app = NSApplication::sharedApplication(mtm);
            app.setApplicationIconImage(Some(&image));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn set_app_icon() {}

/// Run the warpui frontend (owns its own NSApplication/event loop).
pub fn run() {
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
        set_app_icon();
        // Use persisted window size if available; otherwise fall back to the
        // default 1480×920. Minimum size (800×500) is not expressible via
        // WindowBounds::ExactSize — it only sets the initial size.
        // TODO min-size: wire up once warpui exposes a set_min_size API.
        let initial_size = persist::load()
            .filter(|st| st.window_w > 0.0 && st.window_h > 0.0)
            .map(|st| vec2f(st.window_w, st.window_h))
            .unwrap_or_else(|| vec2f(1480.0, 920.0));
        ctx.add_window(
            AddWindowOptions {
                title: Some("Crane".to_string()),
                window_bounds: platform::WindowBounds::ExactSize(initial_size),
                ..Default::default()
            },
            CraneShellView::new,
        );
    });
}

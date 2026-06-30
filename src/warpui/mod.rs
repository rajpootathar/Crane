//! warpui frontend for Crane — the GPU-rendered UI that reuses the rest of the
//! `crane` crate's logic (`crate::git`, `crate::state`, `crate::format`, …) and
//! replaces only the egui rendering layer. Launched from `main()` when the
//! `CRANE_WARP` env var is set; egui remains the default until parity, then the
//! egui path is removed.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use warpui::{platform, AddWindowOptions, AssetProvider};

pub mod color;
pub mod controller;
pub mod file_pane;
pub mod file_tree;
pub mod git;
pub mod grid_element;
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

/// Run the warpui frontend (owns its own NSApplication/event loop).
pub fn run() {
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
        ctx.add_window(
            AddWindowOptions {
                title: Some("Crane".to_string()),
                ..Default::default()
            },
            CraneShellView::new,
        );
    });
}

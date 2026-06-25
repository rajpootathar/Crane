//! warpui terminal + app-shell prototype for Crane. Two binaries share
//! these modules: `warp_term_spike` (terminal only) and `shell` (the full
//! app-shell layout with the terminal docked in the center).

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use warpui::AssetProvider;

pub mod color;
pub mod controller;
pub mod grid_element;
pub mod input;
pub mod shell;
pub mod theme;
pub mod view;

#[derive(Clone, Copy, RustEmbed)]
#[folder = "assets"]
pub struct Assets;

pub static ASSETS: Assets = Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|f| f.data)
            .ok_or_else(|| anyhow!("no asset exists at path {}", path))
    }
}

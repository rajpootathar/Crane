//! PTY-backed terminal: alacritty parser + egui grid renderer.

mod grid_snap;
mod sync_handler;
mod term;
pub mod view;

pub use term::*;

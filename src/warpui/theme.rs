//! Crane warpui colour tokens — all values come from the global `crate::theme::current()`
//! so themes switch at runtime without restarting the process.

use warpui::color::ColorU;

fn rgb(c: crate::theme::Rgb) -> ColorU {
    ColorU { r: c.r, g: c.g, b: c.b, a: 255 }
}

pub fn bg() -> ColorU            { rgb(crate::theme::current().bg) }
pub fn sidebar_bg() -> ColorU    { rgb(crate::theme::current().sidebar_bg) }
pub fn topbar_bg() -> ColorU     { rgb(crate::theme::current().topbar_bg) }
pub fn surface() -> ColorU       { rgb(crate::theme::current().surface) }
pub fn border() -> ColorU        { rgb(crate::theme::current().border) }
pub fn divider() -> ColorU       { rgb(crate::theme::current().divider) }
pub fn text() -> ColorU          { rgb(crate::theme::current().text) }
pub fn text_hover() -> ColorU    { rgb(crate::theme::current().text_hover) }
pub fn text_muted() -> ColorU    { rgb(crate::theme::current().text_muted) }
pub fn text_header() -> ColorU   { rgb(crate::theme::current().text_header) }
pub fn accent() -> ColorU        { rgb(crate::theme::current().accent) }
pub fn row_active() -> ColorU    { rgb(crate::theme::current().row_active) }
pub fn row_hover() -> ColorU     { rgb(crate::theme::current().row_hover) }
pub fn focus_border() -> ColorU  { rgb(crate::theme::current().focus_border) }
pub fn error() -> ColorU         { rgb(crate::theme::current().error) }
pub fn success() -> ColorU       { rgb(crate::theme::current().success) }
pub fn warning() -> ColorU       { rgb(crate::theme::current().warning) }

/// Text selection highlight background.
pub fn selection() -> ColorU {
    let c = crate::theme::current().selection;
    ColorU { r: c.r, g: c.g, b: c.b, a: 180 }
}

/// Translucent accent for drag drop-zone overlays.
pub fn drop_zone() -> ColorU {
    let c = crate::theme::current().accent;
    ColorU { r: c.r, g: c.g, b: c.b, a: 70 }
}

/// Translucent dim over inactive panes (currently unused — no dim mode).
pub fn pane_dim() -> ColorU {
    let c = crate::theme::current().bg;
    ColorU { r: c.r, g: c.g, b: c.b, a: 120 }
}

// Panel dimensions — not colours, never change with themes.
pub const TOPBAR_H: f32 = 34.0;
pub const STATUS_H: f32 = 28.0;
pub const LEFT_W: f32   = 240.0;
pub const RIGHT_W: f32  = 300.0;

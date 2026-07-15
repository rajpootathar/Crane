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
///
/// Prefer the theme's dedicated `selection` field, rendered opaque. Custom
/// themes may omit it (serde default = Rgb(0,0,0)) — in that case fall back
/// to the historical accent-at-~28%-alpha derivation so old theme files keep
/// working without modification.
pub fn selection() -> ColorU {
    let s = crate::theme::current().selection;
    if s.r == 0 && s.g == 0 && s.b == 0 {
        let a = crate::theme::current().accent;
        ColorU { r: a.r, g: a.g, b: a.b, a: 72 }
    } else {
        ColorU { r: s.r, g: s.g, b: s.b, a: 255 }
    }
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

/// White-alpha overlay washes — the app-wide hover/selection language.
/// Alphas are on-white overlays so they read identically on every theme.
pub fn hover_wash() -> ColorU     { ColorU { r: 255, g: 255, b: 255, a: 9 }  }
pub fn selection_wash() -> ColorU { ColorU { r: 255, g: 255, b: 255, a: 18 } }
pub fn context_wash() -> ColorU   { ColorU { r: 255, g: 255, b: 255, a: 6 }  }
/// Destructive menu-item hover: error() at ~15% alpha.
pub fn danger_wash() -> ColorU {
    let e = crate::theme::current().error;
    ColorU { r: e.r, g: e.g, b: e.b, a: 38 }
}
pub fn menu_shadow() -> ColorU    { ColorU { r: 0, g: 0, b: 0, a: 128 } }

// Panel dimensions — not colours, never change with themes.
pub const TOPBAR_H: f32 = 36.0;
pub const STATUS_H: f32 = 26.0;
pub const HEADER_H: f32 = 24.0;
pub const TAB_H: f32    = 26.0;
pub const LEFT_W: f32   = 240.0;
pub const RIGHT_W: f32  = 300.0;

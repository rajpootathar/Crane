//! Crane DEFAULT (dark) theme tokens, ported verbatim from
//! `src/theme.rs::Theme::dark()` as warpui ColorU. In the real migration
//! these come from the reused theme.rs data via an `Rgb::to_warp()` shim;
//! here they are inlined so the shell prototype matches Crane exactly.

use warpui::color::ColorU;

const fn c(r: u8, g: u8, b: u8) -> ColorU {
    ColorU { r, g, b, a: 255 }
}

pub const BG: ColorU = c(14, 16, 24);
pub const SIDEBAR_BG: ColorU = c(18, 20, 28);
pub const TOPBAR_BG: ColorU = c(20, 22, 32);
pub const SURFACE: ColorU = c(40, 45, 60);
pub const BORDER: ColorU = c(60, 66, 86);
pub const DIVIDER: ColorU = c(36, 40, 52);
pub const TEXT: ColorU = c(212, 216, 228);
pub const TEXT_HOVER: ColorU = c(234, 238, 248);
pub const TEXT_MUTED: ColorU = c(150, 156, 172);
pub const TEXT_HEADER: ColorU = c(140, 146, 162);
pub const ACCENT: ColorU = c(90, 135, 220);
pub const ROW_ACTIVE: ColorU = c(48, 56, 80);
pub const ROW_HOVER: ColorU = c(30, 34, 46);
pub const FOCUS_BORDER: ColorU = c(100, 140, 220);
pub const ERROR: ColorU = c(220, 95, 95);
pub const SUCCESS: ColorU = c(90, 190, 140);
pub const WARNING: ColorU = c(232, 170, 70);

// Panel dimensions (from src/ui/top.rs, src/ui/status.rs, src/state/state.rs).
pub const TOPBAR_H: f32 = 34.0;
pub const STATUS_H: f32 = 28.0;
pub const LEFT_W: f32 = 240.0;
pub const RIGHT_W: f32 = 300.0;

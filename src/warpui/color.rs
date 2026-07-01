//! crane_term::Color -> warpui ColorU mapping. The 256-color palette and
//! the `color_to_egui` match arms are ported verbatim from Crane's
//! `src/terminal/view.rs`. The default fg/bg here mirror Crane's DEFAULT
//! theme (`theme.rs` terminal_fg/terminal_bg); a full port would thread the
//! active theme through instead of hardcoding the default.

use crane_term::{Color as TermColor, NamedColor};
use warpui::color::ColorU;

/// The 256-color palette (16 ANSI + 6×6×6 cube + grayscale ramp).
/// Identical to `view.rs::palette`.
pub fn palette(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => (0x1a, 0x1c, 0x28),
        1 => (0xcc, 0x55, 0x55),
        2 => (0x44, 0xaa, 0x99),
        3 => (0xe8, 0x92, 0x2a),
        4 => (0x5a, 0x7a, 0xbf),
        5 => (0xaa, 0x66, 0xcc),
        6 => (0x55, 0xaa, 0xaa),
        7 => (0xb0, 0xb4, 0xc0),
        8 => (0x4a, 0x4c, 0x5a),
        9 => (0xff, 0x66, 0x66),
        10 => (0x55, 0xcc, 0xbb),
        11 => (0xff, 0xaa, 0x44),
        12 => (0x77, 0x99, 0xdd),
        13 => (0xcc, 0x77, 0xdd),
        14 => (0x77, 0xcc, 0xcc),
        15 => (0xdd, 0xdd, 0xee),
        16..=231 => {
            let i = idx - 16;
            let r = (i / 36) * 51;
            let g = ((i % 36) / 6) * 51;
            let b = (i % 6) * 51;
            (r, g, b)
        }
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            (gray, gray, gray)
        }
    }
}

#[inline]
fn rgb(t: (u8, u8, u8)) -> ColorU {
    ColorU::new(t.0, t.1, t.2, 255)
}

#[inline]
fn theme_rgb(c: crate::theme::Rgb) -> ColorU {
    ColorU::new(c.r, c.g, c.b, 255)
}

/// Default terminal foreground — the active theme's `terminal_fg`.
pub fn default_fg() -> ColorU {
    theme_rgb(crate::theme::current().terminal_fg)
}

/// Default terminal background — the active theme's `terminal_bg`.
pub fn default_bg() -> ColorU {
    theme_rgb(crate::theme::current().terminal_bg)
}

/// Cursor color — the active theme's accent.
pub fn cursor_color() -> ColorU {
    theme_rgb(crate::theme::current().accent)
}

/// Port of `view.rs::color_to_egui`.
pub fn term_color_to_coloru(color: TermColor, is_fg: bool) -> ColorU {
    match color {
        TermColor::Rgb { r, g, b } => ColorU::new(r, g, b, 255),
        TermColor::Indexed(idx) => rgb(palette(idx)),
        TermColor::Named(named) => match named {
            NamedColor::Foreground => default_fg(),
            NamedColor::Background => default_bg(),
            NamedColor::Cursor => default_fg(),
            other => {
                let idx = other as u16;
                if idx < 16 {
                    rgb(palette(idx as u8))
                } else if is_fg {
                    default_fg()
                } else {
                    default_bg()
                }
            }
        },
    }
}

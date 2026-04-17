//! Theme system — runtime colours read from a global `Theme`.
//!
//! - Two built-in themes (`dark`, `light`) ship with Crane.
//! - User themes live at `~/.config/crane/themes/*.toml` and are loaded on
//!   startup. The active theme name is persisted in the session file.
//! - Every accessor clones the current theme from a parking_lot RwLock;
//!   reads are cheap (essentially an atomic pointer load in release).

use egui::Color32;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
    pub fn to_color32(self) -> Color32 {
        Color32::from_rgb(self.r, self.g, self.b)
    }
}

/// A named colour palette covering every surface Crane paints itself.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Theme {
    pub name: String,
    pub bg: Rgb,
    pub sidebar_bg: Rgb,
    pub topbar_bg: Rgb,
    pub surface: Rgb,
    pub surface_alt: Rgb,
    pub surface_hi: Rgb,
    pub border: Rgb,
    pub border_strong: Rgb,
    pub divider: Rgb,
    pub text: Rgb,
    pub text_hover: Rgb,
    pub text_muted: Rgb,
    pub text_header: Rgb,
    pub accent: Rgb,
    pub row_hover: Rgb,
    pub row_active: Rgb,
    pub focus_border: Rgb,
    pub inactive_border: Rgb,
    pub error: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub terminal_bg: Rgb,
    pub terminal_fg: Rgb,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "crane-dark".into(),
            bg: Rgb::new(14, 16, 24),
            sidebar_bg: Rgb::new(18, 20, 28),
            topbar_bg: Rgb::new(20, 22, 32),
            surface: Rgb::new(40, 45, 60),
            surface_alt: Rgb::new(56, 62, 82),
            surface_hi: Rgb::new(72, 80, 104),
            border: Rgb::new(60, 66, 86),
            border_strong: Rgb::new(96, 106, 132),
            divider: Rgb::new(36, 40, 52),
            text: Rgb::new(212, 216, 228),
            text_hover: Rgb::new(234, 238, 248),
            text_muted: Rgb::new(150, 156, 172),
            text_header: Rgb::new(140, 146, 162),
            accent: Rgb::new(90, 135, 220),
            row_hover: Rgb::new(30, 34, 46),
            row_active: Rgb::new(48, 56, 80),
            focus_border: Rgb::new(100, 140, 220),
            inactive_border: Rgb::new(36, 40, 52),
            error: Rgb::new(220, 110, 110),
            success: Rgb::new(120, 210, 140),
            warning: Rgb::new(220, 180, 110),
            terminal_bg: Rgb::new(14, 16, 24),
            terminal_fg: Rgb::new(176, 180, 192),
        }
    }

    pub fn light() -> Self {
        Self {
            name: "crane-light".into(),
            bg: Rgb::new(244, 246, 250),
            sidebar_bg: Rgb::new(234, 236, 242),
            topbar_bg: Rgb::new(228, 230, 238),
            surface: Rgb::new(218, 222, 232),
            surface_alt: Rgb::new(200, 206, 220),
            surface_hi: Rgb::new(180, 188, 206),
            border: Rgb::new(196, 202, 216),
            border_strong: Rgb::new(150, 160, 180),
            divider: Rgb::new(210, 214, 224),
            text: Rgb::new(28, 32, 44),
            text_hover: Rgb::new(10, 14, 22),
            text_muted: Rgb::new(100, 108, 128),
            text_header: Rgb::new(88, 96, 116),
            accent: Rgb::new(30, 100, 200),
            row_hover: Rgb::new(220, 226, 238),
            row_active: Rgb::new(196, 214, 240),
            focus_border: Rgb::new(40, 120, 220),
            inactive_border: Rgb::new(196, 202, 216),
            error: Rgb::new(190, 60, 60),
            success: Rgb::new(40, 150, 80),
            warning: Rgb::new(200, 140, 40),
            terminal_bg: Rgb::new(248, 249, 252),
            terminal_fg: Rgb::new(36, 40, 52),
        }
    }

    pub fn darcula() -> Self {
        // IntelliJ Darcula: editor #2b2b2b, sidebar #3c3f41, panels #313335.
        Self {
            name: "darcula".into(),
            bg: Rgb::new(43, 43, 43),
            sidebar_bg: Rgb::new(49, 51, 53),
            topbar_bg: Rgb::new(60, 63, 65),
            surface: Rgb::new(69, 73, 74),
            surface_alt: Rgb::new(82, 88, 90),
            surface_hi: Rgb::new(96, 106, 109),
            border: Rgb::new(41, 43, 45),
            border_strong: Rgb::new(100, 106, 112),
            divider: Rgb::new(41, 43, 45),
            text: Rgb::new(187, 187, 187),
            text_hover: Rgb::new(225, 225, 225),
            text_muted: Rgb::new(128, 128, 128),
            text_header: Rgb::new(150, 150, 150),
            accent: Rgb::new(87, 151, 206),
            row_hover: Rgb::new(60, 63, 65),
            row_active: Rgb::new(75, 110, 175),
            focus_border: Rgb::new(87, 151, 206),
            inactive_border: Rgb::new(41, 43, 45),
            error: Rgb::new(204, 102, 102),
            success: Rgb::new(120, 180, 90),
            warning: Rgb::new(220, 170, 90),
            terminal_bg: Rgb::new(43, 43, 43),
            terminal_fg: Rgb::new(187, 187, 187),
        }
    }

    pub fn github_dark() -> Self {
        Self {
            name: "github-dark".into(),
            bg: Rgb::new(13, 17, 23),
            sidebar_bg: Rgb::new(22, 27, 34),
            topbar_bg: Rgb::new(22, 27, 34),
            surface: Rgb::new(33, 38, 45),
            surface_alt: Rgb::new(48, 54, 61),
            surface_hi: Rgb::new(68, 76, 86),
            border: Rgb::new(48, 54, 61),
            border_strong: Rgb::new(88, 96, 105),
            divider: Rgb::new(33, 38, 45),
            text: Rgb::new(201, 209, 217),
            text_hover: Rgb::new(240, 246, 252),
            text_muted: Rgb::new(139, 148, 158),
            text_header: Rgb::new(139, 148, 158),
            accent: Rgb::new(88, 166, 255),
            row_hover: Rgb::new(33, 38, 45),
            row_active: Rgb::new(38, 52, 80),
            focus_border: Rgb::new(88, 166, 255),
            inactive_border: Rgb::new(48, 54, 61),
            error: Rgb::new(248, 81, 73),
            success: Rgb::new(63, 185, 80),
            warning: Rgb::new(210, 153, 34),
            terminal_bg: Rgb::new(13, 17, 23),
            terminal_fg: Rgb::new(201, 209, 217),
        }
    }

    pub fn vscode_dark() -> Self {
        Self {
            name: "vscode-dark".into(),
            bg: Rgb::new(30, 30, 30),
            sidebar_bg: Rgb::new(37, 37, 38),
            topbar_bg: Rgb::new(60, 60, 60),
            surface: Rgb::new(45, 45, 45),
            surface_alt: Rgb::new(60, 60, 60),
            surface_hi: Rgb::new(80, 80, 80),
            border: Rgb::new(60, 60, 60),
            border_strong: Rgb::new(95, 95, 95),
            divider: Rgb::new(27, 27, 28),
            text: Rgb::new(212, 212, 212),
            text_hover: Rgb::new(255, 255, 255),
            text_muted: Rgb::new(170, 170, 170),
            text_header: Rgb::new(150, 150, 150),
            accent: Rgb::new(0, 122, 204),
            row_hover: Rgb::new(42, 45, 46),
            row_active: Rgb::new(9, 71, 113),
            focus_border: Rgb::new(0, 122, 204),
            inactive_border: Rgb::new(60, 60, 60),
            error: Rgb::new(244, 71, 71),
            success: Rgb::new(137, 209, 133),
            warning: Rgb::new(220, 180, 90),
            terminal_bg: Rgb::new(30, 30, 30),
            terminal_fg: Rgb::new(212, 212, 212),
        }
    }

    pub fn vscode_light() -> Self {
        // VS Code Light+ defaults. Softer off-white bg to reduce glare,
        // activity-bar/title-bar in the familiar darker neutral, clear
        // contrast for the sidebar vs editor.
        Self {
            name: "vscode-light".into(),
            bg: Rgb::new(255, 255, 255),
            sidebar_bg: Rgb::new(243, 243, 243),
            topbar_bg: Rgb::new(233, 233, 233),
            surface: Rgb::new(224, 224, 224),
            surface_alt: Rgb::new(206, 206, 206),
            surface_hi: Rgb::new(184, 184, 184),
            border: Rgb::new(204, 204, 204),
            border_strong: Rgb::new(160, 160, 160),
            divider: Rgb::new(214, 214, 214),
            text: Rgb::new(37, 37, 37),
            text_hover: Rgb::new(0, 0, 0),
            text_muted: Rgb::new(97, 97, 97),
            text_header: Rgb::new(106, 115, 125),
            accent: Rgb::new(0, 120, 212),
            row_hover: Rgb::new(228, 230, 241),
            row_active: Rgb::new(200, 219, 248),
            focus_border: Rgb::new(0, 120, 212),
            inactive_border: Rgb::new(212, 212, 212),
            error: Rgb::new(204, 32, 24),
            success: Rgb::new(16, 124, 16),
            warning: Rgb::new(191, 138, 0),
            terminal_bg: Rgb::new(255, 255, 255),
            terminal_fg: Rgb::new(37, 37, 37),
        }
    }

    pub fn one_dark() -> Self {
        Self {
            name: "one-dark".into(),
            bg: Rgb::new(40, 44, 52),
            sidebar_bg: Rgb::new(33, 37, 43),
            topbar_bg: Rgb::new(33, 37, 43),
            surface: Rgb::new(60, 64, 73),
            surface_alt: Rgb::new(72, 77, 89),
            surface_hi: Rgb::new(92, 100, 115),
            border: Rgb::new(49, 53, 60),
            border_strong: Rgb::new(90, 96, 110),
            divider: Rgb::new(33, 37, 43),
            text: Rgb::new(171, 178, 191),
            text_hover: Rgb::new(224, 230, 240),
            text_muted: Rgb::new(127, 132, 142),
            text_header: Rgb::new(127, 132, 142),
            accent: Rgb::new(97, 175, 239),
            row_hover: Rgb::new(48, 54, 63),
            row_active: Rgb::new(58, 79, 111),
            focus_border: Rgb::new(97, 175, 239),
            inactive_border: Rgb::new(49, 53, 60),
            error: Rgb::new(224, 108, 117),
            success: Rgb::new(152, 195, 121),
            warning: Rgb::new(229, 192, 123),
            terminal_bg: Rgb::new(40, 44, 52),
            terminal_fg: Rgb::new(171, 178, 191),
        }
    }

    pub fn builtins() -> Vec<Theme> {
        vec![
            Self::dark(),
            Self::light(),
            Self::darcula(),
            Self::github_dark(),
            Self::vscode_dark(),
            Self::vscode_light(),
            Self::one_dark(),
        ]
    }
}

static CURRENT: RwLock<Option<Theme>> = RwLock::new(None);

pub fn init(theme: Theme) {
    *CURRENT.write() = Some(theme);
}

pub fn set(theme: Theme) {
    *CURRENT.write() = Some(theme);
}

pub fn current() -> Theme {
    CURRENT
        .read()
        .clone()
        .unwrap_or_else(Theme::dark)
}

pub fn themes_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.config/crane/themes"))
}

/// Scan `~/.config/crane/themes/*.toml` and return all successfully parsed
/// themes. Built-in themes are returned first, then user themes.
pub fn load_all() -> Vec<Theme> {
    let mut out = Theme::builtins();
    let dir = themes_dir();
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(theme) = load_from_path(&path)
            {
                out.push(theme);
            }
        }
    }
    out
}

pub fn load_from_path(path: &Path) -> Option<Theme> {
    let bytes = std::fs::read_to_string(path).ok()?;
    toml::from_str(&bytes).ok()
}

pub fn find_by_name(name: &str) -> Option<Theme> {
    load_all().into_iter().find(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_unique_names() {
        let names: Vec<_> = Theme::builtins().iter().map(|t| t.name.clone()).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len());
    }

    #[test]
    fn rgb_round_trip_toml() {
        let dark = Theme::dark();
        let s = toml::to_string(&dark).unwrap();
        let back: Theme = toml::from_str(&s).unwrap();
        assert_eq!(dark.name, back.name);
        assert_eq!(dark.bg.r, back.bg.r);
    }

    #[test]
    fn current_falls_back_to_dark_if_uninitialised() {
        // Reset
        *CURRENT.write() = None;
        assert_eq!(current().name, "crane-dark");
    }
}

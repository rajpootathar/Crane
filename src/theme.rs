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

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default)]
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
    /// Text selection / highlight background. Applied to selected
    /// cells in the Terminal pane and (future) selected ranges in the
    /// File pane. Custom themes set their own value in the `.toml`
    /// under `selection = [r, g, b]`. Missing in older theme files →
    /// falls back to the theme accent at ~28% opacity.
    #[serde(default)]
    pub selection: Rgb,
    /// Name of the syntect theme to use for code highlighting. Matches a
    /// `two_face::theme::EmbeddedThemeName` display name (e.g. "OneHalfDark",
    /// "VisualStudioDarkPlus", "Dracula") or a syntect default. Falls back to
    /// a bg-brightness heuristic in file_view if absent/unresolved.
    #[serde(default)]
    pub syntax_theme: String,
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
            selection: Rgb::new(50, 78, 128),
            syntax_theme: "OneHalfDark".into(),
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
            selection: Rgb::new(186, 210, 246),
            syntax_theme: "InspiredGithub".into(),
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
            selection: Rgb::new(33, 66, 131),
            syntax_theme: "Dracula".into(),
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
            selection: Rgb::new(33, 60, 103),
            syntax_theme: "TwoDark".into(),
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
            selection: Rgb::new(38, 79, 120),
            syntax_theme: "VisualStudioDarkPlus".into(),
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
            selection: Rgb::new(173, 214, 255),
            syntax_theme: "OneHalfLight".into(),
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
            selection: Rgb::new(62, 68, 81),
            syntax_theme: "TwoDark".into(),
        }
    }

    pub fn tokyo_night() -> Self {
        // Tokyo Night Storm — palette by enkia, the most-installed
        // variant. Cool blues with violet/cyan accents.
        Self {
            name: "tokyo-night".into(),
            bg: Rgb::new(26, 27, 38),
            sidebar_bg: Rgb::new(22, 22, 30),
            topbar_bg: Rgb::new(22, 22, 30),
            surface: Rgb::new(36, 40, 59),
            surface_alt: Rgb::new(52, 59, 88),
            surface_hi: Rgb::new(72, 81, 113),
            border: Rgb::new(33, 35, 53),
            border_strong: Rgb::new(86, 95, 137),
            divider: Rgb::new(22, 22, 30),
            text: Rgb::new(192, 202, 245),
            text_hover: Rgb::new(216, 222, 233),
            text_muted: Rgb::new(118, 124, 156),
            text_header: Rgb::new(122, 162, 247),
            accent: Rgb::new(122, 162, 247),
            row_hover: Rgb::new(33, 35, 53),
            row_active: Rgb::new(54, 74, 130),
            focus_border: Rgb::new(122, 162, 247),
            inactive_border: Rgb::new(33, 35, 53),
            error: Rgb::new(247, 118, 142),
            success: Rgb::new(158, 206, 106),
            warning: Rgb::new(224, 175, 104),
            terminal_bg: Rgb::new(26, 27, 38),
            terminal_fg: Rgb::new(192, 202, 245),
            selection: Rgb::new(54, 74, 130),
            syntax_theme: "TwoDark".into(),
        }
    }

    pub fn dracula() -> Self {
        // Dracula — official palette per draculatheme.com.
        Self {
            name: "dracula".into(),
            bg: Rgb::new(40, 42, 54),
            sidebar_bg: Rgb::new(33, 34, 44),
            topbar_bg: Rgb::new(33, 34, 44),
            surface: Rgb::new(68, 71, 90),
            surface_alt: Rgb::new(86, 89, 112),
            surface_hi: Rgb::new(108, 112, 138),
            border: Rgb::new(56, 58, 75),
            border_strong: Rgb::new(108, 112, 138),
            divider: Rgb::new(33, 34, 44),
            text: Rgb::new(248, 248, 242),
            text_hover: Rgb::new(255, 255, 255),
            text_muted: Rgb::new(150, 152, 168),
            text_header: Rgb::new(189, 147, 249),
            accent: Rgb::new(189, 147, 249),
            row_hover: Rgb::new(50, 53, 67),
            row_active: Rgb::new(68, 71, 90),
            focus_border: Rgb::new(255, 121, 198),
            inactive_border: Rgb::new(56, 58, 75),
            error: Rgb::new(255, 85, 85),
            success: Rgb::new(80, 250, 123),
            warning: Rgb::new(241, 250, 140),
            terminal_bg: Rgb::new(40, 42, 54),
            terminal_fg: Rgb::new(248, 248, 242),
            selection: Rgb::new(68, 71, 90),
            syntax_theme: "Dracula".into(),
        }
    }

    pub fn catppuccin_mocha() -> Self {
        // Catppuccin Mocha — official palette from
        // catppuccin.com/palette. Pastel-on-dark.
        Self {
            name: "catppuccin-mocha".into(),
            bg: Rgb::new(30, 30, 46),
            sidebar_bg: Rgb::new(24, 24, 37),
            topbar_bg: Rgb::new(24, 24, 37),
            surface: Rgb::new(49, 50, 68),
            surface_alt: Rgb::new(69, 71, 90),
            surface_hi: Rgb::new(88, 91, 112),
            border: Rgb::new(49, 50, 68),
            border_strong: Rgb::new(88, 91, 112),
            divider: Rgb::new(24, 24, 37),
            text: Rgb::new(205, 214, 244),
            text_hover: Rgb::new(220, 224, 248),
            text_muted: Rgb::new(147, 153, 178),
            text_header: Rgb::new(180, 190, 254),
            accent: Rgb::new(137, 180, 250),
            row_hover: Rgb::new(49, 50, 68),
            row_active: Rgb::new(69, 71, 90),
            focus_border: Rgb::new(137, 180, 250),
            inactive_border: Rgb::new(49, 50, 68),
            error: Rgb::new(243, 139, 168),
            success: Rgb::new(166, 227, 161),
            warning: Rgb::new(249, 226, 175),
            terminal_bg: Rgb::new(30, 30, 46),
            terminal_fg: Rgb::new(205, 214, 244),
            selection: Rgb::new(69, 71, 90),
            syntax_theme: "OneHalfDark".into(),
        }
    }

    pub fn gruvbox_dark() -> Self {
        // Gruvbox Dark Hard — palette by morhetz. Warm retro browns/
        // yellows easy on the eyes for long sessions.
        Self {
            name: "gruvbox-dark".into(),
            bg: Rgb::new(40, 40, 40),
            sidebar_bg: Rgb::new(29, 32, 33),
            topbar_bg: Rgb::new(29, 32, 33),
            surface: Rgb::new(60, 56, 54),
            surface_alt: Rgb::new(80, 73, 69),
            surface_hi: Rgb::new(102, 92, 84),
            border: Rgb::new(60, 56, 54),
            border_strong: Rgb::new(124, 111, 100),
            divider: Rgb::new(50, 48, 47),
            text: Rgb::new(235, 219, 178),
            text_hover: Rgb::new(251, 241, 199),
            text_muted: Rgb::new(168, 153, 132),
            text_header: Rgb::new(189, 174, 147),
            accent: Rgb::new(250, 189, 47),
            row_hover: Rgb::new(50, 48, 47),
            row_active: Rgb::new(80, 73, 69),
            focus_border: Rgb::new(250, 189, 47),
            inactive_border: Rgb::new(60, 56, 54),
            error: Rgb::new(251, 73, 52),
            success: Rgb::new(184, 187, 38),
            warning: Rgb::new(250, 189, 47),
            terminal_bg: Rgb::new(40, 40, 40),
            terminal_fg: Rgb::new(235, 219, 178),
            selection: Rgb::new(80, 73, 69),
            syntax_theme: "TwoDark".into(),
        }
    }

    pub fn nord() -> Self {
        // Nord — palette from nordtheme.com. Cool arctic blues.
        Self {
            name: "nord".into(),
            bg: Rgb::new(46, 52, 64),    // nord0
            sidebar_bg: Rgb::new(36, 41, 51),
            topbar_bg: Rgb::new(36, 41, 51),
            surface: Rgb::new(59, 66, 82), // nord1
            surface_alt: Rgb::new(67, 76, 94), // nord2
            surface_hi: Rgb::new(76, 86, 106), // nord3
            border: Rgb::new(59, 66, 82),
            border_strong: Rgb::new(76, 86, 106),
            divider: Rgb::new(36, 41, 51),
            text: Rgb::new(216, 222, 233), // nord4
            text_hover: Rgb::new(229, 233, 240), // nord5
            text_muted: Rgb::new(143, 152, 168),
            text_header: Rgb::new(136, 192, 208), // nord8
            accent: Rgb::new(136, 192, 208), // nord8 frost
            row_hover: Rgb::new(59, 66, 82),
            row_active: Rgb::new(67, 76, 94),
            focus_border: Rgb::new(136, 192, 208),
            inactive_border: Rgb::new(59, 66, 82),
            error: Rgb::new(191, 97, 106),    // nord11
            success: Rgb::new(163, 190, 140), // nord14
            warning: Rgb::new(235, 203, 139), // nord13
            terminal_bg: Rgb::new(46, 52, 64),
            terminal_fg: Rgb::new(216, 222, 233),
            selection: Rgb::new(67, 76, 94),
            syntax_theme: "Nord".into(),
        }
    }

    pub fn solarized_dark() -> Self {
        // Solarized Dark — Ethan Schoonover's palette.
        Self {
            name: "solarized-dark".into(),
            bg: Rgb::new(0, 43, 54),       // base03
            sidebar_bg: Rgb::new(7, 54, 66), // base02
            topbar_bg: Rgb::new(7, 54, 66),
            surface: Rgb::new(7, 54, 66),
            surface_alt: Rgb::new(20, 70, 84),
            surface_hi: Rgb::new(40, 90, 104),
            border: Rgb::new(7, 54, 66),
            border_strong: Rgb::new(88, 110, 117), // base01
            divider: Rgb::new(7, 54, 66),
            text: Rgb::new(131, 148, 150),  // base0
            text_hover: Rgb::new(147, 161, 161), // base1
            text_muted: Rgb::new(101, 123, 131), // base00
            text_header: Rgb::new(108, 113, 196), // violet
            accent: Rgb::new(38, 139, 210), // blue
            row_hover: Rgb::new(7, 54, 66),
            row_active: Rgb::new(20, 70, 84),
            focus_border: Rgb::new(38, 139, 210),
            inactive_border: Rgb::new(7, 54, 66),
            error: Rgb::new(220, 50, 47),   // red
            success: Rgb::new(133, 153, 0), // green
            warning: Rgb::new(181, 137, 0), // yellow
            terminal_bg: Rgb::new(0, 43, 54),
            terminal_fg: Rgb::new(131, 148, 150),
            selection: Rgb::new(20, 70, 84),
            syntax_theme: "Solarized (dark)".into(),
        }
    }

    pub fn solarized_light() -> Self {
        Self {
            name: "solarized-light".into(),
            bg: Rgb::new(253, 246, 227),   // base3
            sidebar_bg: Rgb::new(238, 232, 213), // base2
            topbar_bg: Rgb::new(238, 232, 213),
            surface: Rgb::new(238, 232, 213),
            surface_alt: Rgb::new(225, 218, 197),
            surface_hi: Rgb::new(208, 200, 178),
            border: Rgb::new(225, 218, 197),
            border_strong: Rgb::new(147, 161, 161), // base1
            divider: Rgb::new(238, 232, 213),
            text: Rgb::new(101, 123, 131), // base00
            text_hover: Rgb::new(88, 110, 117), // base01
            text_muted: Rgb::new(131, 148, 150), // base0
            text_header: Rgb::new(108, 113, 196),
            accent: Rgb::new(38, 139, 210),
            row_hover: Rgb::new(238, 232, 213),
            row_active: Rgb::new(225, 218, 197),
            focus_border: Rgb::new(38, 139, 210),
            inactive_border: Rgb::new(225, 218, 197),
            error: Rgb::new(220, 50, 47),
            success: Rgb::new(133, 153, 0),
            warning: Rgb::new(181, 137, 0),
            terminal_bg: Rgb::new(253, 246, 227),
            terminal_fg: Rgb::new(101, 123, 131),
            selection: Rgb::new(225, 218, 197),
            syntax_theme: "Solarized (light)".into(),
        }
    }

    pub fn monokai() -> Self {
        // Monokai — Wimer Hazenberg's classic Sublime Text palette.
        Self {
            name: "monokai".into(),
            bg: Rgb::new(39, 40, 34),
            sidebar_bg: Rgb::new(29, 30, 25),
            topbar_bg: Rgb::new(29, 30, 25),
            surface: Rgb::new(49, 50, 44),
            surface_alt: Rgb::new(73, 72, 62),
            surface_hi: Rgb::new(102, 100, 88),
            border: Rgb::new(49, 50, 44),
            border_strong: Rgb::new(117, 113, 94),
            divider: Rgb::new(29, 30, 25),
            text: Rgb::new(248, 248, 242),
            text_hover: Rgb::new(255, 255, 255),
            text_muted: Rgb::new(117, 113, 94),
            text_header: Rgb::new(166, 226, 46),
            accent: Rgb::new(102, 217, 239),
            row_hover: Rgb::new(49, 50, 44),
            row_active: Rgb::new(73, 72, 62),
            focus_border: Rgb::new(166, 226, 46),
            inactive_border: Rgb::new(49, 50, 44),
            error: Rgb::new(249, 38, 114),
            success: Rgb::new(166, 226, 46),
            warning: Rgb::new(230, 219, 116),
            terminal_bg: Rgb::new(39, 40, 34),
            terminal_fg: Rgb::new(248, 248, 242),
            selection: Rgb::new(73, 72, 62),
            syntax_theme: "Monokai Extended".into(),
        }
    }

    pub fn high_contrast_dark() -> Self {
        // WCAG AAA contrast: pure white text on pure black bg, with
        // high-saturation accent colours. For low-vision users and
        // bright-room daytime use.
        Self {
            name: "high-contrast-dark".into(),
            bg: Rgb::new(0, 0, 0),
            sidebar_bg: Rgb::new(0, 0, 0),
            topbar_bg: Rgb::new(15, 15, 15),
            surface: Rgb::new(20, 20, 20),
            surface_alt: Rgb::new(40, 40, 40),
            surface_hi: Rgb::new(70, 70, 70),
            border: Rgb::new(120, 120, 120),
            border_strong: Rgb::new(255, 255, 255),
            divider: Rgb::new(80, 80, 80),
            text: Rgb::new(255, 255, 255),
            text_hover: Rgb::new(255, 255, 255),
            text_muted: Rgb::new(200, 200, 200),
            text_header: Rgb::new(255, 255, 0),
            accent: Rgb::new(0, 170, 255),
            row_hover: Rgb::new(40, 40, 40),
            row_active: Rgb::new(0, 80, 140),
            focus_border: Rgb::new(255, 255, 0),
            inactive_border: Rgb::new(120, 120, 120),
            error: Rgb::new(255, 80, 80),
            success: Rgb::new(80, 255, 80),
            warning: Rgb::new(255, 220, 80),
            terminal_bg: Rgb::new(0, 0, 0),
            terminal_fg: Rgb::new(255, 255, 255),
            selection: Rgb::new(0, 100, 180),
            syntax_theme: "TwoDark".into(),
        }
    }

    pub fn high_contrast_light() -> Self {
        Self {
            name: "high-contrast-light".into(),
            bg: Rgb::new(255, 255, 255),
            sidebar_bg: Rgb::new(255, 255, 255),
            topbar_bg: Rgb::new(245, 245, 245),
            surface: Rgb::new(240, 240, 240),
            surface_alt: Rgb::new(220, 220, 220),
            surface_hi: Rgb::new(190, 190, 190),
            border: Rgb::new(120, 120, 120),
            border_strong: Rgb::new(0, 0, 0),
            divider: Rgb::new(180, 180, 180),
            text: Rgb::new(0, 0, 0),
            text_hover: Rgb::new(0, 0, 0),
            text_muted: Rgb::new(60, 60, 60),
            text_header: Rgb::new(0, 0, 200),
            accent: Rgb::new(0, 80, 200),
            row_hover: Rgb::new(220, 220, 220),
            row_active: Rgb::new(180, 210, 250),
            focus_border: Rgb::new(0, 0, 200),
            inactive_border: Rgb::new(120, 120, 120),
            error: Rgb::new(180, 0, 0),
            success: Rgb::new(0, 130, 0),
            warning: Rgb::new(180, 110, 0),
            terminal_bg: Rgb::new(255, 255, 255),
            terminal_fg: Rgb::new(0, 0, 0),
            selection: Rgb::new(180, 210, 250),
            syntax_theme: "InspiredGithub".into(),
        }
    }

    pub fn builtins() -> Vec<Theme> {
        vec![
            // Crane defaults.
            Self::dark(),
            Self::light(),
            // Tier 1 — universally recognized dark themes.
            Self::tokyo_night(),
            Self::dracula(),
            Self::catppuccin_mocha(),
            Self::gruvbox_dark(),
            Self::nord(),
            // Tier 2 — classics + editor brand themes.
            Self::one_dark(),
            Self::solarized_dark(),
            Self::solarized_light(),
            Self::monokai(),
            Self::darcula(),
            Self::github_dark(),
            Self::vscode_dark(),
            Self::vscode_light(),
            // Accessibility — WCAG-conformant high contrast pairs.
            Self::high_contrast_dark(),
            Self::high_contrast_light(),
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
    PathBuf::from(format!("{home}/.crane/themes"))
}

/// Scan `~/.config/crane/themes/*.toml` and return all successfully parsed
/// themes. Built-in themes are returned first, then user themes.
pub fn load_all() -> Vec<Theme> {
    let mut out: Vec<Theme> = Vec::new();
    let dir = themes_dir();
    let mut disk_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(theme) = load_from_path(&path)
            {
                disk_names.insert(theme.name.clone());
                out.push(theme);
            }
        }
    }
    // Add any built-in not shadowed by an on-disk theme of the same name.
    for theme in Theme::builtins() {
        if !disk_names.contains(&theme.name) {
            out.push(theme);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn load_from_path(path: &Path) -> Option<Theme> {
    let bytes = std::fs::read_to_string(path).ok()?;
    toml::from_str(&bytes).ok()
}

pub fn find_by_name(name: &str) -> Option<Theme> {
    load_all().into_iter().find(|t| t.name == name)
}

/// On first launch, write every built-in theme out to
/// `~/.config/crane/themes/<name>.toml` so users can see a working
/// template + tweak any colour. Existing files are never overwritten.
pub fn ensure_builtin_tomls_on_disk() {
    let dir = themes_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for theme in Theme::builtins() {
        let path = dir.join(format!("{}.toml", theme.name));
        if path.exists() {
            continue;
        }
        if let Ok(contents) = toml::to_string_pretty(&theme) {
            let header = format!(
                "# Crane theme: {}\n# \
                 Edit any Rgb value below and Crane will pick up the file\n# \
                 the next time you open the theme picker.\n\n",
                theme.name
            );
            let _ = std::fs::write(&path, format!("{header}{contents}"));
        }
    }
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

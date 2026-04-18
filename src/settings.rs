//! User-level preferences — things that follow the user across projects
//! and should not be tied to a specific session's open files.
//!
//! On-disk at `~/.crane/settings.json`. Old installs with these keys
//! living inside session.json are migrated on first read.

use crate::lsp::LanguageConfigs;
use crate::state::{App, RightTab};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_theme_name")]
    pub selected_theme: String,
    #[serde(default)]
    pub syntax_theme_override: Option<String>,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default)]
    pub custom_mono_font: Option<String>,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    #[serde(default = "default_left_w")]
    pub left_panel_w: f32,
    #[serde(default = "default_right_w")]
    pub right_panel_w: f32,
    #[serde(default)]
    pub editor_word_wrap: bool,
    #[serde(default)]
    pub editor_trim_on_save: bool,
    #[serde(default = "t")]
    pub show_left: bool,
    #[serde(default = "t")]
    pub show_right: bool,
    #[serde(default)]
    pub right_tab_files: bool, // serialized as bool to stay stable across enum changes
    #[serde(default)]
    pub language_configs: LanguageConfigs,
}

fn default_theme_name() -> String {
    "crane-dark".into()
}
fn default_font_size() -> f32 {
    14.0
}
fn default_ui_scale() -> f32 {
    1.0
}
fn default_left_w() -> f32 {
    240.0
}
fn default_right_w() -> f32 {
    300.0
}
fn t() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            selected_theme: default_theme_name(),
            syntax_theme_override: None,
            font_size: default_font_size(),
            custom_mono_font: None,
            ui_scale: default_ui_scale(),
            left_panel_w: default_left_w(),
            right_panel_w: default_right_w(),
            editor_word_wrap: false,
            editor_trim_on_save: false,
            show_left: true,
            show_right: true,
            right_tab_files: false,
            language_configs: LanguageConfigs::default(),
        }
    }
}

pub fn settings_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.crane/settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(settings_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = settings_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Snapshot the user-preference slice of App state.
    pub fn from_app(app: &App) -> Self {
        Self {
            selected_theme: app.selected_theme.clone(),
            syntax_theme_override: app.syntax_theme_override.clone(),
            font_size: app.font_size,
            custom_mono_font: app.custom_mono_font.clone(),
            ui_scale: app.ui_scale,
            left_panel_w: app.left_panel_w,
            right_panel_w: app.right_panel_w,
            editor_word_wrap: app.editor_word_wrap,
            editor_trim_on_save: app.editor_trim_on_save,
            show_left: app.show_left,
            show_right: app.show_right,
            right_tab_files: matches!(app.right_tab, RightTab::Files),
            language_configs: app.language_configs.clone(),
        }
    }

    /// Apply settings to an App — call after `App::new` at startup.
    pub fn apply(self, app: &mut App) {
        app.selected_theme = self.selected_theme;
        app.syntax_theme_override = self.syntax_theme_override;
        app.font_size = self.font_size.clamp(9.0, 28.0);
        app.custom_mono_font = self.custom_mono_font;
        app.ui_scale = self.ui_scale.clamp(0.75, 1.5);
        app.left_panel_w = self.left_panel_w.clamp(180.0, 600.0);
        app.right_panel_w = self.right_panel_w.clamp(200.0, 700.0);
        app.editor_word_wrap = self.editor_word_wrap;
        app.editor_trim_on_save = self.editor_trim_on_save;
        app.show_left = self.show_left;
        app.show_right = self.show_right;
        app.right_tab = if self.right_tab_files {
            RightTab::Files
        } else {
            RightTab::Changes
        };
        app.language_configs = self.language_configs;
    }
}

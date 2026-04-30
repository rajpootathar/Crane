//! One-off startup helpers: PATH rehydration for GUI launches, old
//! config directory migration, the Dock / window icon, and font
//! loading. Extracted from `main.rs` to keep the entry point focused
//! on App wiring.

use crate::theme;

/// Login-shells aren't sourced by Finder / Dock when launching a GUI
/// app, so PATH ends up stripped down to the system defaults. Heuristic:
/// if none of the common user-ish PATH entries are present but HOME is
/// set, we're probably GUI-launched — spawn `$SHELL -l -c "echo $PATH"`
/// and import it. Login mode (`-l`) is deliberate: `-i` would source
/// `.zshrc` / `.bashrc`, which triggers nvm / brew shellenv / banners
/// and can add seconds of startup time.
///
/// Unix-only: Windows GUI apps inherit PATH correctly from the
/// environment, so this function is a no-op there.
#[cfg(unix)]
pub fn fix_path_for_gui_launch() {
    let original = std::env::var("PATH").unwrap_or_default();
    let looks_gui = !original.contains("/usr/local/bin")
        && !original.contains("/opt/homebrew/bin")
        && !original.contains(".cargo/bin")
        && std::env::var("HOME").is_ok();

    // Start from whatever PATH we have. If we detect a GUI launch,
    // replace with the login-shell PATH — but don't stop there: login
    // shells don't source `.zshrc` / `.bashrc`, so version-manager
    // installs (nvm, asdf, fnm, volta) are invisible. We always append
    // the sprinkle list below so npm / node / etc. installed via those
    // managers still get resolved.
    let mut current = original.clone();
    if looks_gui {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let output = std::process::Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("echo __CRANE_PATH__:$PATH")
            .output();
        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().find(|l| l.starts_with("__CRANE_PATH__:")) {
                let path = line.trim_start_matches("__CRANE_PATH__:").to_string();
                if !path.is_empty() {
                    current = path;
                }
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let mut extras: Vec<String> = vec![
        format!("{home}/.cargo/bin"),
        format!("{home}/.local/bin"),
        format!("{home}/bin"),
        format!("{home}/go/bin"),
        format!("{home}/.volta/bin"),
        format!("{home}/.fnm/aliases/default/bin"),
        format!("{home}/.asdf/shims"),
        format!("{home}/.bun/bin"),
        format!("{home}/n/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    // nvm installs one directory per Node version; glob and include
    // all of them so whatever the user has active is found. Order by
    // mtime so the most-recently-used version wins the PATH race.
    let nvm_dir = std::path::PathBuf::from(format!("{home}/.nvm/versions/node"));
    if let Ok(rd) = std::fs::read_dir(&nvm_dir) {
        let mut versions: Vec<(std::time::SystemTime, String)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path().join("bin");
                if !p.is_dir() {
                    return None;
                }
                let mtime = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                Some((mtime, p.to_string_lossy().into_owned()))
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in versions {
            extras.push(p);
        }
    }

    // De-dup while preserving order (extras come first so they take
    // precedence for binaries that also exist in /usr/bin).
    let mut seen = std::collections::HashSet::new();
    let mut parts: Vec<String> = Vec::new();
    for p in extras.into_iter().chain(current.split(':').map(|s| s.to_string())) {
        if p.is_empty() || !seen.insert(p.clone()) {
            continue;
        }
        parts.push(p);
    }
    // SAFETY: called from main() before any threads spawn.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

/// No-op on non-Unix platforms (Windows inherits PATH correctly).
#[cfg(not(unix))]
pub fn fix_path_for_gui_launch() {}

/// Earlier builds stored config under `~/.config/crane`; we moved to
/// `~/.crane` so Crane's files sit alongside other dev tools the user
/// typically keeps at the home root. One-shot rename at startup.
pub fn migrate_config_dir() {
    let home = match crate::util::home_dir() {
        Some(h) => h,
        None => return,
    };
    let old_dir = home.join(".config").join("crane");
    let new_dir = home.join(".crane");
    if old_dir.is_dir() && !new_dir.exists() {
        let _ = std::fs::rename(&old_dir, &new_dir);
    }
}

pub fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../crane.png");
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

/// JetBrains Mono Regular — bundled (~264 KB). OFL 1.1 licensed.
/// Primary Monospace font for aesthetics.
const JETBRAINS_MONO_TTF: &[u8] =
    include_bytes!("../assets/JetBrainsMono-Regular.ttf");

/// Cascadia Mono Regular — bundled (~562 KB). OFL 1.1 licensed.
/// Registered as a fallback AFTER JetBrains Mono so egui's per-glyph
/// lookup falls through to it for codepoints JBM lacks. Crucially,
/// JBM has no Braille patterns (U+2800–U+28FF), which breaks sparkline
/// rendering in TUI apps like nvitop / btop. Cascadia Mono covers
/// Braille, block elements, shade, and box-drawing — filling the gap
/// without changing the default look of ASCII code / UI text.
const CASCADIA_MONO_TTF: &[u8] =
    include_bytes!("../assets/CascadiaMono-Regular.ttf");

pub fn load_fonts(ctx: &egui::Context, custom_mono: Option<&str>) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    fonts.font_data.insert(
        "jetbrains_mono".to_string(),
        std::sync::Arc::new(egui::FontData::from_static(JETBRAINS_MONO_TTF)),
    );
    fonts.font_data.insert(
        "cascadia_mono".to_string(),
        std::sync::Arc::new(egui::FontData::from_static(CASCADIA_MONO_TTF)),
    );
    if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        mono.insert(0, "jetbrains_mono".to_string());
        mono.insert(1, "cascadia_mono".to_string());
    }
    // Proportional family also gets Cascadia as a fallback so labels
    // that happen to contain Braille / block glyphs don't render tofu.
    if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        prop.push("cascadia_mono".to_string());
    }

    if let Some(path) = custom_mono
        && let Ok(bytes) = std::fs::read(path)
    {
        let name = "user_mono".to_string();
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            mono.insert(0, name);
        }
    }

    ctx.set_fonts(fonts);
}

pub fn apply_style(ctx: &egui::Context) {
    let t = theme::current();
    let light = t.bg.r as u32 + t.bg.g as u32 + t.bg.b as u32 > 128 * 3;
    ctx.set_visuals(if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    });

    let mut style = (*ctx.global_style()).clone();
    let surface_1 = t.surface.to_color32();
    let surface_2 = t.surface_alt.to_color32();
    let surface_3 = t.surface_hi.to_color32();
    let border_subtle = t.border.to_color32();
    let border_strong = t.border_strong.to_color32();
    let text_primary = t.text.to_color32();
    let text_hover = t.text_hover.to_color32();
    let accent = t.accent.to_color32();

    let corner = egui::CornerRadius::same(6);
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = corner;
    }

    style.visuals.widgets.inactive.weak_bg_fill = surface_1;
    style.visuals.widgets.inactive.bg_fill = surface_1;
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, border_subtle);
    style.visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, text_primary);

    style.visuals.widgets.hovered.weak_bg_fill = surface_2;
    style.visuals.widgets.hovered.bg_fill = surface_2;
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, border_strong);
    style.visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, text_hover);

    style.visuals.widgets.active.weak_bg_fill = surface_3;
    style.visuals.widgets.active.bg_fill = surface_3;
    style.visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, border_strong);
    style.visuals.widgets.active.fg_stroke =
        egui::Stroke::new(1.0, text_hover);

    style.visuals.selection.bg_fill =
        egui::Color32::from_rgba_unmultiplied(t.accent.r, t.accent.g, t.accent.b, 70);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    style.visuals.window_corner_radius = egui::CornerRadius::same(10);
    style.visuals.window_fill = t.surface.to_color32();
    style.visuals.window_stroke = egui::Stroke::new(1.0, border_subtle);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(8);

    // TextEdit / ScrollArea / inline code all key off these. Without
    // them the Files pane editor ignored the theme.
    style.visuals.panel_fill = t.bg.to_color32();
    style.visuals.extreme_bg_color = t.bg.to_color32();
    style.visuals.code_bg_color = t.surface.to_color32();
    style.visuals.faint_bg_color = t.row_hover.to_color32();
    style.visuals.override_text_color = Some(text_primary);

    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.menu_margin = egui::Margin::symmetric(6, 6);

    // egui exposes debug paint flags only in debug builds. Zero them
    // explicitly so a stray debug flag doesn't bleed into a dev build.
    #[cfg(debug_assertions)]
    {
        style.debug = egui::style::DebugOptions::default();
        style.debug.debug_on_hover = false;
        style.debug.debug_on_hover_with_all_modifiers = false;
        style.debug.show_expand_width = false;
        style.debug.show_expand_height = false;
        style.debug.show_resize = false;
        style.debug.show_interactive_widgets = false;
        style.debug.show_widget_hits = false;
    }

    ctx.set_global_style(style);
}

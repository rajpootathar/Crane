//! Landing page rendered inside a `PaneContent::Welcome` slot. Three
//! primary actions (Terminal / Browser / Files) and a keyboard shortcut
//! cheat-sheet so the user can either click a button or learn the
//! chord once and stop reading this screen.
//!
//! The view is stateless — it just returns a `WelcomeAction` the caller
//! translates into a `PaneAction` that main.rs applies (terminal spawn
//! needs `ctx`, the right panel toggle lives on `App`, so neither can
//! happen in-place here).

use crate::theme;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};
use egui_phosphor::regular as icons;

/// What the user clicked on the landing page. `None` means nothing
/// was clicked this frame — dispatch is lifted out of the render
/// function because the handlers need the parent `Context` / `App`.
pub enum WelcomeAction {
    OpenTerminal,
    OpenBrowser,
    ToggleFilesPanel,
}

pub fn render(ui: &mut egui::Ui) -> Option<WelcomeAction> {
    let mut action: Option<WelcomeAction> = None;
    let rect = ui.available_rect_before_wrap();

    // Paint a flat background in the pane surface color. Without this,
    // the welcome area inherits whatever the outer layout chose — on
    // some themes that's a near-transparent tint that doesn't read.
    ui.painter()
        .rect_filled(rect, 0.0, theme::current().surface.to_color32());

    // Center the whole stack vertically + horizontally by measuring
    // the content first, then drawing into a child UI anchored at the
    // computed top-left. Using egui's layout inheritance tends to push
    // things to the top-left, so we do the math explicitly.
    const LOGO_H: f32 = 84.0;
    const LOGO_W: f32 = 82.0; // crane.png is 800×820, keep that aspect
    const GAP_LOGO: f32 = 16.0;
    const TITLE_H: f32 = 44.0;
    const SUBTITLE_H: f32 = 22.0;
    const BUTTONS_H: f32 = 96.0;
    const SHORTCUTS_H: f32 = 180.0;
    const GAP_TITLE: f32 = 6.0;
    const GAP_BLOCK: f32 = 28.0;
    let total_h = LOGO_H
        + GAP_LOGO
        + TITLE_H
        + GAP_TITLE
        + SUBTITLE_H
        + GAP_BLOCK
        + BUTTONS_H
        + GAP_BLOCK
        + SHORTCUTS_H;
    // Sit a touch above geometric center — the shortcut cheat-sheet is
    // informationally dense so the visual weight already leans low.
    let top_y = rect.min.y + ((rect.height() - total_h) * 0.42).max(20.0);
    let content_w = rect.width().min(620.0);
    let left_x = rect.min.x + (rect.width() - content_w) * 0.5;

    // Logo — centered horizontally above the wordmark.
    let logo_rect = Rect::from_min_size(
        Pos2::new(rect.center().x - LOGO_W * 0.5, top_y),
        Vec2::new(LOGO_W, LOGO_H),
    );
    if let Some(tex) = crane_logo(ui.ctx()) {
        ui.painter().image(
            tex.id(),
            logo_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    let title_rect = Rect::from_min_size(
        Pos2::new(left_x, logo_rect.max.y + GAP_LOGO),
        Vec2::new(content_w, TITLE_H),
    );
    ui.painter().text(
        title_rect.center(),
        egui::Align2::CENTER_CENTER,
        "Crane",
        FontId::new(34.0, egui::FontFamily::Proportional),
        theme::current().text.to_color32(),
    );

    let subtitle_rect = Rect::from_min_size(
        Pos2::new(left_x, title_rect.max.y + GAP_TITLE),
        Vec2::new(content_w, SUBTITLE_H),
    );
    ui.painter().text(
        subtitle_rect.center(),
        egui::Align2::CENTER_CENTER,
        "Pick a surface to begin, or use a shortcut below.",
        FontId::new(13.0, egui::FontFamily::Proportional),
        theme::current().text_muted.to_color32(),
    );

    // Three big buttons in a row.
    let buttons_top = subtitle_rect.max.y + GAP_BLOCK;
    let btn_w = 170.0;
    let btn_h = BUTTONS_H;
    let btn_gap = 16.0;
    let row_w = btn_w * 3.0 + btn_gap * 2.0;
    let row_x = rect.min.x + (rect.width() - row_w) * 0.5;
    let buttons = [
        (
            icons::TERMINAL_WINDOW,
            "Terminal",
            "Spawn a shell in this pane",
            "⌘ T splits",
            WelcomeAction::OpenTerminal,
        ),
        (
            icons::CUBE,
            "Browser",
            "Embedded WebKit tab",
            "⌥ ⌘ T new tab",
            WelcomeAction::OpenBrowser,
        ),
        (
            icons::FOLDER_OPEN,
            "Files",
            "Show the workspace tree",
            "⌘ /  toggles",
            WelcomeAction::ToggleFilesPanel,
        ),
    ];
    for (i, (glyph, label, hint, chord, act)) in buttons.into_iter().enumerate() {
        let x = row_x + (btn_w + btn_gap) * i as f32;
        let brect = Rect::from_min_size(Pos2::new(x, buttons_top), Vec2::new(btn_w, btn_h));
        if welcome_button(ui, brect, glyph, label, hint, chord) {
            action = Some(act);
        }
    }

    // Shortcut cheat-sheet — two columns of chord / description pairs.
    let sc_top = buttons_top + BUTTONS_H + GAP_BLOCK;
    let sc_rect = Rect::from_min_size(
        Pos2::new(left_x, sc_top),
        Vec2::new(content_w, SHORTCUTS_H),
    );
    draw_shortcuts(ui, sc_rect);

    action
}

fn welcome_button(
    ui: &mut egui::Ui,
    rect: Rect,
    glyph: &str,
    label: &str,
    hint: &str,
    chord: &str,
) -> bool {
    let id = egui::Id::new(("welcome_btn", label));
    let resp = ui.interact(rect, id, egui::Sense::click());
    let hovered = resp.hovered();
    let bg = if hovered {
        theme::current().row_hover.to_color32()
    } else {
        theme::current().topbar_bg.to_color32()
    };
    let border = if hovered {
        theme::current().accent.to_color32()
    } else {
        theme::current().inactive_border.to_color32()
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    // Glyph
    painter.text(
        Pos2::new(rect.center().x, rect.min.y + 26.0),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::new(24.0, egui::FontFamily::Proportional),
        theme::current().accent.to_color32(),
    );
    // Label
    painter.text(
        Pos2::new(rect.center().x, rect.min.y + 56.0),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::new(14.0, egui::FontFamily::Proportional),
        theme::current().text.to_color32(),
    );
    // Hint
    painter.text(
        Pos2::new(rect.center().x, rect.min.y + 74.0),
        egui::Align2::CENTER_CENTER,
        hint,
        FontId::new(11.0, egui::FontFamily::Proportional),
        theme::current().text_muted.to_color32(),
    );
    // Chord
    painter.text(
        Pos2::new(rect.center().x, rect.max.y - 14.0),
        egui::Align2::CENTER_CENTER,
        chord,
        FontId::new(10.5, egui::FontFamily::Monospace),
        theme::current().text_muted.to_color32(),
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn draw_shortcuts(ui: &mut egui::Ui, rect: Rect) {
    let header = Color32::from_rgb(140, 146, 162);
    ui.painter().text(
        Pos2::new(rect.min.x, rect.min.y),
        egui::Align2::LEFT_TOP,
        "SHORTCUTS",
        FontId::new(10.5, egui::FontFamily::Proportional),
        header,
    );
    let items: [(&str, &str); 10] = [
        ("⌘ T", "Split pane — new terminal"),
        ("⌘ ⇧ T", "New tab in workspace"),
        ("⌘ D / ⌘ ⇧ D", "Split horizontally / vertically"),
        ("⌥ ⌘ T", "New tab in focused Browser"),
        ("⌘ W / ⌘ ⇧ W", "Close pane / tab"),
        ("⌘ [ / ⌘ ]", "Focus previous / next pane"),
        ("⌘ B / ⌘ /", "Toggle Left / Right Panel"),
        ("⌘ = / ⌘ -", "Font size up / down"),
        ("⌘ 0", "Reset font size"),
        ("⌘ ,", "Open Settings"),
    ];
    let y0 = rect.min.y + 22.0;
    let col_w = rect.width() * 0.5;
    let row_h = 18.0;
    let chord_w = 110.0;
    for (i, (chord, desc)) in items.iter().enumerate() {
        let col = i % 2;
        let row = i / 2;
        let x = rect.min.x + col_w * col as f32;
        let y = y0 + row as f32 * row_h;
        ui.painter().text(
            Pos2::new(x, y),
            egui::Align2::LEFT_TOP,
            *chord,
            FontId::new(11.5, egui::FontFamily::Monospace),
            theme::current().accent.to_color32(),
        );
        ui.painter().text(
            Pos2::new(x + chord_w, y),
            egui::Align2::LEFT_TOP,
            *desc,
            FontId::new(11.5, egui::FontFamily::Proportional),
            theme::current().text_muted.to_color32(),
        );
    }

}

/// Decode `crane.png` once and cache the GPU texture in the egui data
/// store. Returns `None` only if decoding fails — in practice that can't
/// happen because the PNG is `include_bytes!`-compiled in.
fn crane_logo(ctx: &egui::Context) -> Option<TextureHandle> {
    let key = egui::Id::new("crane_welcome_logo");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(tex);
    }
    let bytes = include_bytes!("../../crane.png");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    let handle = ctx.load_texture("crane_welcome_logo", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, handle.clone()));
    Some(handle)
}

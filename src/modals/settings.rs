use crate::state::{App, SettingsSection};
use crate::theme;
use egui::{Color32, RichText};

const SIDEBAR_W: f32 = 200.0;
const WIN_W: f32 = 960.0;
const WIN_H: f32 = 640.0;

pub enum SettingsEffect {
    None,
    ReloadFonts,
}

pub fn render(
    ctx: &egui::Context,
    app: &mut App,
    apply_style: impl FnOnce(&egui::Context),
) -> SettingsEffect {
    if !app.show_settings {
        return SettingsEffect::None;
    }
    let mut theme_change: Option<String> = None;
    let mut effect = SettingsEffect::None;

    // Paint a dimmed click-absorbing backdrop so clicks outside the
    // dialog don't fall through to panes underneath. egui::Modal
    // handles Esc + background-click dismiss + focus ordering.
    let mut close_clicked = false;
    let modal_resp = egui::Modal::new(egui::Id::new("settings_modal")).show(ctx, |ui| {
        ui.set_width(WIN_W);
        ui.set_height(WIN_H);
            ui.horizontal(|ui| {
                ui.heading("Settings");
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(egui_phosphor::regular::X).size(13.0),
                                )
                                .frame(false)
                                .min_size(egui::vec2(22.0, 22.0)),
                            )
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close_clicked = true;
                        }
                    },
                );
            });
            ui.separator();
            let content_h = ui.available_height();
            ui.horizontal_top(|ui| {
                render_sidebar(ui, app, content_h);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_min_height(content_h);
                    ui.set_max_height(content_h);
                    egui::ScrollArea::vertical()
                        .id_salt(("settings_section", app.settings_section as u32))
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            match app.settings_section {
                                SettingsSection::Appearance => {
                                    if render_appearance(ui, app, &mut theme_change) {
                                        effect = SettingsEffect::ReloadFonts;
                                    }
                                }
                                SettingsSection::Editor => render_editor(ui, app),
                                SettingsSection::Terminal => render_terminal(ui),
                                SettingsSection::LanguageServers => crate::modals::settings_lsp::render(ui, app),
                                SettingsSection::Shortcuts => render_shortcuts(ui),
                                SettingsSection::About => render_about(ui, app),
                            }
                        });
                });
            });
    });

    // Click on the dimmed backdrop, or Esc, closes the modal.
    if modal_resp.should_close() || close_clicked {
        app.show_settings = false;
    }
    if let Some(name) = theme_change
        && let Some(t) = theme::find_by_name(&name)
    {
        theme::set(t);
        app.selected_theme = name;
        apply_style(ctx);
        ctx.request_repaint();
    }
    effect
}

fn render_sidebar(ui: &mut egui::Ui, app: &mut App, content_h: f32) {
    ui.vertical(|ui| {
        ui.set_width(SIDEBAR_W);
        ui.set_min_height(content_h);
        ui.add_space(4.0);
        for section in SettingsSection::ALL {
            let selected = app.settings_section == *section;
            let bg = if selected {
                let a = theme::current().accent;
                Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)
            } else {
                Color32::TRANSPARENT
            };
            let fg = if selected {
                theme::current().text.to_color32()
            } else {
                theme::current().text_muted.to_color32()
            };
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(SIDEBAR_W - 6.0, 32.0),
                egui::Sense::click(),
            );
            let hovered = resp.hovered();
            let fill = if hovered && !selected {
                theme::current().row_hover.to_color32()
            } else {
                bg
            };
            if fill != Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, 6.0, fill);
            }
            ui.painter().text(
                egui::pos2(rect.min.x + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                section.icon(),
                egui::FontId::new(15.0, egui::FontFamily::Proportional),
                fg,
            );
            ui.painter().text(
                egui::pos2(rect.min.x + 36.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                section.label(),
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
                fg,
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                app.settings_section = *section;
            }
            ui.add_space(2.0);
        }
    });
}

fn render_appearance(
    ui: &mut egui::Ui,
    app: &mut App,
    theme_change: &mut Option<String>,
) -> bool {
    section_title(ui, "Appearance");
    let mut reload_fonts = false;
    ui.add_space(6.0);

    // --- Fonts section ---
    setting_row(ui, "Editor / Terminal font size", |ui| {
        ui.scope(|ui| {
            let v = ui.visuals_mut();
            v.widgets.inactive.bg_fill = theme::current().surface_alt.to_color32();
            v.widgets.hovered.bg_fill = theme::current().surface_hi.to_color32();
            v.widgets.active.bg_fill = theme::current().accent.to_color32();
            ui.add(
                egui::Slider::new(&mut app.font_size, 9.0..=28.0)
                    .step_by(1.0)
                    .trailing_fill(true),
            );
        });
        if ui.small_button("Reset").clicked() {
            app.font_size = 14.0;
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("Tip: Cmd +  /  Cmd −  /  Cmd 0 also resize the editor & terminal.")
                .size(10.5)
                .italics()
                .color(theme::current().text_muted.to_color32()),
        );
    });
    ui.add_space(4.0);
    setting_row(ui, "Monospace font", |ui| {
        let name = app
            .custom_mono_font
            .as_deref()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p)
                    .to_string()
            })
            .unwrap_or_else(|| "Default".to_string());
        ui.label(
            RichText::new(name)
                .size(12.0)
                .color(theme::current().text_muted.to_color32()),
        );
        if ui.small_button("Choose…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose monospace .ttf / .otf")
                .add_filter("Font", &["ttf", "otf", "TTF", "OTF"])
                .pick_file()
        {
            app.custom_mono_font = Some(path.to_string_lossy().to_string());
            reload_fonts = true;
        }
        if app.custom_mono_font.is_some() && ui.small_button("Reset").clicked() {
            app.custom_mono_font = None;
            reload_fonts = true;
        }
    });

    ui.add_space(12.0);
    ui.label(
        RichText::new("Theme")
            .size(12.5)
            .color(theme::current().text.to_color32())
            .strong(),
    );
    ui.add_space(4.0);
    let footer_h = 62.0;
    let avail = ui.available_height() - footer_h;
    egui::ScrollArea::vertical()
        .id_salt("settings_themes")
        .max_height(avail.max(120.0))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for t in theme::load_all() {
                let is_active = app.selected_theme == t.name;
                let fill = if is_active {
                    let a = theme::current().accent;
                    Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)
                } else {
                    theme::current().surface.to_color32()
                };
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 44.0),
                    egui::Sense::click(),
                );
                let hover_fill = if resp.hovered() && !is_active {
                    theme::current().row_hover.to_color32()
                } else {
                    fill
                };
                ui.painter().rect_filled(rect, 6.0, hover_fill);
                let swatch_size = 12.0;
                for (i, col) in [t.bg, t.surface, t.accent, t.text, t.selection].iter().enumerate() {
                    let c = col.to_color32();
                    let sw = egui::Rect::from_min_size(
                        egui::pos2(
                            rect.min.x + 12.0 + (swatch_size + 4.0) * i as f32,
                            rect.center().y - swatch_size / 2.0,
                        ),
                        egui::vec2(swatch_size, swatch_size),
                    );
                    ui.painter().rect_filled(sw, 2.0, c);
                }
                let name_x = rect.min.x + 12.0 + (swatch_size + 4.0) * 5.0 + 8.0;
                ui.painter().text(
                    egui::pos2(name_x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &t.name,
                    egui::FontId::new(13.0, egui::FontFamily::Proportional),
                    theme::current().text.to_color32(),
                );
                if is_active {
                    ui.painter().text(
                        egui::pos2(rect.max.x - 14.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        egui_phosphor::regular::CHECK,
                        egui::FontId::new(14.0, egui::FontFamily::Proportional),
                        theme::current().accent.to_color32(),
                    );
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() && !is_active {
                    *theme_change = Some(t.name.clone());
                }
                ui.add_space(4.0);
            }
        });
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "Custom themes (.toml) live in {}",
            theme::themes_dir().display()
        ))
        .size(11.0)
        .color(theme::current().text_muted.to_color32()),
    );
    if ui.small_button("Open themes folder").clicked() {
        let dir = theme::themes_dir();
        let _ = std::fs::create_dir_all(&dir);
        super::open_in_file_manager(&dir);
    }

    ui.add_space(12.0);
    setting_row(ui, "Syntax highlighting", |ui| {
        let current_label = app
            .syntax_theme_override
            .clone()
            .unwrap_or_else(|| format!("Auto ({})", theme::current().syntax_theme));
        egui::ComboBox::from_id_salt("syntax_theme_override")
            .selected_text(current_label)
            .width(220.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(app.syntax_theme_override.is_none(), "Auto (pair with UI theme)")
                    .clicked()
                {
                    app.syntax_theme_override = None;
                }
                ui.separator();
                for name in crate::views::file_view::available_syntax_themes() {
                    let is_active = app
                        .syntax_theme_override
                        .as_deref()
                        .map(|s| s == name.as_str())
                        .unwrap_or(false);
                    if ui.selectable_label(is_active, &name).clicked() {
                        app.syntax_theme_override = Some(name);
                    }
                }
            });
    });
    reload_fonts
}

fn render_editor(ui: &mut egui::Ui, app: &mut App) {
    section_title(ui, "Editor");
    ui.add_space(10.0);
    ui.checkbox(&mut app.editor_word_wrap, "Word wrap");
    ui.add_space(2.0);
    ui.checkbox(
        &mut app.editor_trim_on_save,
        "Trim trailing whitespace on save",
    );
    ui.add_space(2.0);
    ui.checkbox(
        &mut app.single_click_open,
        "Single-click to open files (preview tab)",
    );
    ui.add_space(10.0);
    placeholder(
        ui,
        "More editor prefs (show whitespace, cursor style, tab width override) will land here.",
    );
}

fn setting_row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(theme::current().surface.to_color32())
        .stroke(egui::Stroke::new(
            1.0,
            theme::current().border.to_color32(),
        ))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new(label)
                        .size(12.5)
                        .color(theme::current().text.to_color32())
                        .strong(),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    content,
                );
            });
        });
}

fn render_terminal(ui: &mut egui::Ui) {
    section_title(ui, "Terminal");
    ui.add_space(8.0);
    placeholder(
        ui,
        "Shell override, font family, cursor style and scrollback size will land here.",
    );
}

fn render_shortcuts(ui: &mut egui::Ui) {
    section_title(ui, "Keyboard Shortcuts");
    ui.add_space(8.0);
    let rows: &[(&str, &str)] = &[
        ("Cmd+O", "Open file (external files open as read-only)"),
        ("Cmd+Shift+O", "Open folder as project workspace"),
        ("Cmd+T", "Split active Pane with new terminal"),
        ("Cmd+Shift+T", "New Tab in active Workspace"),
        ("Cmd+D", "Split Pane horizontally (side-by-side)"),
        ("Cmd+Shift+D", "Split Pane vertically (stacked)"),
        ("Cmd+W", "Close focused Pane"),
        ("Cmd+Shift+W", "Close active Tab"),
        ("Cmd+[ / Cmd+]", "Focus prev / next Pane"),
        ("Cmd+B", "Toggle Left Panel"),
        ("Cmd+/", "Toggle Right Panel"),
        ("Cmd+= / Cmd+-", "Increase / decrease font size"),
        ("Cmd+0", "Reset font size"),
        ("Cmd+S", "Save file (Files Pane)"),
        ("F2 / Cmd+R", "Rename active Tab"),
        ("Cmd+`", "Tab switcher — next (release Cmd to commit)"),
        ("Cmd+~", "Tab switcher — previous"),
        ("Cmd+K", "Terminal: clear screen + scrollback"),
        ("Shift+Tab", "Terminal: back-tab (CSI Z) for TUIs"),
        ("Ctrl+C / Ctrl+D", "Terminal: interrupt / EOF"),
    ];
    egui::ScrollArea::vertical()
        .id_salt("settings_shortcuts")
        .show(ui, |ui| {
            egui::Grid::new("shortcuts_grid")
                .num_columns(2)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for (key, desc) in rows {
                        ui.label(RichText::new(*key).monospace().strong());
                        ui.label(*desc);
                        ui.end_row();
                    }
                });
        });
}

fn render_about(ui: &mut egui::Ui, app: &mut App) {
    section_title(ui, "About");
    ui.add_space(12.0);
    ui.label(
        RichText::new("Crane")
            .size(22.0)
            .color(theme::current().text.to_color32())
            .strong(),
    );
    ui.label(
        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
            .size(12.0)
            .color(theme::current().text_muted.to_color32()),
    );
    ui.add_space(16.0);
    ui.label(
        RichText::new("Native GPU-rendered desktop development environment.")
            .size(12.5)
            .color(theme::current().text.to_color32()),
    );
    ui.add_space(8.0);
    ui.label(
        RichText::new("Built with Rust · egui · wgpu · vte · crane_term.")
            .size(11.5)
            .color(theme::current().text_muted.to_color32()),
    );
    ui.add_space(20.0);
    ui.horizontal(|ui| {
        if ui.button("GitHub").clicked() {
            let _ = webbrowser::open("https://github.com/rajpootathar/Crane");
        }
        if ui.button("Releases").clicked() {
            let _ = webbrowser::open("https://github.com/rajpootathar/Crane/releases");
        }
        if ui.button("Check for updates").clicked() {
            app.update_check.dismissed_this_session = None;
            app.update_check.available = None;
            app.update_check.manual_check = true;
            app.update_check.spawn_check(ui.ctx().clone());
        }
    });
    if let Some(u) = &app.update_check.available {
        ui.add_space(10.0);
        ui.label(
            RichText::new(format!("Update available: v{}", u.version))
                .color(theme::current().accent.to_color32())
                .strong(),
        );
    }

}


pub(crate) fn section_title(ui: &mut egui::Ui, label: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(label)
            .size(16.0)
            .color(theme::current().text.to_color32())
            .strong(),
    );
    ui.add_space(2.0);
    ui.separator();
}

fn placeholder(ui: &mut egui::Ui, msg: &str) {
    ui.label(
        RichText::new(msg)
            .size(12.0)
            .italics()
            .color(theme::current().text_muted.to_color32()),
    );
}

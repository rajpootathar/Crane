use crate::layout::FilesPane;
use crate::theme;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use egui::text::{LayoutJob, TextFormat};
use egui_phosphor::regular as icons;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(|| {
        let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
        // User-dropped `.sublime-syntax` / `.tmLanguage` packages in
        // ~/.crane/syntaxes/ get folded in so Babel/TSX/custom grammars
        // work without recompiling.
        if let Ok(home) = std::env::var("HOME") {
            let dir = std::path::PathBuf::from(format!("{home}/.crane/syntaxes"));
            if dir.is_dir() {
                let _ = builder.add_from_folder(&dir, true);
            }
        }
        builder.build()
    })
}
fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(ThemeSet::load_defaults)
}

/// Map file extension to a syntax name, with sensible fallbacks for flavours
/// (TSX→TypeScript, JSX→JavaScript, etc.) when a dedicated syntax isn't loaded.
fn find_syntax_for_ext(ext: &str) -> &'static syntect::parsing::SyntaxReference {
    let ss = syntaxes();
    if let Some(syn) = ss.find_syntax_by_extension(ext) {
        return syn;
    }
    let fallback = match ext {
        "tsx" | "mts" | "cts" => "TypeScript",
        "jsx" | "mjs" | "cjs" => "JavaScript",
        "vue" | "svelte" | "astro" => "HTML",
        "zsh" | "fish" | "bash" => "Bourne Again Shell (bash)",
        "h" => "C",
        "hpp" | "hh" | "hxx" | "cc" | "cxx" => "C++",
        _ => "Plain Text",
    };
    ss.find_syntax_by_name(fallback)
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

pub fn render(
    ui: &mut egui::Ui,
    pane_id: u64,
    pane: &mut FilesPane,
    font_size: f32,
    title: &mut String,
) {
    ui.push_id(("files_pane", pane_id), |ui| {
        render_inner(ui, pane, font_size, title);
    });
}

fn render_inner(ui: &mut egui::Ui, pane: &mut FilesPane, font_size: f32, title: &mut String) {
    if pane.tabs.is_empty() {
        let t = theme::current();
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                RichText::new("No files open")
                    .size(14.0)
                    .color(t.text.to_color32()),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Click a file in the Files sidebar to open it here")
                    .color(t.text_muted.to_color32())
                    .size(11.5),
            );
        });
        return;
    }

    // Tab bar
    let mut close_idx: Option<usize> = None;
    let mut activate_idx: Option<usize> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.add_space(4.0);
        for (idx, tab) in pane.tabs.iter().enumerate() {
            let is_active = idx == pane.active;
            let label = if tab.dirty() {
                format!("● {}", tab.name)
            } else {
                tab.name.clone()
            };
            let (clicked, close_clicked) = draw_file_tab(ui, &label, is_active, idx);
            if clicked {
                activate_idx = Some(idx);
            }
            if close_clicked {
                close_idx = Some(idx);
            }
        }
    });
    if let Some(idx) = activate_idx {
        pane.active = idx;
    }
    if let Some(idx) = close_idx {
        pane.close(idx);
        if pane.tabs.is_empty() {
            return;
        }
    }
    ui.add_space(2.0);

    let active_idx = pane.active.min(pane.tabs.len() - 1);
    pane.active = active_idx;

    // Save shortcut
    let save_pressed = ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::S)
    });

    {
        let tab = &mut pane.tabs[active_idx];
        let name_label = if tab.dirty() {
            format!("● {}", tab.name)
        } else {
            tab.name.clone()
        };
        *title = format!("Files · {name_label}");

        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new(&tab.path)
                    .size(10.5)
                    .color(theme::current().text_muted.to_color32()),
            );
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let save_btn = ui.add_enabled(
                        tab.dirty(),
                        egui::Button::new(
                            RichText::new(format!("{}  Save", icons::FLOPPY_DISK))
                                .size(11.5),
                        )
                        .min_size(egui::vec2(0.0, 24.0)),
                    );
                    if save_btn.clicked() || (save_pressed && tab.dirty()) {
                        if let Err(e) = std::fs::write(&tab.path, &tab.content) {
                            eprintln!("save failed: {e}");
                        } else {
                            tab.original_content = tab.content.clone();
                        }
                    }
                },
            );
        });
        ui.add_space(2.0);

        let font = FontId::new(font_size, FontFamily::Monospace);
        let ext = Path::new(&tab.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let syntax: &'static syntect::parsing::SyntaxReference = find_syntax_for_ext(ext);
        let bg = theme::current().bg;
        let is_light = bg.r as u32 + bg.g as u32 + bg.b as u32 > 128 * 3;
        let st_theme: &'static syntect::highlighting::Theme = if is_light {
            themes()
                .themes
                .get("InspiredGitHub")
                .unwrap_or_else(|| &themes().themes["base16-ocean.light"])
        } else {
            &themes().themes["base16-ocean.dark"]
        };
        let fallback_fg = theme::current().text.to_color32();

        let mut layouter = move |ui: &egui::Ui,
                                  buffer: &dyn egui::TextBuffer,
                                  _wrap_width: f32|
              -> std::sync::Arc<egui::Galley> {
            let text = buffer.as_str();
            let mut job = LayoutJob::default();
            let mut hl = HighlightLines::new(syntax, st_theme);
            for line in LinesWithEndings::from(text) {
                let segments: Vec<(Style, &str)> = hl
                    .highlight_line(line, syntaxes())
                    .unwrap_or_else(|_| vec![(Style::default(), line)]);
                for (style, segment) in segments {
                    let color = if style.foreground.r == 0
                        && style.foreground.g == 0
                        && style.foreground.b == 0
                        && style.foreground.a == 0
                    {
                        fallback_fg
                    } else {
                        Color32::from_rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        )
                    };
                    job.append(
                        segment,
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color,
                            ..Default::default()
                        },
                    );
                }
            }
            ui.fonts_mut(|f| f.layout_job(job))
        };

        ScrollArea::both()
            .id_salt(("file_scroll", active_idx))
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let editor = egui::TextEdit::multiline(&mut tab.content)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(30)
                    .layouter(&mut layouter);
                ui.add(editor);
            });
    }
}

fn draw_file_tab(
    ui: &mut egui::Ui,
    name: &str,
    is_active: bool,
    idx: usize,
) -> (bool, bool) {
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let close_font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let text_w = ui
        .fonts_mut(|f| f.layout_no_wrap(name.to_string(), font.clone(), egui::Color32::WHITE))
        .size()
        .x;
    let padding_x = 10.0;
    let gap = 6.0;
    let close_size = 16.0;
    let height = 24.0;
    let width = padding_x + text_w + gap + close_size + padding_x - 2.0;

    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.max.x - padding_x - close_size + 2.0,
            rect.min.y + (height - close_size) / 2.0,
        ),
        egui::vec2(close_size, close_size),
    );
    let close_response = ui.interact(
        close_rect,
        ui.id().with(("file_tab_close", idx)),
        egui::Sense::click(),
    );

    let t = theme::current();
    let accent_tint = {
        let a = t.accent;
        Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)
    };
    let (bg, fg) = if is_active {
        (accent_tint, t.text.to_color32())
    } else if response.hovered() || close_response.hovered() {
        (t.row_hover.to_color32(), t.text.to_color32())
    } else {
        (egui::Color32::TRANSPARENT, t.text_muted.to_color32())
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, bg);
    }
    ui.painter().text(
        egui::pos2(rect.min.x + padding_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        font,
        fg,
    );
    if close_response.hovered() {
        ui.painter().rect_filled(
            close_rect.shrink(1.0),
            4.0,
            theme::current().error.to_color32(),
        );
    }
    ui.painter().text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        icons::X,
        close_font,
        fg,
    );
    if response.hovered() || close_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (
        response.clicked() && !close_response.hovered(),
        close_response.clicked(),
    )
}

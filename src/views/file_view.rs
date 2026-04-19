use crate::state::layout::FilesPane;
use crate::lsp::Diagnostic;
use crate::theme;
use crate::views::diagnostics_overlay;
use crate::views::file_util::{
    char_idx_to_line_col, is_image_path, line_col_to_char, reveal_in_file_manager,
    short_path, trim_trailing_whitespace,
};
use crate::views::highlight::{rehighlight, LineHighlightCache};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use egui_phosphor::regular as icons;
use std::path::Path;
use std::sync::OnceLock;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

/// Cross-frame cache for a Files Pane tab's rendered galley. Keeps syntect
/// off the hot path — only runs when `key` changes (text / theme / font).
#[derive(Clone)]
struct CachedGalley {
    key: u64,
    galley: std::sync::Arc<egui::Galley>,
}



fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(|| {
        // two-face ships ~250 Sublime-grade syntaxes: TypeScript, TSX, JSX,
        // Dockerfile, Astro, Svelte, GraphQL, Prisma, Nix, Zig, etc. — a
        // big step up from syntect's bundled set for modern dev work.
        let mut builder = two_face::syntax::extra_newlines().into_builder();
        // User-dropped packages still fold in on top.
        if let Ok(home) = std::env::var("HOME") {
            let dir = std::path::PathBuf::from(format!("{home}/.crane/syntaxes"));
            if dir.is_dir() {
                let _ = builder.add_from_folder(&dir, true);
            }
        }
        builder.build()
    })
}
pub fn available_syntax_themes() -> Vec<String> {
    let mut names: Vec<String> = themes().themes.keys().cloned().collect();
    names.sort_unstable();
    let priority = [
        "VisualStudioDarkPlus",
        "OneHalfDark",
        "OneHalfLight",
        "TwoDark",
        "Dracula",
        "MonokaiExtended",
        "MonokaiExtendedBright",
        "MonokaiExtendedOrigin",
        "Nord",
        "SolarizedDark",
        "SolarizedLight",
        "GruvboxDark",
        "GruvboxLight",
        "Github",
        "InspiredGithub",
    ];
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for p in priority {
        if names.iter().any(|n| n == p) {
            out.push(p.to_string());
        }
    }
    for n in names {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        let extras = two_face::theme::extra();
        for name in two_face::theme::EmbeddedLazyThemeSet::theme_names() {
            let key = format!("{name:?}"); // enum Debug prints the variant name, e.g. "VisualStudioDarkPlus"
            set.themes.insert(key, extras.get(*name).clone());
        }
        set
    })
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

/// Editor-level preferences read from `App` / `Settings`.
#[derive(Clone, Copy)]
pub struct EditorPrefs {
    pub word_wrap: bool,
    pub trim_on_save: bool,
}

pub fn render(
    ui: &mut egui::Ui,
    pane_id: u64,
    pane: &mut FilesPane,
    font_size: f32,
    title: &mut String,
    syntax_theme_override: Option<&str>,
    diagnostics_for: &dyn Fn(&str) -> Vec<Diagnostic>,
    notify_saved: &dyn Fn(&str, &str),
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    goto_request: &dyn Fn(&str, u32, u32),
    workspace_root: Option<&std::path::Path>,
    prefs: EditorPrefs,
) {
    // Scope widget ids by pane so unrelated Files panes don't share
    // state (undo history, scroll positions, etc.) — the real work
    // lives entirely in this function; there's no separate inner.
    ui.push_id(("files_pane", pane_id), |ui| {
        render_scoped(
            ui,
            pane,
            font_size,
            title,
            syntax_theme_override,
            diagnostics_for,
            notify_saved,
            format_before_save,
            goto_request,
            workspace_root,
            prefs,
        );
    });
}

fn render_scoped(
    ui: &mut egui::Ui,
    pane: &mut FilesPane,
    font_size: f32,
    title: &mut String,
    syntax_theme_override: Option<&str>,
    diagnostics_for: &dyn Fn(&str) -> Vec<Diagnostic>,
    notify_saved: &dyn Fn(&str, &str),
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    goto_request: &dyn Fn(&str, u32, u32),
    workspace_root: Option<&std::path::Path>,
    prefs: EditorPrefs,
) {
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

    // Tab bar — horizontal scroll so many-open-file cases don't hide
    // tabs past the viewport edge.
    let mut close_idx: Option<usize> = None;
    let mut activate_idx: Option<usize> = None;
    ScrollArea::horizontal()
        .id_salt("file_tab_bar")
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(4.0);
                for (idx, tab) in pane.tabs.iter().enumerate() {
                    let is_active = idx == pane.active;
                    let label = if tab.dirty() {
                        format!("{}  {}", icons::CIRCLE, tab.name)
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
    // Cmd+F toggles the find bar for the active tab.
    let find_toggle = ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::F)
    });
    if find_toggle
        && !pane.tabs.is_empty()
    {
        let idx = pane.active.min(pane.tabs.len() - 1);
        let t = &mut pane.tabs[idx];
        t.find_query = match &t.find_query {
            Some(_) => None,
            None => Some(String::new()),
        };
    }

    {
        let tab = &mut pane.tabs[active_idx];
        poll_external_change(tab);
        let name_label = if tab.dirty() {
            format!("{}  {}", icons::CIRCLE, tab.name)
        } else {
            tab.name.clone()
        };
        *title = format!("Files · {name_label}");

        // External-change banner. Shown when another editor modified
        // the file on disk. Three actions: Reload (drops our edits),
        // Overwrite (force-save our buffer), Dismiss (just clear the
        // warning — user accepts the divergence).
        if tab.external_change {
            let t = theme::current();
            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(220, 100, 100, 28))
                .stroke(egui::Stroke::new(1.0, t.error.to_color32()))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}  This file changed on disk outside Crane.",
                                icons::WARNING
                            ))
                            .size(11.5)
                            .color(t.text.to_color32()),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.small_button("Dismiss").clicked() {
                                    tab.external_change = false;
                                    tab.disk_mtime = std::fs::metadata(&tab.path)
                                        .and_then(|m| m.modified())
                                        .ok();
                                }
                                if ui.small_button("Overwrite").clicked() {
                                    save_tab(
                                        tab,
                                        prefs,
                                        format_before_save,
                                        notify_saved,
                                        true,
                                    );
                                }
                                if ui.small_button("Reload").clicked() {
                                    reload_tab(tab);
                                }
                            },
                        );
                    });
                });
            ui.add_space(4.0);
        }

        let is_md = Path::new(&tab.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
            .unwrap_or(false);

        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let rel = short_path(&tab.path, workspace_root);
            ui.label(
                RichText::new(&rel)
                    .size(10.5)
                    .color(theme::current().text_muted.to_color32()),
            )
            .on_hover_text(&tab.path);
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
                        save_tab(tab, prefs, format_before_save, notify_saved, false);
                    }
                    if is_md {
                        let label = if tab.preview_mode {
                            format!("{}  Edit", icons::PENCIL_SIMPLE)
                        } else {
                            format!("{}  Preview", icons::EYE)
                        };
                        let btn = ui.add(
                            egui::Button::new(RichText::new(label).size(11.5))
                                .min_size(egui::vec2(0.0, 24.0)),
                        );
                        if btn.clicked() {
                            tab.preview_mode = !tab.preview_mode;
                        }
                    }
                },
            );
        });
        ui.add_space(2.0);

        // Find bar — rendered above the editor when open. Enter jumps to
        let FindBarOutcome { close: find_close, next: find_next, prev: find_prev } =
            render_find_bar(ui, tab);
        if find_close {
            tab.find_query = None;
        }
        if (find_next || find_prev)
            && let Some(q) = tab.find_query.clone()
            && !q.is_empty()
        {
            // Jump cursor to the next / prev occurrence of `q`.
            let te_id = ui
                .id()
                .with(("file_editor", &tab.path))
                .with("body");
            let cur = egui::TextEdit::load_state(ui.ctx(), te_id)
                .and_then(|s| s.cursor.char_range().map(|r| r.primary.index))
                .unwrap_or(0);
            let cur_byte =
                crate::format::char_idx_to_byte(&tab.content, cur);
            let target_byte = if find_next {
                tab.content[cur_byte..]
                    .find(&q)
                    .map(|p| cur_byte + p)
                    .or_else(|| tab.content.find(&q))
            } else {
                tab.content[..cur_byte]
                    .rfind(&q)
                    .or_else(|| tab.content.rfind(&q))
            };
            if let Some(byte) = target_byte {
                let chars_up_to = tab.content[..byte].chars().count();
                let (line, col) =
                    char_idx_to_line_col(&tab.content, chars_up_to);
                tab.pending_cursor = Some((line, col));
            }
        }

        let diagnostics: Vec<Diagnostic> = diagnostics_for(&tab.path);

        let font = FontId::new(font_size, FontFamily::Monospace);
        let ext = Path::new(&tab.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let syntax: &'static syntect::parsing::SyntaxReference = find_syntax_for_ext(ext);
        let bg = theme::current().bg;
        let is_light = bg.r as u32 + bg.g as u32 + bg.b as u32 > 128 * 3;
        let requested = syntax_theme_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| theme::current().syntax_theme.clone());
        let all = &themes().themes;
        let st_theme: &'static syntect::highlighting::Theme = all
            .get(&requested)
            .or_else(|| {
                if is_light {
                    all.get("InspiredGithub")
                        .or_else(|| all.get("InspiredGitHub"))
                        .or_else(|| all.get("base16-ocean.light"))
                } else {
                    all.get("OneHalfDark")
                        .or_else(|| all.get("base16-eighties.dark"))
                        .or_else(|| all.get("base16-ocean.dark"))
                }
            })
            .unwrap_or_else(|| all.values().next().expect("at least one theme"));
        let fallback_fg = theme::current().text.to_color32();

        // Salt the cache key on syntax-affecting inputs ONLY (text is
        // hashed inside the closure). Diagnostics now render as an overlay
        // pass after the galley, so LSP updates no longer invalidate the
        // cached highlight.
        // Include the current UI theme name in the cache key. Without
        // it, switching theme (e.g. crane-dark → crane-light) returned
        // a stale galley whose foreground colors and glyph indices
        // were baked against the previous theme + font atlas, showing
        // up as scrambled / gibberish text until the file was edited.
        let theme_name = theme::current().name.clone();
        let layout_salt = crate::util::hash64((font_size.to_bits(), &requested, &theme_name));
        let cache_path = tab.path.clone();
        let cache_id = egui::Id::new(("file_view_layouter", &cache_path));
        let line_cache_id = egui::Id::new(("file_view_lines", &cache_path));

        let mut layouter = move |ui: &egui::Ui,
                                  buffer: &dyn egui::TextBuffer,
                                  _wrap_width: f32|
              -> std::sync::Arc<egui::Galley> {
            let text = buffer.as_str();
            let key = crate::util::hash64((text, layout_salt));

            if let Some(cached) = ui
                .memory(|m| m.data.get_temp::<CachedGalley>(cache_id))
                && cached.key == key
            {
                return cached.galley;
            }

            // Incremental path: sync the per-line highlight cache with the
            // current buffer (runs syntect only on changed lines + all
            // lines below the first change), then rebuild the LayoutJob
            // from the cache. On a typical keystroke at the bottom of a
            // file this rehighlights exactly one line.
            let context_hash =
                crate::util::hash64((&requested, syntax.name.as_str(), &theme_name));
            let mut line_cache: LineHighlightCache = ui
                .memory(|m| m.data.get_temp::<LineHighlightCache>(line_cache_id))
                .unwrap_or_default();
            rehighlight(
                &mut line_cache,
                text,
                syntax,
                st_theme,
                syntaxes(),
                context_hash,
            );

            let mut job = LayoutJob::default();
            for entry in &line_cache.lines {
                for (style, piece) in &entry.segments {
                    let color = if style.foreground.a == 0 {
                        fallback_fg
                    } else {
                        Color32::from_rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        )
                    };
                    job.append(
                        piece,
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color,
                            ..Default::default()
                        },
                    );
                }
            }

            let galley = ui.fonts_mut(|f| f.layout_job(job));
            ui.memory_mut(|m| {
                m.data.insert_temp(
                    cache_id,
                    CachedGalley {
                        key,
                        galley: galley.clone(),
                    },
                );
                m.data.insert_temp(line_cache_id, line_cache);
            });
            galley
        };

        // Reserve a status strip at the bottom for diagnostics counts.
        let avail_h = ui.available_height();
        let status_h = 22.0;
        let editor_h = (avail_h - status_h).max(80.0);
        // split('\n') gives the correct visible-line count (including a
        // trailing empty line after a final newline) without double-counting.
        let line_count = tab.content.split('\n').count().max(1);
        let digits = line_count.to_string().len().max(2);
        let gutter_font = FontId::new(font_size, FontFamily::Monospace);
        // Cache the monospace glyph width per font size in egui memory
        // so we're not doing a full font layout every frame just to
        // measure the number "0".
        let gutter_char_w = {
            let key = egui::Id::new(("gutter_char_w", font_size.to_bits()));
            if let Some(w) = ui.memory(|m| m.data.get_temp::<f32>(key)) {
                w
            } else {
                let w = ui
                    .fonts_mut(|f| {
                        f.layout_no_wrap(
                            "0".to_string(),
                            gutter_font.clone(),
                            Color32::WHITE,
                        )
                    })
                    .size()
                    .x;
                ui.memory_mut(|m| m.data.insert_temp(key, w));
                w
            }
        };
        let gutter_w = gutter_char_w * digits as f32 + 16.0;
        // Image files: decode + upload a GPU texture once, then display.
        if is_image_path(&tab.path) {
            if tab.image_texture.is_none()
                && let Ok(bytes) = std::fs::read(&tab.path)
                && let Ok(img) = image::load_from_memory(&bytes)
            {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                tab.image_texture = Some(ui.ctx().load_texture(
                    format!("crane_img:{}", tab.path),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
            ScrollArea::both()
                .id_salt(("image_scroll", active_idx))
                .auto_shrink([false; 2])
                .max_height(editor_h)
                .show(ui, |ui| {
                    if let Some(tex) = &tab.image_texture {
                        let size = tex.size_vec2();
                        ui.add(egui::Image::from_texture(tex).fit_to_original_size(1.0).max_size(size));
                    } else {
                        ui.label(
                            RichText::new("Couldn't decode image")
                                .color(theme::current().error.to_color32()),
                        );
                    }
                });
            return;
        }

        // Markdown preview mode: render formatted HTML instead of the
        // source editor. Same content buffer, no separate store.
        if is_md && tab.preview_mode {
            ScrollArea::vertical()
                .id_salt(("md_preview", active_idx))
                .auto_shrink([false; 2])
                .max_height(editor_h)
                .show(ui, |ui| {
                    crate::views::markdown_view::render_md(
                        ui,
                        &tab.content,
                        font_size,
                    );
                });
            return;
        }

        let scroll_out = ScrollArea::both()
            .id_salt(("file_scroll", active_idx))
            .auto_shrink([false; 2])
            .max_height(editor_h)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // Gutter: right-aligned muted line numbers in the editor's
                    // monospace font so the baseline matches the code rows.
                    let gutter_color = theme::current().text_muted.to_color32();
                    ui.vertical(|ui| {
                        ui.set_min_width(gutter_w);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut job = LayoutJob::default();
                        for n in 1..=line_count {
                            let s = format!("{n:>width$}  \n", n = n, width = digits);
                            job.append(
                                &s,
                                0.0,
                                TextFormat {
                                    font_id: gutter_font.clone(),
                                    color: gutter_color,
                                    ..Default::default()
                                },
                            );
                        }
                        ui.add(egui::Label::new(job).selectable(false));
                    });
                    // Scope the TextEdit's widget id by file path — without
                    // this every tab in a Files Pane shared the same
                    // source-location-derived id, so undo/redo history
                    // (and cursor position + selection) leaked across files.
                    // Ctrl+Z in file A would replay edits made in file B.
                    let tab_path_for_id = tab.path.clone();
                    ui.push_id(("file_editor", tab_path_for_id), |ui| {
                        let te_id = ui.id().with("body");
                        // Project-local indent rules from nearest .prettierrc
                        // or package.json "prettier" field. In a monorepo
                        // each subproject's rules apply to its own files.
                        let style = crate::format::discover(Path::new(&tab.path));
                        let indent = style.indent_unit();
                        let focused = ui.memory(|m| m.has_focus(te_id));
                        if focused {
                            // Cmd+Shift+Z → redo is already handled by egui
                            // 0.34's TextEdit (checked in its builder.rs —
                            // matches Modifiers::SHIFT | COMMAND with Z).
                            // Don't intercept; consuming the event here
                            // actually *prevents* the native redo from
                            // seeing it.
                            let (tab_pressed, enter_pressed) = ui.input_mut(|i| {
                                let t = i.key_pressed(egui::Key::Tab)
                                    && !i.modifiers.shift
                                    && !i.modifiers.command
                                    && !i.modifiers.mac_cmd;
                                if t {
                                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                                }
                                let e = i.key_pressed(egui::Key::Enter)
                                    && !i.modifiers.shift
                                    && !i.modifiers.command
                                    && !i.modifiers.mac_cmd;
                                if e {
                                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                                }
                                (t, e)
                            });

                            if tab_pressed
                                && let Some(mut state) =
                                    egui::TextEdit::load_state(ui.ctx(), te_id)
                            {
                                let cursor = state
                                    .cursor
                                    .char_range()
                                    .map(|r| r.primary.index)
                                    .unwrap_or(0);
                                let byte =
                                    crate::format::char_idx_to_byte(&tab.content, cursor);
                                tab.content.insert_str(byte, &indent);
                                let new_cc = egui::text::CCursor::new(
                                    cursor + indent.chars().count(),
                                );
                                state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::one(new_cc),
                                ));
                                state.store(ui.ctx(), te_id);
                            }

                            if enter_pressed
                                && let Some(mut state) =
                                    egui::TextEdit::load_state(ui.ctx(), te_id)
                            {
                                let cursor = state
                                    .cursor
                                    .char_range()
                                    .map(|r| r.primary.index)
                                    .unwrap_or(0);
                                let byte =
                                    crate::format::char_idx_to_byte(&tab.content, cursor);
                                let (prev_indent, bump) =
                                    crate::format::auto_indent_context(&tab.content, byte);
                                let next_is_close = tab
                                    .content
                                    .as_bytes()
                                    .get(byte)
                                    .map(|c| matches!(c, b'}' | b')' | b']'))
                                    .unwrap_or(false);
                                let body_indent =
                                    if bump { format!("{prev_indent}{indent}") } else { prev_indent.clone() };
                                let inserted = if bump && next_is_close {
                                    // Sitting between { and } — split onto
                                    // three lines, cursor on the indented
                                    // middle one.
                                    format!("\n{body_indent}\n{prev_indent}")
                                } else {
                                    format!("\n{body_indent}")
                                };
                                tab.content.insert_str(byte, &inserted);
                                let advance = if bump && next_is_close {
                                    // cursor lands after the first newline
                                    // + body_indent (so before the second
                                    // newline).
                                    1 + body_indent.chars().count()
                                } else {
                                    inserted.chars().count()
                                };
                                let new_cc = egui::text::CCursor::new(cursor + advance);
                                state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::one(new_cc),
                                ));
                                state.store(ui.ctx(), te_id);
                            }
                        }
                        // Cmd+X on an empty selection cuts the whole line
                        // (trailing newline included). Matches VS Code /
                        // JetBrains behavior. Intercepted before TextEdit
                        // runs so its default no-op path doesn't swallow
                        // the shortcut.
                        // macOS: egui synthesizes Event::Cut from Cmd+X
                        // without emitting a Key event, so `consume_key`
                        // never fires. Detect the Cut event directly and
                        // strip it from the queue when we handle it.
                        let cut_line = ui.memory(|m| m.has_focus(te_id))
                            && ui.input_mut(|i| {
                                let idx = i.events.iter().position(|e| {
                                    matches!(e, egui::Event::Cut)
                                });
                                if let Some(idx) = idx {
                                    i.events.remove(idx);
                                    true
                                } else {
                                    false
                                }
                            });
                        if cut_line
                            && let Some(mut state) =
                                egui::TextEdit::load_state(ui.ctx(), te_id)
                        {
                            let range = state
                                .cursor
                                .char_range()
                                .unwrap_or_else(|| egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(0),
                                ));
                            let empty = range.primary.index == range.secondary.index;
                            if empty {
                                let cursor = range.primary.index;
                                let byte = crate::format::char_idx_to_byte(
                                    &tab.content,
                                    cursor,
                                );
                                let bytes = tab.content.as_bytes();
                                let line_start = bytes[..byte]
                                    .iter()
                                    .rposition(|b| *b == b'\n')
                                    .map(|i| i + 1)
                                    .unwrap_or(0);
                                let line_end = bytes[byte..]
                                    .iter()
                                    .position(|b| *b == b'\n')
                                    .map(|i| byte + i + 1)
                                    .unwrap_or(bytes.len());
                                let cut = tab.content[line_start..line_end].to_string();
                                if !cut.is_empty() {
                                    // Push pre-cut state onto the TextEdit's
                                    // undo stack as its own entry so each
                                    // Cmd+X is one discrete Cmd+Z step
                                    // (otherwise egui's time-debounced
                                    // feed_state merges rapid cuts).
                                    let mut undoer = state.undoer();
                                    undoer.add_undo(&(range, tab.content.clone()));
                                    state.set_undoer(undoer);

                                    ui.ctx().copy_text(cut);
                                    tab.content.replace_range(line_start..line_end, "");
                                    let line_start_char = tab.content[..line_start]
                                        .chars()
                                        .count();
                                    let new_cc = egui::text::CCursor::new(line_start_char);
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::one(new_cc),
                                    ));
                                    state.store(ui.ctx(), te_id);
                                }
                            } else {
                                // Non-empty selection — forward a normal Cut
                                // event so the TextEdit performs a standard cut.
                                ui.input_mut(|i| i.events.push(egui::Event::Cut));
                            }
                        }
                        let editor = egui::TextEdit::multiline(&mut tab.content)
                            .id(te_id)
                            .code_editor()
                            .lock_focus(true)
                            .frame(egui::Frame::NONE)
                            .desired_width(if prefs.word_wrap {
                                ui.available_width()
                            } else {
                                f32::INFINITY
                            })
                            .desired_rows(30)
                            .layouter(&mut layouter);
                        let out = editor.show(ui);
                        diagnostics_overlay::paint(
                            ui,
                            &out.galley,
                            out.galley_pos,
                            &diagnostics,
                        );
                        // Find-bar match highlights — soft amber fill
                        // behind every occurrence of the query in view.
                        if let Some(q) = tab.find_query.as_deref()
                            && !q.is_empty()
                        {
                            paint_find_matches(
                                ui,
                                &out.galley,
                                out.galley_pos,
                                &tab.content,
                                q,
                            );
                        }

                        // Apply a pending cursor (goto-definition landed
                        // here in this frame or a previous one).
                        if let Some((line, ch)) = tab.pending_cursor.take()
                            && let Some(mut state) =
                                egui::TextEdit::load_state(ui.ctx(), te_id)
                        {
                            let cc = crate::format::char_idx_to_byte(
                                &tab.content,
                                line_col_to_char(&tab.content, line, ch),
                            );
                            let _ = cc;
                            let cursor = line_col_to_char(&tab.content, line, ch);
                            let new_cc = egui::text::CCursor::new(cursor);
                            state.cursor.set_char_range(Some(
                                egui::text::CCursorRange::one(new_cc),
                            ));
                            state.store(ui.ctx(), te_id);
                            ui.memory_mut(|m| m.request_focus(te_id));
                        }

                        // F12 or Cmd+click → goto-definition at cursor.
                        let f12 = ui
                            .input(|i| i.key_pressed(egui::Key::F12))
                            && ui.memory(|m| m.has_focus(te_id));
                        let cmd_click = out.response.clicked()
                            && ui.input(|i| {
                                i.modifiers.command || i.modifiers.mac_cmd
                            });
                        if f12 || cmd_click {
                            // Locate the cursor's (line, col).
                            let cc_idx = if cmd_click
                                && let Some(p) = out.response.interact_pointer_pos()
                            {
                                let rel =
                                    egui::pos2(p.x - out.galley_pos.x, p.y - out.galley_pos.y);
                                out.galley.cursor_from_pos(rel.to_vec2()).index
                            } else {
                                egui::TextEdit::load_state(ui.ctx(), te_id)
                                    .and_then(|s| {
                                        s.cursor.char_range().map(|r| r.primary.index)
                                    })
                                    .unwrap_or(0)
                            };
                            let (line, ch) =
                                char_idx_to_line_col(&tab.content, cc_idx);
                            goto_request(&tab.path, line, ch);
                        }
                        // Note: egui 0.34 keeps TextEditState.undoer private,
                        // so we can't tighten Ctrl+Z granularity without
                        // forking. Upstream issue; revisit when possible.
                        let ctx_save = std::rc::Rc::new(std::cell::Cell::new(false));
                        let ctx_reveal = std::rc::Rc::new(std::cell::Cell::new(false));
                        let ctx_copy = std::rc::Rc::new(std::cell::Cell::new(false));
                        let path_for_copy = tab.path.clone();
                        let cs = ctx_save.clone();
                        let cr = ctx_reveal.clone();
                        let cc = ctx_copy.clone();
                        out.response.context_menu(|ui| {
                            if ui.button(format!("{}  Save", icons::FLOPPY_DISK)).clicked() {
                                cs.set(true);
                                ui.close();
                            }
                            if ui.button(format!("{}  Reveal in Finder", icons::FOLDER_OPEN)).clicked() {
                                cr.set(true);
                                ui.close();
                            }
                            if ui.button(format!("{}  Copy Path", icons::COPY)).clicked() {
                                ui.ctx().copy_text(path_for_copy.clone());
                                cc.set(true);
                                ui.close();
                            }
                        });
                        if ctx_save.get() {
                            save_tab(tab, prefs, format_before_save, notify_saved, false);
                        }
                        if ctx_reveal.get() {
                            reveal_in_file_manager(&tab.path);
                        }
                        let _ = ctx_copy.get();
                    });
                });
            });

        paint_scrollbar_diag_markers(
            ui,
            scroll_out.inner_rect,
            line_count,
            &diagnostics,
        );

        render_file_status_strip(ui, tab, &diagnostics, status_h);
    }
}

/// Per-file status strip at the bottom of the Files pane.
/// Left: diagnostics counts (click = jump to next of that severity).
/// Right: Ln/Col · indent · language.
fn render_file_status_strip(
    ui: &mut egui::Ui,
    tab: &mut crate::state::layout::FileTab,
    diagnostics: &[Diagnostic],
    height: f32,
) {
    let t = theme::current();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y),
        ],
        egui::Stroke::new(1.0, t.divider.to_color32()),
    );

    let (e, w, i) = count_by_severity(diagnostics);

    let te_id = ui
        .id()
        .with(("file_editor", &tab.path))
        .with("body");
    let cursor_idx = egui::TextEdit::load_state(ui.ctx(), te_id)
        .and_then(|s| s.cursor.char_range().map(|r| r.primary.index))
        .unwrap_or(0);
    let (cur_line, cur_col) = char_idx_to_line_col(&tab.content, cursor_idx);

    let style = crate::format::discover(Path::new(&tab.path));
    let indent_label = if style.use_tabs {
        "Tabs".to_string()
    } else {
        format!("Spaces: {}", style.tab_width)
    };
    let lang = language_label(&tab.path);

    let mut clicked_sev: Option<u8> = None;
    let pills: [(&str, usize, Color32, u8); 3] = [
        (icons::X_CIRCLE, e, t.error.to_color32(), 1),
        (icons::WARNING, w, Color32::from_rgb(226, 192, 80), 2),
        (icons::INFO, i, t.accent.to_color32(), 3),
    ];
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal_centered(|ui| {
            for (icon, count, active_color, sev) in pills {
                ui.add_space(8.0);
                if sev_button(ui, icon, count, active_color, t.text_muted.to_color32()) {
                    clicked_sev = Some(sev);
                }
            }

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(lang)
                            .size(11.0)
                            .color(t.text_muted.to_color32())
                            .monospace(),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(indent_label)
                            .size(11.0)
                            .color(t.text_muted.to_color32()),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!("Ln {}, Col {}", cur_line + 1, cur_col + 1))
                            .size(11.0)
                            .color(t.text_muted.to_color32()),
                    );
                },
            );
        });
    });

    if let Some(sev) = clicked_sev {
        jump_to_next_diagnostic(tab, diagnostics, sev, cur_line);
    }
}

fn sev_button(
    ui: &mut egui::Ui,
    icon: &str,
    count: usize,
    active_color: Color32,
    muted_color: Color32,
) -> bool {
    let color = if count > 0 { active_color } else { muted_color };
    let resp = ui.add(
        egui::Label::new(
            RichText::new(format!("{icon}  {count}"))
                .size(11.5)
                .color(color),
        )
        .sense(egui::Sense::click()),
    );
    if resp.hovered() && count > 0 {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked() && count > 0
}

fn count_by_severity(diags: &[Diagnostic]) -> (usize, usize, usize) {
    let mut e = 0;
    let mut w = 0;
    let mut i = 0;
    for d in diags {
        match d.severity {
            1 => e += 1,
            2 => w += 1,
            _ => i += 1,
        }
    }
    (e, w, i)
}

fn jump_to_next_diagnostic(
    tab: &mut crate::state::layout::FileTab,
    diags: &[Diagnostic],
    sev: u8,
    from_line: u32,
) {
    let matching: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| match sev {
            1 => d.severity == 1,
            2 => d.severity == 2,
            _ => d.severity == 0 || d.severity >= 3,
        })
        .collect();
    if matching.is_empty() {
        return;
    }
    let next = matching
        .iter()
        .find(|d| d.line > from_line)
        .or_else(|| matching.first());
    if let Some(d) = next {
        tab.pending_cursor = Some((d.line, d.col_start));
    }
}

fn language_label(path: &str) -> String {
    // Extensionless files with a well-known basename (Dockerfile, Makefile, …).
    if let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) {
        match name.to_ascii_lowercase().as_str() {
            "dockerfile" | "containerfile" => return "Dockerfile".to_string(),
            "makefile" | "gnumakefile" => return "Makefile".to_string(),
            "cmakelists.txt" => return "CMake".to_string(),
            "jenkinsfile" => return "Groovy".to_string(),
            "rakefile" | "gemfile" | "podfile" => return "Ruby".to_string(),
            "cargo.lock" => return "TOML".to_string(),
            _ => {}
        }
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "Rust",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "py" | "pyi" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" | "sc" => "Scala",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "C++",
        "cs" => "C#",
        "swift" => "Swift",
        "m" | "mm" => "Objective-C",
        "rb" => "Ruby",
        "php" => "PHP",
        "lua" => "Lua",
        "pl" | "pm" => "Perl",
        "r" => "R",
        "dart" => "Dart",
        "zig" => "Zig",
        "nim" => "Nim",
        "hs" => "Haskell",
        "elm" => "Elm",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "clj" | "cljs" | "cljc" | "edn" => "Clojure",
        "ml" | "mli" => "OCaml",
        "fs" | "fsx" | "fsi" => "F#",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "sql" => "SQL",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "less" => "Less",
        "html" | "htm" => "HTML",
        "xml" | "xsl" | "xslt" => "XML",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "md" | "markdown" | "mdx" => "Markdown",
        "json" | "jsonc" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "ini" | "cfg" | "conf" => "INI",
        "env" => "Dotenv",
        "dockerfile" => "Dockerfile",
        "makefile" | "mk" => "Makefile",
        "graphql" | "gql" => "GraphQL",
        "proto" => "Protobuf",
        "tf" | "tfvars" => "Terraform",
        "nix" => "Nix",
        "prisma" => "Prisma",
        "tex" | "latex" => "LaTeX",
        "vim" => "Vim",
        "diff" | "patch" => "Diff",
        "log" => "Log",
        "txt" => "Plain",
        "" => "Plain",
        other => return other.to_string(),
    }
    .to_string()
}


/// Paints colored ticks along the right edge of the scroll viewport,
/// one per diagnostic, proportional to its line. Minimap-lite: gives the
/// user a visual overview of where issues sit without opening a panel.
fn paint_scrollbar_diag_markers(
    ui: &egui::Ui,
    scroll_rect: egui::Rect,
    total_lines: usize,
    diagnostics: &[Diagnostic],
) {
    if diagnostics.is_empty() || total_lines == 0 {
        return;
    }
    let t = theme::current();
    let painter = ui.painter_at(scroll_rect);
    // Strip sits just inside the right edge of the viewport.
    let strip_w = 3.0;
    let x1 = scroll_rect.max.x - 2.0;
    let x0 = x1 - strip_w;
    let h = scroll_rect.height();
    let total = total_lines.max(1) as f32;
    for d in diagnostics {
        let color = match d.severity {
            1 => t.error.to_color32(),
            2 => Color32::from_rgb(226, 192, 80),
            _ => t.accent.to_color32(),
        };
        let y = scroll_rect.min.y + (d.line as f32 / total) * h;
        let rect = egui::Rect::from_min_max(
            egui::pos2(x0, y - 1.0),
            egui::pos2(x1, y + 1.0),
        );
        painter.rect_filled(rect, 0.5, color);
    }
}

struct FindBarOutcome {
    close: bool,
    next: bool,
    prev: bool,
}

/// Renders the Cmd+F find bar when `tab.find_query` is Some. Returns
/// which navigation action was triggered this frame (close / next /
/// prev). Mutates only the editable query string; the caller is
/// responsible for clearing `tab.find_query` on close.
fn render_find_bar(ui: &mut egui::Ui, tab: &mut crate::state::layout::FileTab) -> FindBarOutcome {
    let mut close = false;
    let mut next = false;
    let mut prev = false;
    let Some(query) = tab.find_query.as_mut() else {
        // Bar just closed — reset the one-shot focus flag so the next
        // Cmd+F will refocus cleanly.
        let focus_flag = egui::Id::new(("find_focused", &tab.path));
        ui.memory_mut(|m| {
            m.data.remove::<bool>(focus_flag);
        });
        return FindBarOutcome { close, next, prev };
    };
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{}  Find", icons::MAGNIFYING_GLASS))
                .size(11.0)
                .color(theme::current().text_muted.to_color32()),
        );
        let input_id = egui::Id::new(("find_input", &tab.path));
        let resp = ui.add(
            egui::TextEdit::singleline(query)
                .id(input_id)
                .desired_width(ui.available_width() - 180.0)
                .hint_text("type to search…"),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            next = true;
        }
        // Focus ONCE when the bar opens — per-frame request_focus was
        // stealing clicks from the nav/close buttons.
        let focus_flag = egui::Id::new(("find_focused", &tab.path));
        let already_focused = ui
            .memory(|m| m.data.get_temp::<bool>(focus_flag))
            .unwrap_or(false);
        if !already_focused {
            resp.request_focus();
            ui.memory_mut(|m| m.data.insert_temp(focus_flag, true));
        }
        let hits = if query.is_empty() {
            0
        } else {
            tab.content.matches(query.as_str()).count()
        };
        ui.label(
            RichText::new(format!("{hits} hits"))
                .size(10.5)
                .color(theme::current().text_muted.to_color32()),
        );
        let btn = |glyph: &str| {
            egui::Button::new(
                RichText::new(glyph)
                    .size(14.0)
                    .color(theme::current().text.to_color32()),
            )
            .min_size(egui::vec2(22.0, 22.0))
        };
        if ui
            .add(btn(icons::ARROW_UP))
            .on_hover_text("Previous (Shift+Enter)")
            .clicked()
        {
            prev = true;
        }
        if ui
            .add(btn(icons::ARROW_DOWN))
            .on_hover_text("Next (Enter)")
            .clicked()
        {
            next = true;
        }
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.add_space(6.0);
                if ui
                    .add(btn(icons::X_CIRCLE))
                    .on_hover_text("Close (Esc)")
                    .clicked()
                {
                    close = true;
                }
            },
        );
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.shift) {
        prev = true;
    }
    ui.add_space(2.0);
    FindBarOutcome { close, next, prev }
}

/// Write `tab.content` to disk. When `force` is false and `tab.external_change`
/// is set, the save is refused — the caller is expected to surface the
/// banner that lets the user pick Reload / Overwrite (force) / Cancel.
fn save_tab(
    tab: &mut crate::state::layout::FileTab,
    prefs: EditorPrefs,
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    notify_saved: &dyn Fn(&str, &str),
    force: bool,
) {
    if tab.external_change && !force {
        return;
    }
    if prefs.trim_on_save {
        tab.content = trim_trailing_whitespace(&tab.content);
    }
    if let Some(formatted) = format_before_save(&tab.content, &tab.path) {
        tab.content = formatted;
    }
    if let Err(e) = std::fs::write(&tab.path, &tab.content) {
        eprintln!("save failed: {e}");
        return;
    }
    tab.original_content = tab.content.clone();
    tab.disk_mtime = std::fs::metadata(&tab.path)
        .and_then(|m| m.modified())
        .ok();
    tab.external_change = false;
    notify_saved(&tab.path, &tab.content);
}

/// Poll the filesystem for external edits to the active tab. Sets
/// `tab.external_change` when the mtime advanced AND the disk bytes
/// differ from what we'd write. Called once per render for the active
/// tab — cheap on SSDs and gated by the mtime check.
fn poll_external_change(tab: &mut crate::state::layout::FileTab) {
    if tab.external_change {
        return;
    }
    let Ok(meta) = std::fs::metadata(&tab.path) else {
        return;
    };
    let Ok(disk_mtime) = meta.modified() else {
        return;
    };
    let stale = match tab.disk_mtime {
        Some(prev) => disk_mtime > prev,
        None => true,
    };
    if !stale {
        return;
    }
    // mtime advanced — compare bytes before alarming (some editors
    // rewrite without changing content).
    let Ok(disk_content) = std::fs::read_to_string(&tab.path) else {
        return;
    };
    if disk_content == tab.original_content {
        // Content matches our baseline: silently catch up the mtime.
        tab.disk_mtime = Some(disk_mtime);
        return;
    }
    tab.external_change = true;
}

/// Reload the active tab from disk, discarding any unsaved edits.
fn reload_tab(tab: &mut crate::state::layout::FileTab) {
    let Ok(disk_content) = std::fs::read_to_string(&tab.path) else {
        return;
    };
    tab.content = disk_content.clone();
    tab.original_content = disk_content;
    tab.disk_mtime = std::fs::metadata(&tab.path)
        .and_then(|m| m.modified())
        .ok();
    tab.external_change = false;
}

fn paint_find_matches(
    ui: &egui::Ui,
    galley: &std::sync::Arc<egui::Galley>,
    origin: egui::Pos2,
    text: &str,
    query: &str,
) {
    let amber = Color32::from_rgba_unmultiplied(220, 180, 50, 90);
    let painter = ui.painter();
    let mut byte = 0usize;
    while let Some(offset) = text[byte..].find(query) {
        let abs = byte + offset;
        let end = abs + query.len();
        let char_start = text[..abs].chars().count();
        let char_end = char_start + text[abs..end].chars().count();
        let r_start = galley.pos_from_cursor(egui::text::CCursor::new(char_start));
        let r_end = galley.pos_from_cursor(egui::text::CCursor::new(char_end));
        // Only paint matches that fit on a single visual line (the common
        // case for a user-typed query; skipping multi-line avoids ugly
        // cross-row rectangles).
        if (r_start.max.y - r_end.max.y).abs() < 1.0 {
            let rect = egui::Rect::from_min_max(
                egui::pos2(origin.x + r_start.min.x, origin.y + r_start.min.y),
                egui::pos2(origin.x + r_end.max.x, origin.y + r_start.max.y),
            );
            painter.rect_filled(rect, 2.0, amber);
        }
        byte = end;
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

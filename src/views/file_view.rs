use crate::state::layout::FilesPane;
use crate::lsp::Diagnostic;
use crate::theme;
use crate::views::diagnostics_overlay;
use crate::views::file_util::{
    char_idx_to_line_col, is_image_path, line_col_to_char, reveal_in_file_manager,
    short_path,
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



pub fn syntaxes() -> &'static SyntaxSet {
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

/// Guaranteed-present fallback used when the user's requested theme
/// (and every named fallback) is missing and the ThemeSet happens to
/// be empty — e.g. if a future two_face version drops an embedded
/// theme or a user strips themes via config. Returning this instead
/// of panicking keeps the editor usable with default (uncolored)
/// syntax output.
pub fn fallback_theme() -> &'static syntect::highlighting::Theme {
    static FALLBACK: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    FALLBACK.get_or_init(syntect::highlighting::Theme::default)
}

pub fn themes() -> &'static ThemeSet {
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
pub fn find_syntax_for_ext(ext: &str) -> &'static syntect::parsing::SyntaxReference {
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

/// Confirm modal for closing a dirty file tab. "Discard" drops the
/// unsaved edits; "Cancel" leaves the tab open. Save-then-close isn't
/// offered here because saving runs formatters / notify_saved / disk
/// I/O that the render_scoped closure wires up — we don't thread
/// those into the tab bar. The user can click Cancel, Cmd+S, then
/// close cleanly.
fn render_close_confirm(ui: &mut egui::Ui, pane: &mut crate::state::layout::FilesPane) {
    let Some(idx) = pane.pending_close else {
        return;
    };
    let Some(tab) = pane.tabs.get(idx) else {
        pane.pending_close = None;
        return;
    };
    let name = tab.name.clone();
    let mut cancel = false;
    let mut confirm = false;
    egui::Window::new("Unsaved changes")
        .id(egui::Id::new(("file_close_confirm", idx)))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.set_min_width(340.0);
            ui.add_space(4.0);
            ui.label(format!("\"{name}\" has unsaved changes."));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Discard them and close the tab?")
                    .color(theme::current().text_muted.to_color32())
                    .size(11.5),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button("Discard").clicked() {
                    confirm = true;
                }
            });
        });
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        pane.pending_close = None;
    } else if confirm {
        pane.close(idx);
        pane.pending_close = None;
    }
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
        // Dirty tab: route through a confirm modal so the user isn't
        // a stray click away from losing unsaved work. Clean tabs
        // close immediately.
        if pane.tabs.get(idx).map(|t| t.dirty()).unwrap_or(false) {
            pane.pending_close = Some(idx);
        } else {
            pane.close(idx);
            if pane.tabs.is_empty() {
                return;
            }
        }
    }
    render_close_confirm(ui, pane);
    if pane.tabs.is_empty() {
        return;
    }
    ui.add_space(2.0);

    let active_idx = pane.active.min(pane.tabs.len() - 1);
    pane.active = active_idx;

    // Save shortcut
    let save_pressed = ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::S)
    });
    // Cmd+F opens the find bar (or replaces the query with the current
    // selection). Esc closes it — Cmd+F never closes.
    let find_toggle = ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::F)
    });
    if find_toggle
        && !pane.tabs.is_empty()
    {
        let idx = pane.active.min(pane.tabs.len() - 1);
        let t = &mut pane.tabs[idx];
        let te_id = egui::Id::new(("file_editor", &t.path)).with("body");
        let selection = egui::TextEdit::load_state(ui.ctx(), te_id)
            .and_then(|s| s.cursor.char_range())
            .filter(|r| r.primary.index != r.secondary.index)
            .map(|r| {
                let start = r.primary.index.min(r.secondary.index);
                let end = r.primary.index.max(r.secondary.index);
                let start_byte = crate::format::char_idx_to_byte(&t.content, start);
                let end_byte = crate::format::char_idx_to_byte(&t.content, end);
                t.content[start_byte..end_byte].to_string()
            });
        if let Some(sel) = selection {
            t.find_query = Some(sel);
        } else if t.find_query.is_none() {
            t.find_query = Some(String::new());
        }
    }

    {
        let tab = &mut pane.tabs[active_idx];
        crate::views::file_save::poll_external_change(tab);
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
                                    crate::views::file_save::save_tab(
                                        tab,
                                        prefs,
                                        format_before_save,
                                        notify_saved,
                                        true,
                                    );
                                }
                                if ui.small_button("Reload").clicked() {
                                    crate::views::file_save::reload_tab(tab);
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
                        crate::views::file_save::save_tab(tab, prefs, format_before_save, notify_saved, false);
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
        let crate::views::file_find::FindBarOutcome {
            close: find_close,
            next: find_next,
            prev: find_prev,
        } = crate::views::file_find::render_find_bar(ui, tab);
        if find_close {
            tab.find_query = None;
        }
        if (find_next || find_prev)
            && let Some(q) = tab.find_query.clone()
            && !q.is_empty()
        {
            // Jump cursor to the next / prev occurrence of `q`.
            let te_id = egui::Id::new(("file_editor", &tab.path)).with("body");
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
                // Store target line so the scroll area can scroll to it
                // when it renders on the next frame.
                tab.find_scroll_to_line = Some(line);
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
            .unwrap_or_else(|| all.values().next().unwrap_or_else(|| fallback_theme()));
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
        let gutter_size = font_size * 0.7;
        let gutter_font = FontId::new(gutter_size, FontFamily::Monospace);
        // Cache the monospace glyph width per font size in egui memory
        // so we're not doing a full font layout every frame just to
        // measure the number "0".
        let gutter_char_w = {
            let key = egui::Id::new(("gutter_char_w", gutter_size.to_bits()));
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

        // Two-column layout: fixed gutter on the left, horizontally-
        // scrollable code on the right. We use a manual child UI for the
        // gutter and a ScrollArea for the code, sharing vertical scroll.
        let avail = ui.available_rect_before_wrap();
        let gutter_rect = egui::Rect::from_min_size(
            avail.min,
            egui::vec2(gutter_w, editor_h),
        );
        let code_left = avail.min.x + gutter_w;

        // --- Code area (scrolls both ways) ---
        // Shrink the available width so the scroll area sits to the right
        // of the gutter column.
        let code_pad = 6.0; // gap between gutter and code
        let mut code_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(code_left + code_pad, avail.min.y),
                    egui::vec2(avail.width() - gutter_w - code_pad, avail.height()),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let code_ui_id = code_ui.id();

        // Capture the actual row height from the TextEdit's galley so the
        // gutter aligns exactly — no guessed multiplier needed.
        let mut actual_row_h = font_size * 1.2;

        let scroll_out = ScrollArea::both()
            .id_salt(("file_scroll", active_idx))
            .auto_shrink([false; 2])
            .max_height(editor_h)
            .show(&mut code_ui, |ui| {
                ui.horizontal_top(|ui| {
                    // Scope the TextEdit's widget id by file path — without
                    // this every tab in a Files Pane shared the same
                    // source-location-derived id, so undo/redo history
                    // (and cursor position + selection) leaked across files.
                    // Ctrl+Z in file A would replay edits made in file B.
                    let tab_path_for_id = tab.path.clone();
                    let te_id = egui::Id::new(("file_editor", &tab_path_for_id)).with("body");
                    ui.push_id(("file_editor", tab_path_for_id), |ui| {
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
                        // Capture the actual row height from the galley so the
                        // gutter aligns exactly with code lines.
                        if out.galley.rows.len() >= 2 {
                            actual_row_h = out.galley.rows[1].rect().min.y
                                - out.galley.rows[0].rect().min.y;
                        } else if let Some(row) = out.galley.rows.first() {
                            actual_row_h = row.rect().height();
                        }
                        // Stash the current primary cursor so the
                        // status strip below renders up-to-date
                        // Ln/Col. `TextEdit::load_state` from the
                        // outer scope returns state whose id path
                        // depends on ancestor push_id layering —
                        // subtle to match, so we just read from the
                        // output here where the id is known correct.
                        if let Some(range) = out.state.cursor.char_range() {
                            tab.last_cursor_idx = range.primary.index;
                        }
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
                            crate::views::file_find::paint_find_matches(
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
                            crate::views::file_save::save_tab(tab, prefs, format_before_save, notify_saved, false);
                        }
                        if ctx_reveal.get() {
                            reveal_in_file_manager(&tab.path);
                        }
                        let _ = ctx_copy.get();
                    });
                });
            });

        // Scroll to find-match target line if requested.
        if let Some(line) = tab.find_scroll_to_line.take() {
            let row_h = actual_row_h;
            let target_y = line as f32 * row_h;
            let mut state = scroll_out.state;
            if target_y < state.offset.y + row_h {
                state.offset.y = (target_y - row_h).max(0.0);
            } else if target_y > state.offset.y + editor_h - row_h * 2.0 {
                state.offset.y = target_y - editor_h + row_h * 3.0;
            }
            state.offset.y = state.offset.y.max(0.0);
            // Must match the ScrollArea's internal state ID:
            // state_id = parent_ui.id().with(id_salt)
            let scroll_state_id =
                code_ui_id.with(egui::Id::new(("file_scroll", active_idx)));
            ui.ctx().data_mut(|d| {
                d.insert_temp(scroll_state_id, state);
            });
        }

        // Consume the full editor height in the parent ui.
        ui.allocate_rect(
            egui::Rect::from_min_size(
                ui.available_rect_before_wrap().min,
                egui::vec2(ui.available_width(), editor_h),
            ),
            egui::Sense::hover(),
        );

        // Paint the fixed gutter overlay. Reads the code area's vertical
        // scroll offset so line numbers scroll in sync with the code,
        // but ignores horizontal scroll entirely (stays pinned left).
        let v_offset = scroll_out.state.offset.y;
        let gutter_bg = theme::current().bg.to_color32();
        let gutter_fg = theme::current().text_muted.to_color32();
        let painter = ui.painter();
        // Background fill to hide code scrolling behind the gutter.
        painter.rect_filled(gutter_rect, 0.0, gutter_bg);
        // Right border separating gutter from code.
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(gutter_rect.max.x - 1.0, gutter_rect.min.y),
                egui::vec2(1.0, gutter_rect.height()),
            ),
            0.0,
            theme::current().border.to_color32(),
        );

        // Refresh per-line git change data when the content changes.
        let content_hash = {
            use std::hash::Hasher;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&tab.content, &mut h);
            h.finish()
        };
        if tab.line_changes_key != content_hash {
            tab.line_changes = workspace_root.and_then(|root| {
                let rel = std::path::Path::new(&tab.path).strip_prefix(root).ok()?;
                crate::git::parse_file_diff(root, rel.to_str()?)
            });
            tab.line_changes_key = content_hash;
        }
        let file_diff = tab.line_changes.as_ref();

        let t = theme::current();
        let green = t.diff_added();
        let blue = t.diff_modified();
        let red = t.diff_deleted();
        let diff_old = if t.is_dark() {
            Color32::from_rgb(200, 120, 120)
        } else {
            Color32::from_rgb(160, 50, 50)
        };
        let diff_new = if t.is_dark() {
            Color32::from_rgb(120, 200, 140)
        } else {
            Color32::from_rgb(30, 140, 60)
        };

        // Paint line numbers + gutter change markers.
        let row_h = actual_row_h;
        let first_visible = (v_offset / row_h).floor() as usize;
        let last_visible = ((v_offset + gutter_rect.height()) / row_h).ceil() as usize;
        let clipped = painter.with_clip_rect(gutter_rect);

        // Tooltip state: (tooltip_pos, first_line_no, old_lines, new_lines)
        let mut tooltip: Option<(egui::Pos2, usize, Vec<String>, Vec<String>)> = None;
        let pointer_pos = ui.ctx().pointer_hover_pos();
        let content_lines: Vec<&str> = tab.content.lines().collect();

        for n in (first_visible + 1)..=(last_visible.min(line_count)) {
            let y = gutter_rect.min.y + (n as f32 - 0.5) * row_h - v_offset;

            if let Some(diff) = file_diff {
                if let Some(dl) = diff.lines.get(&n) {
                    let color = match dl.kind {
                        crate::git::DiffLineKind::Added => green,
                        crate::git::DiffLineKind::Modified => blue,
                    };
                    let marker_rect = egui::Rect::from_min_size(
                        egui::pos2(gutter_rect.min.x, y - row_h * 0.5),
                        egui::vec2(3.0, row_h),
                    );
                    clipped.rect_filled(marker_rect, 0.0, color);

                    // Only blue (Modified) lines are hoverable
                    if dl.kind == crate::git::DiffLineKind::Modified {
                        if let Some(pos) = pointer_pos {
                            if gutter_rect.contains(pos) {
                                let line_top = y - row_h * 0.5;
                                let line_bot = y + row_h * 0.5;
                                if pos.y >= line_top
                                    && pos.y <= line_bot
                                    && tooltip.is_none()
                                {
                                    // Use the hunk's full block — this is what
                                    // makes a `-20 +5` chunk render as one
                                    // change with all 20 old lines visible
                                    // (rather than truncating to the first 5).
                                    if let Some(block) = dl
                                        .block_idx
                                        .and_then(|i| diff.blocks.get(i))
                                    {
                                        let new_lines: Vec<String> = (0..block
                                            .new_count)
                                            .map(|i| {
                                                content_lines
                                                    .get(block.new_start - 1 + i)
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_default()
                                            })
                                            .collect();
                                        let old = block.old_lines.clone();
                                        if old != new_lines {
                                            let tip_y = gutter_rect.min.y
                                                + (block.new_start as f32 - 0.5)
                                                    * row_h
                                                - v_offset
                                                - row_h * 0.5;
                                            tooltip = Some((
                                                egui::pos2(
                                                    gutter_rect.max.x + 4.0,
                                                    tip_y,
                                                ),
                                                block.new_start,
                                                old,
                                                new_lines,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            clipped.text(
                egui::pos2(gutter_rect.max.x - 8.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{n:>width$}", n = n, width = digits),
                gutter_font.clone(),
                gutter_fg,
            );
        }

        // Deletion gap markers: red bar between lines where content was removed.
        if let Some(diff) = file_diff {
            for gap in &diff.deletions {
                let gap_y = if gap.after_line == 0 {
                    gutter_rect.min.y - v_offset
                } else {
                    gutter_rect.min.y + gap.after_line as f32 * row_h - v_offset
                };
                if gap_y >= gutter_rect.min.y - 4.0 && gap_y <= gutter_rect.max.y + 4.0 {
                    let gap_h = 3.0;
                    let gap_rect = egui::Rect::from_min_size(
                        egui::pos2(gutter_rect.min.x, gap_y - gap_h * 0.5),
                        egui::vec2(gutter_rect.width() - 8.0, gap_h),
                    );
                    clipped.rect_filled(gap_rect, 1.0, red);

                    if let Some(pos) = pointer_pos {
                        let expanded = gap_rect.expand2(egui::vec2(0.0, 4.0));
                        if expanded.contains(pos) && tooltip.is_none() {
                            tooltip = Some((
                                egui::pos2(gutter_rect.max.x + 4.0, gap_y - 4.0),
                                gap.after_line,
                                gap.head_lines.clone(),
                                Vec::new(),
                            ));
                        }
                    }
                }
            }
        }

        // Show diff tooltip outside the clip rect
        if let Some((pos, first_line, old_lines, new_lines)) = tooltip {
            egui::show_tooltip_at(
                ui.ctx(),
                egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("gutter_diff_tooltip")),
                egui::Id::new("gutter_diff_tooltip"),
                pos,
                |ui| {
                    ui.vertical(|ui| {
                        let count = old_lines.len().max(new_lines.len());
                        let last_line = first_line + count.saturating_sub(1);
                        ui.label(
                            RichText::new(format!("Lines {}-{}", first_line, last_line))
                                .size(11.0)
                                .strong()
                                .color(Color32::from_rgb(160, 160, 160)),
                        );
                        ui.add_space(2.0);
                        for line in &old_lines {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("-").monospace().size(12.0).color(diff_old));
                                ui.label(RichText::new(line.clone()).monospace().size(12.0).color(diff_old));
                            });
                        }
                        for line in &new_lines {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("+").monospace().size(12.0).color(diff_new));
                                ui.label(RichText::new(line.clone()).monospace().size(12.0).color(diff_new));
                            });
                        }
                    });
                },
            );
        }

        crate::views::file_status::paint_scrollbar_diag_markers(
            ui,
            scroll_out.inner_rect,
            line_count,
            &diagnostics,
        );

        // Git change markers on the scrollbar — colored dashes on the right
        // edge showing where added/modified lines sit.
        if let Some(diff) = file_diff {
            let scroll_rect = scroll_out.inner_rect;
            let scroll_painter = ui.painter_at(scroll_rect);
            let x1 = scroll_rect.max.x - 2.0;
            let x0 = x1 - 6.0;
            let h = scroll_rect.height();
            let total = line_count.max(1) as f32;
            for (&line_no, dl) in &diff.lines {
                let color = match dl.kind {
                    crate::git::DiffLineKind::Added => green,
                    crate::git::DiffLineKind::Modified => blue,
                };
                let y = scroll_rect.min.y + (line_no as f32 / total) * h;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(x0, y - 1.5),
                    egui::pos2(x1, y + 1.5),
                );
                scroll_painter.rect_filled(rect, 1.0, color);
            }
            // Deletion markers on scrollbar
            for gap in &diff.deletions {
                let y = scroll_rect.min.y
                    + ((gap.after_line as f32 + 0.5) / total) * h;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(x0, y - 1.0),
                    egui::pos2(x1, y + 1.0),
                );
                scroll_painter.rect_filled(rect, 1.0, red);
            }
        }

        crate::views::file_status::render_status_strip(ui, tab, &diagnostics, status_h);
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

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, height),
        egui::Sense::click_and_drag(),
    );
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
    // Middle-click anywhere on the tab closes it — browser convention.
    // Counts as a close whether the pointer is over the × button or
    // the label body, so the user doesn't need to aim.
    let closed = close_response.clicked() || response.middle_clicked();
    (
        response.clicked() && !close_response.hovered(),
        closed,
    )
}

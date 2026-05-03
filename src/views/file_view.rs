use crate::state::layout::FilesPane;
use crate::lsp::Diagnostic;
use crate::theme;
use crate::views::diagnostics_overlay;
use crate::views::file_util::{
    char_idx_to_line_col, is_image_path, line_col_to_char, reveal_in_file_manager,
    short_path, toggle_line_comments,
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
    external_drop_handled: bool,
    dropped_external_files: &mut Vec<std::path::PathBuf>,
) -> bool {
    let mut should_close = false;
    ui.push_id(("files_pane", pane_id), |ui| {
        should_close = render_scoped(
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
            external_drop_handled,
            dropped_external_files,
        );
    });
    should_close
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
    let name = tab.name().to_string();
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
    external_drop_handled: bool,
    dropped_external_files: &mut Vec<std::path::PathBuf>,
) -> bool {
    // External drag-drop on file pane: open as read-only tabs.
    // Only handle if the explorer tree didn't already handle the drop.
    if !external_drop_handled {
        let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
            i.raw.dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        dropped_external_files.extend(dropped);
    }

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
        return false;
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
                    let is_diff = matches!(tab, crate::state::layout::TabKind::Diff(_));
                    let is_preview = tab.as_file().is_some_and(|f| f.preview);
                    let is_read_only = tab.is_read_only();
                    let label = if tab.is_dirty() {
                        format!("{}  {}", icons::CIRCLE, tab.name())
                    } else if is_diff {
                        format!("{}  {}", icons::GIT_DIFF, tab.name())
                    } else if is_read_only {
                        format!("{}  {}", icons::LOCK, tab.name())
                    } else {
                        tab.name().to_string()
                    };
                    let (clicked, close_clicked) = draw_file_tab(ui, &label, is_active, is_diff, is_preview, is_read_only, idx);
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
        if pane.tabs.get(idx).map(|t| t.is_dirty()).unwrap_or(false) {
            pane.pending_close = Some(idx);
        } else {
            pane.close(idx);
            if pane.tabs.is_empty() {
                return true;
            }
        }
    }
    render_close_confirm(ui, pane);
    if pane.tabs.is_empty() {
        return true;
    }
    ui.add_space(2.0);

    let active_idx = pane.active.min(pane.tabs.len() - 1);
    pane.active = active_idx;

    // Dispatch based on active tab kind: file editor vs diff view.
    let active_is_file = matches!(&pane.tabs[active_idx], crate::state::layout::TabKind::File(_));
    let active_is_diff = matches!(&pane.tabs[active_idx], crate::state::layout::TabKind::Diff(_));
    let active_read_only = pane.tabs.get(active_idx).map(|t| t.is_read_only()).unwrap_or(false);

    // Save shortcut (blocked for read-only files)
    let save_pressed = active_is_file && !active_read_only && ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::S)
    });
    // Cmd+F opens the find bar (or replaces the query with the current
    // selection). Esc closes it — Cmd+F never closes.
    let find_toggle = active_is_file && !active_read_only && ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::F)
    });
    if find_toggle {
        let t = pane.tabs[active_idx].as_file_mut().unwrap();
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
    // Cmd+H toggles the replace row inside the find bar.
    let replace_toggle = active_is_file && ui.input_mut(|i| {
        let pressed = (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::H);
        if pressed {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::H);
            i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::H);
        }
        pressed
    });
    if replace_toggle {
        let t = pane.tabs[active_idx].as_file_mut().unwrap();
        if t.find_query.is_none() {
            t.find_query = Some(String::new());
        }
        t.show_replace = !t.show_replace;
    }

    if active_is_diff {
        if let Some(dt) = pane.tabs[active_idx].as_diff_mut() {
            *title = format!("Files · {}", dt.title);
            crate::views::diff_view::render_diff_body(ui, dt, font_size, active_idx);
        }
        return false;
    }

    {
        let tab = pane.tabs[active_idx].as_file_mut().unwrap();
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

        // Save error banner
        if let Some(err) = &tab.save_error {
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
                                "{}  Save failed: {err}",
                                icons::WARNING
                            ))
                            .size(11.5)
                            .color(t.text.to_color32()),
                        );
                    });
                });
            ui.add_space(4.0);
        }

        // PDF viewer: short-circuit the entire text-editor body. The
        // path row above shows the breadcrumb; the PDF view paints its
        // own toolbar (page nav + zoom + Open Externally) and the
        // document scroll area below.
        if crate::views::pdf_view::is_pdf_path(&tab.path) {
            if tab.pdf_state.is_none() {
                tab.pdf_state = Some(Box::new(
                    crate::views::pdf_view::PdfTabState::new(
                        std::path::PathBuf::from(&tab.path),
                    ),
                ));
            }
            if let Some(state) = tab.pdf_state.as_mut() {
                crate::views::pdf_view::render_pdf(ui, state);
            }
            return false;
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
                    if tab.read_only {
                        let unlock_btn = ui.add(
                            egui::Button::new(
                                RichText::new(format!("{}  Unlock", icons::LOCK_OPEN))
                                    .size(11.5),
                            )
                            .min_size(egui::vec2(0.0, 24.0)),
                        );
                        if unlock_btn.clicked() {
                            tab.read_only = false;
                            tab.save_error = None;
                            tab.original_content = tab.content.clone();
                            tab.disk_mtime = std::fs::metadata(&tab.path)
                                .and_then(|m| m.modified())
                                .ok();
                        }
                    } else {
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
            replace: find_replace,
            replace_all: find_replace_all,
        } = crate::views::file_find::render_find_bar(ui, tab);
        if find_close {
            tab.find_query = None;
        }

        // Replace at cursor
        if find_replace && let Some(q) = tab.find_query.clone() && !q.is_empty() {
            let te_id = egui::Id::new(("file_editor", &tab.path)).with("body");
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                let cursor = state.cursor.char_range()
                    .map(|r| r.primary.index).unwrap_or(0);
                let byte = crate::format::char_idx_to_byte(&tab.content, cursor);
                let after = byte + 1.min(tab.content.len().saturating_sub(byte));
                let found = tab.content[after..].find(&q)
                    .map(|p| after + p)
                    .or_else(|| tab.content.find(&q));
                if let Some(match_byte) = found {
                    tab.content.replace_range(match_byte..match_byte + q.len(), &tab.replace_query);
                    let new_pos = tab.content[..match_byte].chars().count() + tab.replace_query.chars().count();
                    let (line, _) = char_idx_to_line_col(&tab.content, new_pos);
                    let new_cc = egui::text::CCursor::new(new_pos);
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(new_cc)));
                    state.store(ui.ctx(), te_id);
                    tab.find_scroll_to_line = Some(line);
                }
            }
        }
        // Replace all
        if find_replace_all && let Some(q) = tab.find_query.clone() && !q.is_empty() {
            let count = tab.content.matches(&q).count();
            if count > 0 {
                tab.content = tab.content.replace(&q, &tab.replace_query);
            }
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
                let after = cur_byte + 1.min(tab.content.len().saturating_sub(cur_byte));
                tab.content[after..]
                    .find(&q)
                    .map(|p| after + p)
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

        // Go-to-line modal (Ctrl+G)
        if tab.goto_line_active {
            let t = theme::current();
            egui::Frame::NONE
                .fill(t.bg.to_color32())
                .stroke(egui::Stroke::new(1.0, t.border.to_color32()))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Go to Line:")
                                .size(11.5)
                                .color(t.text.to_color32()),
                        );
                        let line_count = tab.content.split('\n').count();
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut tab.goto_line_input)
                                .desired_width(80.0)
                                .font(egui::FontId::new(12.0, FontFamily::Monospace)),
                        );
                        let goto_focus_id = egui::Id::new(("goto_focused", &tab.path));
                        let needs_focus = !ui.memory(|m| m.data.get_temp::<bool>(goto_focus_id).unwrap_or(false));
                        if needs_focus {
                            response.request_focus();
                            ui.memory_mut(|m| m.data.insert_temp(goto_focus_id, true));
                        }
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if enter {
                            if let Ok(line_num) = tab.goto_line_input.trim().parse::<u32>() {
                                let target = if line_count <= 1 {
                                    0u32
                                } else {
                                    line_num.saturating_sub(1).min(line_count as u32 - 1)
                                };
                                tab.pending_cursor = Some((target, 0));
                                tab.find_scroll_to_line = Some(target);
                            }
                            tab.goto_line_active = false;
                            tab.goto_line_input.clear();
                            ui.memory_mut(|m| m.data.remove_temp::<bool>(goto_focus_id));
                        }
                        if escape {
                            tab.goto_line_active = false;
                            tab.goto_line_input.clear();
                            ui.memory_mut(|m| m.data.remove_temp::<bool>(goto_focus_id));
                        }
                    });
                });
            ui.add_space(2.0);
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
            return false;
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
            return false;
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
                        if focused && !tab.read_only {
                            // Cmd+Shift+Z → redo is already handled by egui
                            // 0.34's TextEdit (checked in its builder.rs —
                            // matches Modifiers::SHIFT | COMMAND with Z).
                            // Don't intercept; consuming the event here
                            // actually *prevents* the native redo from
                            // seeing it.
                            let (tab_pressed, enter_pressed, shift_tab_pressed, cmd_slash_pressed, ctrl_g_pressed) = ui.input_mut(|i| {
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
                                let st = i.key_pressed(egui::Key::Tab)
                                    && i.modifiers.shift
                                    && !i.modifiers.command
                                    && !i.modifiers.mac_cmd;
                                if st {
                                    i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
                                }
                                let cs = (i.modifiers.command || i.modifiers.mac_cmd)
                                    && i.key_pressed(egui::Key::Slash);
                                if cs {
                                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash);
                                    i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::Slash);
                                }
                                let cg = i.modifiers.ctrl && !i.modifiers.shift
                                    && !i.modifiers.command && !i.modifiers.mac_cmd
                                    && i.key_pressed(egui::Key::G);
                                if cg {
                                    i.consume_key(egui::Modifiers::CTRL, egui::Key::G);
                                }
                                (t, e, st, cs, cg)
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

                            // Shift+Tab: outdent — remove one indent level
                            if shift_tab_pressed
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
                                let bytes = tab.content.as_bytes();
                                let line_start = bytes[..byte]
                                    .iter()
                                    .rposition(|b| *b == b'\n')
                                    .map(|i| i + 1)
                                    .unwrap_or(0);
                                let line_prefix = tab.content[line_start..byte].to_string();
                                let removed = if line_prefix.starts_with(&indent) {
                                    indent.len()
                                } else if line_prefix.starts_with('\t') {
                                    1
                                } else {
                                    let spaces = line_prefix.len()
                                        - line_prefix.trim_start_matches(' ').len();
                                    spaces.min(indent.len())
                                };
                                if removed > 0 {
                                    tab.content.replace_range(line_start..line_start + removed, "");
                                    let line_start_char = tab.content[..line_start].chars().count();
                                    let new_cc = egui::text::CCursor::new(
                                        line_start_char + line_prefix[removed..].chars().count(),
                                    );
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::one(new_cc),
                                    ));
                                    state.store(ui.ctx(), te_id);
                                }
                            }

                            // Helper: strip one indent level from a string
                            fn remove_one_indent(s: &str, max_spaces: usize) -> String {
                                let chars: Vec<char> = s.chars().collect();
                                let mut i = 0;
                                while i < chars.len() && i < max_spaces && chars[i] == ' ' {
                                    i += 1;
                                }
                                if i == 0 && chars.first() == Some(&'\t') {
                                    i = 1;
                                }
                                chars[i..].iter().collect()
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
                                let (prev_indent, bump, dedent) =
                                    crate::format::auto_indent_context(&tab.content, byte);
                                let next_is_close = tab
                                    .content
                                    .as_bytes()
                                    .get(byte)
                                    .map(|c| matches!(c, b'}' | b')' | b']'))
                                    .unwrap_or(false);
                                let dedented_indent = remove_one_indent(&prev_indent, indent.chars().count());
                                let body_indent = if bump {
                                    format!("{prev_indent}{indent}")
                                } else if dedent && next_is_close {
                                    // e.g. cursor between } and ) — keep at brace level
                                    prev_indent.clone()
                                } else if dedent {
                                    dedented_indent
                                } else {
                                    prev_indent.clone()
                                };
                                let inserted = if bump && next_is_close {
                                    format!("\n{body_indent}\n{prev_indent}")
                                } else if dedent && next_is_close {
                                    format!("\n{body_indent}\n{prev_indent}")
                                } else {
                                    format!("\n{body_indent}")
                                };
                                tab.content.insert_str(byte, &inserted);
                                let advance = if bump && next_is_close {
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

                            // Bracket auto-close: typing an opener inserts the
                            // pair and places the cursor between them.
                            // Typing a closer when it's already the next char
                            // just moves the cursor past it (skip-over).
                            {
                                let pairs: &[(&str, &str)] = &[
                                    ("{", "}"), ("(", ")"), ("[", "]"),
                                ];
                                let typed_event = ui.input_mut(|i| {
                                    i.events.iter().position(|e| {
                                        matches!(e, egui::Event::Text(t) if pairs.iter().any(|(o, _)| *o == t.as_str()))
                                    })
                                });
                                if let Some(ev_idx) = typed_event {
                                    let typed = match &ui.input(|i| i.events[ev_idx].clone()) {
                                        egui::Event::Text(t) => t.clone(),
                                        _ => unreachable!(),
                                    };
                                    if let Some(mut state) =
                                        egui::TextEdit::load_state(ui.ctx(), te_id)
                                    {
                                        let cursor = state
                                            .cursor.char_range()
                                            .map(|r| r.primary.index)
                                            .unwrap_or(0);
                                        let byte =
                                            crate::format::char_idx_to_byte(&tab.content, cursor);
                                        let (open, close) = pairs.iter().find(|(o, _)| *o == typed.as_str()).unwrap();
                                        // Skip-over: if the next char is the same closing
                                        // bracket, just advance the cursor.
                                        let next_char = tab.content[byte..].chars().next();
                                        if next_char == Some(close.chars().next().unwrap())
                                            && open != close
                                        {
                                            let new_cc = egui::text::CCursor::new(cursor + 1);
                                            state.cursor.set_char_range(Some(
                                                egui::text::CCursorRange::one(new_cc),
                                            ));
                                            state.store(ui.ctx(), te_id);
                                        } else {
                                            // Insert pair
                                            tab.content.insert_str(byte, &format!("{open}{close}"));
                                            let new_cc = egui::text::CCursor::new(cursor + 1);
                                            state.cursor.set_char_range(Some(
                                                egui::text::CCursorRange::one(new_cc),
                                            ));
                                            state.store(ui.ctx(), te_id);
                                        }
                                        // Remove the event so TextEdit doesn't also type it
                                        ui.input_mut(|i| { i.events.remove(ev_idx); });
                                    }
                                }
                            }

                            // Alt+Up/Down: move current line (or selection) up/down.
                            // Alt+Shift+Down: duplicate line down.
                            {
                                let alt_up = ui.input_mut(|i| {
                                    let pressed = i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp)
                                        && !i.modifiers.shift && !i.modifiers.command && !i.modifiers.mac_cmd;
                                    if pressed { i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp); }
                                    pressed
                                });
                                let alt_down = ui.input_mut(|i| {
                                    let pressed = i.modifiers.alt && i.key_pressed(egui::Key::ArrowDown)
                                        && !i.modifiers.shift && !i.modifiers.command && !i.modifiers.mac_cmd;
                                    if pressed { i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown); }
                                    pressed
                                });
                                let alt_shift_down = ui.input_mut(|i| {
                                    let pressed = i.modifiers.alt && i.modifiers.shift && i.key_pressed(egui::Key::ArrowDown)
                                        && !i.modifiers.command && !i.modifiers.mac_cmd;
                                    if pressed { i.consume_key(egui::Modifiers::ALT | egui::Modifiers::SHIFT, egui::Key::ArrowDown); }
                                    pressed
                                });
                                if alt_up || alt_down {
                                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                                        let range = state.cursor.char_range().unwrap_or_else(||
                                            egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                                        let sel_start = range.primary.index.min(range.secondary.index);
                                        let sel_end = range.primary.index.max(range.secondary.index);
                                        let bytes = tab.content.as_bytes();
                                        let start_byte = crate::format::char_idx_to_byte(&tab.content, sel_start);
                                        let end_byte = crate::format::char_idx_to_byte(&tab.content, sel_end);
                                        let line_start = bytes[..start_byte]
                                            .iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                                        let line_end = bytes[end_byte..]
                                            .iter().position(|b| *b == b'\n')
                                            .map(|i| end_byte + i + 1)
                                            .unwrap_or(tab.content.len());
                                        if alt_down {
                                            if line_end < tab.content.len() {
                                                let next_end = bytes[line_end..]
                                                    .iter().position(|b| *b == b'\n')
                                                    .map(|i| line_end + i + 1)
                                                    .unwrap_or(tab.content.len());
                                                let swapped = format!("{}{}",
                                                    &tab.content[line_end..next_end],
                                                    &tab.content[line_start..line_end]);
                                                tab.content.replace_range(line_start..next_end, &swapped);
                                                let new_cc = egui::text::CCursor::new(
                                                    sel_start + (next_end - line_end) as usize
                                                );
                                                state.cursor.set_char_range(Some(
                                                    egui::text::CCursorRange::one(new_cc),
                                                ));
                                                state.store(ui.ctx(), te_id);
                                            }
                                        } else {
                                            if line_start > 0 {
                                                let prev_start = bytes[..line_start.saturating_sub(1)]
                                                    .iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                                                let prev_len = line_start - prev_start;
                                                tab.content.replace_range(prev_start..line_end, &format!(
                                                    "{}{}",
                                                    &tab.content[line_start..line_end],
                                                    &tab.content[prev_start..line_start],
                                                ));
                                                let new_cc = egui::text::CCursor::new(sel_start.saturating_sub(prev_len));
                                                state.cursor.set_char_range(Some(
                                                    egui::text::CCursorRange::one(new_cc),
                                                ));
                                                state.store(ui.ctx(), te_id);
                                            }
                                        }
                                    }
                                }
                                // Alt+Shift+Down: duplicate line
                                if alt_shift_down {
                                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                                        let cursor = state.cursor.char_range()
                                            .map(|r| r.primary.index).unwrap_or(0);
                                        let byte = crate::format::char_idx_to_byte(&tab.content, cursor);
                                        let bytes = tab.content.as_bytes();
                                        let line_start = bytes[..byte]
                                            .iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                                        let line_end = bytes[byte..]
                                            .iter().position(|b| *b == b'\n')
                                            .map(|i| byte + i + 1)
                                            .unwrap_or(tab.content.len());
                                        let line_text = tab.content[line_start..line_end].to_string();
                                        tab.content.insert_str(line_end, &format!("\n{line_text}"));
                                        let new_cc = egui::text::CCursor::new(
                                            cursor + 1 + line_text.chars().count(),
                                        );
                                        state.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(new_cc),
                                        ));
                                        state.store(ui.ctx(), te_id);
                                    }
                                }
                            }

                            // Cmd+/: toggle line comment
                            if cmd_slash_pressed
                                && let Some(state) =
                                    egui::TextEdit::load_state(ui.ctx(), te_id)
                            {
                                let range = state.cursor.char_range().unwrap_or_else(||
                                    egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                                let sel_start = range.primary.index.min(range.secondary.index);
                                let sel_end = range.primary.index.max(range.secondary.index);
                                let comment_str = crate::views::file_util::comment_prefix(&tab.path);
                                toggle_line_comments(
                                    &mut tab.content, sel_start, sel_end, &comment_str,
                                );
                                state.store(ui.ctx(), te_id);
                            }

                            // Ctrl+G: go to line
                            if ctrl_g_pressed {
                                tab.goto_line_active = true;
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
                            .interactive(!tab.read_only)
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

                        // Cmd+hover: underline the token under the pointer
                        // and switch to hand cursor (goto-definition hint).
                        let cmd_held = ui.input(|i| {
                            i.modifiers.command || i.modifiers.mac_cmd
                        });
                        if cmd_held && out.response.hovered() {
                            if let Some(ptr) = ui.input(|i| i.pointer.latest_pos()) {
                                let rel = egui::vec2(
                                    ptr.x - out.galley_pos.x,
                                    ptr.y - out.galley_pos.y,
                                );
                                let ccursor = out.galley.cursor_from_pos(rel);
                                let idx = ccursor.index;
                                let chars: Vec<char> = tab.content.chars().collect();
                                if idx > 0 && idx <= chars.len() {
                                    let ch = chars[idx - 1];
                                    if ch.is_alphanumeric() || ch == '_' {
                                        let mut start = idx - 1;
                                        while start > 0
                                            && (chars[start - 1].is_alphanumeric()
                                                || chars[start - 1] == '_')
                                        {
                                            start -= 1;
                                        }
                                        let mut end = idx;
                                        while end < chars.len()
                                            && (chars[end].is_alphanumeric()
                                                || chars[end] == '_')
                                        {
                                            end += 1;
                                        }
                                        let p0 = out
                                            .galley
                                            .pos_from_cursor(
                                                egui::text::CCursor::new(start),
                                            );
                                        let p1 = out
                                            .galley
                                            .pos_from_cursor(
                                                egui::text::CCursor::new(end),
                                            );
                                        let y = p1.max.y + 1.5;
                                        ui.painter().line_segment(
                                            [
                                                egui::pos2(
                                                    out.galley_pos.x + p0.min.x,
                                                    out.galley_pos.y + y,
                                                ),
                                                egui::pos2(
                                                    out.galley_pos.x + p1.max.x,
                                                    out.galley_pos.y + y,
                                                ),
                                            ],
                                            (1.5, theme::current().accent.to_color32()),
                                        );
                                    }
                                }
                            }
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

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
                            let sel_start = range.primary.index.min(range.secondary.index);
                            let sel_end = range.primary.index.max(range.secondary.index);
                            if sel_start != sel_end {
                                let start_byte = crate::format::char_idx_to_byte(&tab.content, sel_start);
                                let end_byte = crate::format::char_idx_to_byte(&tab.content, sel_end);
                                let sel_chars = tab.content[start_byte..end_byte].chars().count();
                                let sel_lines = tab.content[start_byte..end_byte].matches('\n').count() + 1;
                                tab.selection_info = Some((sel_chars, sel_lines));
                            } else {
                                tab.selection_info = None;
                            }
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
                            let cursor = line_col_to_char(&tab.content, line, ch);
                            let new_cc = egui::text::CCursor::new(cursor);
                            state.cursor.set_char_range(Some(
                                egui::text::CCursorRange::one(new_cc),
                            ));
                            state.store(ui.ctx(), te_id);
                            ui.memory_mut(|m| m.request_focus(te_id));
                            // Scroll the editor viewport to the target line.
                            tab.find_scroll_to_line = Some(line);
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
                        // Cmd+C on empty selection copies the whole line.
                        // macOS: egui synthesizes Event::Copy from Cmd+C.
                        // Only consume the event when the selection is empty so
                        // normal selected-text copy still works.
                        let has_copy_event = ui.memory(|m| m.has_focus(te_id))
                            && ui.input(|i| {
                                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                            });
                        if has_copy_event
                            && let Some(state) = egui::TextEdit::load_state(ui.ctx(), te_id)
                        {
                            let range = state.cursor.char_range().unwrap_or_else(||
                                egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                            let empty = range.primary.index == range.secondary.index;
                            if empty {
                                ui.input_mut(|i| {
                                    if let Some(idx) = i.events.iter().position(|e| {
                                        matches!(e, egui::Event::Copy)
                                    }) {
                                        i.events.remove(idx);
                                    }
                                });
                                let cursor = range.primary.index;
                                let byte = crate::format::char_idx_to_byte(&tab.content, cursor);
                                let bytes = tab.content.as_bytes();
                                let line_start = bytes[..byte]
                                    .iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                                let line_end = bytes[byte..]
                                    .iter().position(|b| *b == b'\n')
                                    .map(|i| byte + i + 1)
                                    .unwrap_or(bytes.len());
                                let line = tab.content[line_start..line_end].to_string();
                                if !line.is_empty() {
                                    ui.ctx().copy_text(line);
                                }
                            }
                            // else: leave Copy event in queue for TextEdit to handle
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
                            if tab.read_only {
                                if ui.button(format!("{}  Unlock for Editing", icons::LOCK_OPEN)).clicked() {
                                    tab.read_only = false;
                                    tab.save_error = None;
                                    tab.original_content = tab.content.clone();
                                    tab.disk_mtime = std::fs::metadata(&tab.path)
                                        .and_then(|m| m.modified())
                                        .ok();
                                    ui.close();
                                }
                            } else {
                                if ui.button(format!("{}  Save", icons::FLOPPY_DISK)).clicked() {
                                    cs.set(true);
                                    ui.close();
                                }
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

        // Current line highlight: subtle background on the line with the
        // primary cursor. Painted over the code area (not gutter).
        let cur_line_num = char_idx_to_line_col(&tab.content, tab.last_cursor_idx).0;
        let code_hl_y = avail.min.y + gutter_w + code_pad + (cur_line_num as f32 + 0.5) * row_h - v_offset;
        let code_area_left = code_left + code_pad;
        let code_area_right = avail.min.x + avail.width();
        if code_hl_y >= avail.min.y - row_h && code_hl_y <= avail.max.y + row_h {
            let bg = theme::current().bg;
            let hl_color = if bg.r as u32 + bg.g as u32 + bg.b as u32 > 128 * 3 {
                Color32::from_rgba_unmultiplied(0, 0, 0, 18)
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, 18)
            };
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(code_area_left, code_hl_y - row_h * 0.5),
                    egui::vec2(code_area_right - code_area_left, row_h),
                ),
                0.0,
                hl_color,
            );
        }
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

        // Promote preview tab to permanent if content changed.
        if tab.preview && tab.dirty() {
            tab.preview = false;
        }
    }
    false
}




fn draw_file_tab(
    ui: &mut egui::Ui,
    name: &str,
    is_active: bool,
    is_diff: bool,
    is_preview: bool,
    is_read_only: bool,
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
    let height = 26.0;
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
    let show_close = is_active || response.hovered() || close_rect_contains(&response, ui);
    let close_response = if show_close {
        Some(ui.interact(
            close_rect,
            ui.id().with(("file_tab_close", idx)),
            egui::Sense::click(),
        ))
    } else {
        None
    };

    let t = theme::current();
    let accent_tint = {
        let a = t.accent;
        Color32::from_rgba_unmultiplied(a.r, a.g, a.b, 55)
    };
    let diff_tint = Color32::from_rgba_unmultiplied(100, 180, 255, 30);
    let readonly_tint = Color32::from_rgba_unmultiplied(220, 80, 80, 35);

    let (bg, fg) = if is_active {
        (accent_tint, t.text.to_color32())
    } else if response.hovered() || close_response.as_ref().is_some_and(|r| r.hovered()) {
        (t.row_hover.to_color32(), t.text.to_color32())
    } else {
        (egui::Color32::TRANSPARENT, t.text_muted.to_color32())
    };

    // Paint tab background
    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, bg);
    }

    // Diff tab: subtle blue tint overlay
    if is_diff && !is_active && bg == egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, diff_tint);
    }

    // Read-only tab: subtle red tint overlay
    if is_read_only && !is_active && bg == egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, readonly_tint);
    } else if is_read_only && is_active {
        // Active read-only: blend red into the accent background
        ui.painter().rect_filled(rect, 5.0, readonly_tint);
    }

    // Bottom border on active tab (solid accent) and inactive tabs (subtle)
    if is_active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 4.0, rect.max.y - 2.0),
                egui::vec2(rect.width() - 8.0, 2.0),
            ),
            1.0,
            t.accent.to_color32(),
        );
    } else {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 4.0, rect.max.y - 1.0),
                egui::vec2(rect.width() - 8.0, 1.0),
            ),
            0.0,
            t.border.to_color32(),
        );
    }

    // Tab label (preview tabs use dimmed color)
    let label_fg = if (is_preview || is_read_only) && !is_active {
        t.text_muted.to_color32()
    } else {
        fg
    };
    ui.painter().text(
        egui::pos2(rect.min.x + padding_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        font,
        label_fg,
    );

    // Close button (only visible when appropriate)
    if let Some(cr) = &close_response {
        if cr.hovered() {
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
    }

    if response.hovered() || close_response.as_ref().is_some_and(|r| r.hovered()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let closed = close_response.as_ref().is_some_and(|r| r.clicked()) || response.middle_clicked();
    (
        response.clicked() && !close_response.as_ref().is_some_and(|r| r.hovered()),
        closed,
    )
}

/// Check if the pointer is over the close rect area (for hover detection
/// when the close button isn't rendered yet).
fn close_rect_contains(response: &egui::Response, ui: &egui::Ui) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let Some(pos) = pointer else { return false };
    let rect = response.rect;
    let padding_x = 10.0;
    let close_size = 16.0;
    let height = rect.height();
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.max.x - padding_x - close_size + 2.0,
            rect.min.y + (height - close_size) / 2.0,
        ),
        egui::vec2(close_size, close_size),
    );
    close_rect.contains(pos)
}

use crate::state::layout::DiffTabData;
use crate::theme;
use crate::views::file_util::is_image_path;
use crate::views::file_view::{find_syntax_for_ext, syntaxes, themes};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, RichText, ScrollArea};
use egui_phosphor::regular as icons;
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, Theme as SynTheme};

const ADD_BG: Color32 = Color32::from_rgb(25, 55, 35);
const DEL_BG: Color32 = Color32::from_rgb(60, 28, 32);
const CTX_FG: Color32 = Color32::from_rgb(180, 186, 198);
const ADD_FG: Color32 = Color32::from_rgb(140, 220, 150);
const DEL_FG: Color32 = Color32::from_rgb(230, 130, 130);
const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
const MINIMAP_W: f32 = 10.0;

struct Row {
    tag: ChangeTag,
    old_ln: String,
    new_ln: String,
    content: String,
}

pub fn render_diff_body(
    ui: &mut egui::Ui,
    tab: &mut DiffTabData,
    font_size: f32,
    _tab_index: usize,
) {
    let is_image = is_image_path(&tab.right_path);
    let left_path = tab.left_path.clone();
    let right_path = tab.right_path.clone();

    if is_image {
        render_image_block(ui, tab, &left_path, &right_path, _tab_index);
        return;
    }

    let diff = TextDiff::from_lines(&tab.left_text, &tab.right_text);
    let font = FontId::new(font_size, FontFamily::Monospace);
    let left_lines_count = tab.left_text.lines().count().max(1);
    let right_lines_count = tab.right_text.lines().count().max(1);

    let syntax = resolve_syntax(&tab.right_path);
    let (ss, st_theme) = resolve_theme();
    let ldigits = left_lines_count.to_string().len().max(3);
    let rdigits = right_lines_count.to_string().len().max(3);
    let char_w = measure_char_w(ui, &font);

    let rows: Vec<Row> = diff
        .iter_all_changes()
        .map(|c| Row {
            tag: c.tag(),
            old_ln: c
                .old_index()
                .map(|i| format!("{:>w$}", i + 1, w = ldigits))
                .unwrap_or_else(|| " ".repeat(ldigits)),
            new_ln: c
                .new_index()
                .map(|i| format!("{:>w$}", i + 1, w = rdigits))
                .unwrap_or_else(|| " ".repeat(rdigits)),
            content: c.value().trim_end_matches('\n').to_string(),
        })
        .collect();

    let tags: Vec<ChangeTag> = rows.iter().map(|r| r.tag).collect();
    let total_rows = tags.len().max(1);

    let hunk_starts = {
        let mut starts = Vec::new();
        let mut in_hunk = false;
        for (i, tag) in tags.iter().enumerate() {
            let changed = !matches!(tag, ChangeTag::Equal);
            if changed && !in_hunk {
                starts.push(i);
            }
            in_hunk = changed;
        }
        starts
    };

    // Compute hunk patches for per-hunk staging. Uses `git diff` to
    // generate proper unified-diff patches that `git apply --cached`
    // can consume. The hunk indices align with `hunk_starts` because
    // both `similar` and `git diff` operate on the same file content.
    let hunk_patches: Vec<Option<String>> = if let Some(repo) = &tab.repo_path {
        let repo_path = std::path::Path::new(repo);
        let rel = tab.right_path.as_str();
        if let Some(raw) = crate::git::file_diff_raw(repo_path, rel) {
            let parsed = crate::git::parse_hunks(&raw);
            hunk_starts
                .iter()
                .enumerate()
                .map(|(hi, _)| parsed.get(hi).map(|(_, patch)| patch.clone()))
                .collect()
        } else {
            vec![None; hunk_starts.len()]
        }
    } else {
        vec![None; hunk_starts.len()]
    };

    // Flag: set when a hunk is staged, triggering a full diff refresh
    // next frame. Stored in egui data keyed to the diff tab.
    let refresh_id = egui::Id::new((
        "diff_hunk_staged",
        tab.left_path.clone(),
        tab.right_path.clone(),
    ));
    let _needs_refresh = ui.ctx().data(|d| d.get_temp::<bool>(refresh_id)).unwrap_or(false);

    let hunk_state_id = egui::Id::new((
        "diff_hunk_idx",
        tab.left_path.clone(),
        tab.right_path.clone(),
    ));
    let mut hunk_idx: Option<usize> = ui
        .ctx()
        .data(|d| d.get_temp::<Option<usize>>(hunk_state_id))
        .unwrap_or(None);
    let mut jump_to_row: Option<usize> = None;

    // -- Header --
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        // Strip "staged:" / "HEAD:" prefix to get the bare path for comparison
        let left_bare = left_path.strip_prefix("staged:").or_else(|| left_path.strip_prefix("HEAD:")).unwrap_or(&left_path);
        let right_bare = right_path.strip_prefix("staged:").or_else(|| right_path.strip_prefix("HEAD:")).unwrap_or(&right_path);
        if left_bare == right_bare {
            let display = std::path::Path::new(left_bare)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(left_bare);
            ui.label(
                RichText::new(display)
                    .size(11.0)
                    .color(ADD_FG)
                    .monospace(),
            );
        } else {
            ui.label(
                RichText::new(&left_path)
                    .size(11.0)
                    .color(DEL_FG)
                    .monospace(),
            );
            ui.label(RichText::new(" -> ").size(11.0).color(MUTED));
            ui.label(
                RichText::new(&right_path)
                    .size(11.0)
                    .color(ADD_FG)
                    .monospace(),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let nav_enabled = !hunk_starts.is_empty();
            let down = ui.add_enabled(
                nav_enabled,
                egui::Button::new(RichText::new(icons::ARROW_DOWN).size(12.0))
                    .min_size(egui::vec2(22.0, 22.0)),
            );
            if down.clicked() {
                let next = match hunk_idx {
                    None => 0,
                    Some(n) => (n + 1).min(hunk_starts.len().saturating_sub(1)),
                };
                hunk_idx = Some(next);
                if let Some(&row) = hunk_starts.get(next) {
                    jump_to_row = Some(row);
                }
            }
            let up = ui.add_enabled(
                nav_enabled,
                egui::Button::new(RichText::new(icons::ARROW_UP).size(12.0))
                    .min_size(egui::vec2(22.0, 22.0)),
            );
            if up.clicked() {
                let prev = match hunk_idx {
                    None => 0,
                    Some(n) => n.saturating_sub(1),
                };
                hunk_idx = Some(prev);
                if let Some(&row) = hunk_starts.get(prev) {
                    jump_to_row = Some(row);
                }
            }
            if !hunk_starts.is_empty() {
                let label = match hunk_idx {
                    Some(n) => format!("{} / {}", n + 1, hunk_starts.len()),
                    None => format!("- / {}", hunk_starts.len()),
                };
                ui.add_space(6.0);
                ui.label(RichText::new(label).size(11.0).color(MUTED).monospace());
            }
        });
    });
    ui.add_space(4.0);
    ui.separator();

    // -- Scroll body --
    let row_h = (font_size * 1.25).ceil();
    let total_body_h = total_rows as f32 * row_h;
    let body_rect = ui.available_rect_before_wrap();
    let jump_y: Option<f32> = jump_to_row.map(|r| (r as f32 * row_h - row_h * 2.0).max(0.0));

    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body_rect));
    body_ui.spacing_mut().item_spacing.y = 0.0;

    let mut scroll = ScrollArea::both()
        .auto_shrink([false; 2])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    if let Some(y) = jump_y {
        scroll = scroll.vertical_scroll_offset(y);
    }

    // Build a lookup: row index -> which hunk it belongs to (for stage buttons)
    let row_to_hunk: Vec<Option<usize>> = {
        let mut map = vec![None; total_rows];
        for (hi, &start) in hunk_starts.iter().enumerate() {
            if let Some(end) = hunk_starts.get(hi + 1) {
                for r in start..*end {
                    if r < map.len() {
                        map[r] = Some(hi);
                    }
                }
            } else {
                for r in start..total_rows {
                    map[r] = Some(hi);
                }
            }
        }
        map
    };

    let gutter_old_w = char_w * ldigits as f32 + 10.0;
    let gutter_new_w = char_w * rdigits as f32 + 10.0;
    let sign_w = char_w * 2.0 + 8.0;
    let stage_btn_w = 20.0;

    let scroll_out = scroll.show_rows(&mut body_ui, row_h, rows.len(), |ui, row_range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in row_range {
            let r = &rows[i];
            let (sign, sign_fg, bg) = match r.tag {
                ChangeTag::Delete => ("-", DEL_FG, DEL_BG),
                ChangeTag::Insert => ("+", ADD_FG, ADD_BG),
                ChangeTag::Equal => (" ", CTX_FG, Color32::TRANSPARENT),
            };
            // Stage button at hunk start -- register interaction early,
            // paint after row background so the button isn't covered.
            let is_hunk_start = hunk_starts.contains(&i);
            let mut stage_btn_paint: Option<(egui::Rect, bool)> = None;
            if is_hunk_start && let Some(hi) = row_to_hunk[i] {
                if let Some(patch) = &hunk_patches[hi] {
                    let btn_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(stage_btn_w, row_h),
                    );
                    let btn_id = egui::Id::new(("stage_hunk", tab.left_path.clone(), tab.right_path.clone(), hi));
                    let btn_resp = ui.interact(btn_rect, btn_id, egui::Sense::click());
                    let btn_hovered = btn_resp.hovered();
                    let btn_clicked = btn_resp.clicked();
                    if btn_hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    btn_resp.on_hover_text("Stage hunk");
                    if btn_clicked {
                        if let Some(repo) = &tab.repo_path {
                            let repo_path = std::path::Path::new(repo);
                            match crate::git::stage_hunk(repo_path, patch) {
                                Ok(()) => {
                                    tab.pending_hunk_stage = true;
                                    ui.ctx().data_mut(|d| d.insert_temp(refresh_id, true));
                                }
                                Err(e) => {
                                    tab.error = Some(format!("Stage hunk failed: {e}"));
                                }
                            }
                        }
                    }
                    stage_btn_paint = Some((btn_rect, btn_hovered));
                }
            }
            let mut hl = HighlightLines::new(syntax, st_theme);
            let segments: Vec<(SynStyle, String)> = hl
                .highlight_line(&format!("{}\n", r.content), ss)
                .map(|v| {
                    v.into_iter()
                        .map(|(s, t)| (s, t.trim_end_matches('\n').to_string()))
                        .collect()
                })
                .unwrap_or_else(|_| vec![(SynStyle::default(), r.content.clone())]);
            let total_w = gutter_old_w + gutter_new_w + sign_w
                + {
                    let mut job = LayoutJob::default();
                    for (style, text) in &segments {
                        let c = style.foreground;
                        let color = if c.a == 0 { CTX_FG } else { Color32::from_rgb(c.r, c.g, c.b) };
                        job.append(text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
                    }
                    ui.fonts_mut(|f| f.layout_job(job)).size().x
                } + stage_btn_w + 8.0;
            let (rect, _resp) = ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
            let painter = ui.painter();
            if bg != Color32::TRANSPARENT {
                // Exclude stage button area from background fill
                let bg_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + stage_btn_w, rect.min.y),
                    rect.max,
                );
                painter.rect_filled(bg_rect, 0.0, bg);
            }
            // Paint stage button on top of row background
            if let Some((btn_rect, hovered)) = &stage_btn_paint {
                if *hovered {
                    painter.rect_filled(*btn_rect, 2.0, theme::current().row_hover.to_color32());
                }
                painter.text(
                    btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::PLUS_CIRCLE,
                    FontId::new(11.0, FontFamily::Proportional),
                    ADD_FG,
                );
            }
            let gx = rect.min.x + stage_btn_w;
            painter.text(
                Pos2::new(gx + gutter_old_w - 4.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                &r.old_ln, font.clone(), MUTED,
            );
            painter.text(
                Pos2::new(gx + gutter_old_w + gutter_new_w - 4.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                &r.new_ln, font.clone(), MUTED,
            );
            painter.text(
                Pos2::new(gx + gutter_old_w + gutter_new_w + sign_w / 2.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                sign, font.clone(), sign_fg,
            );
            let galley = {
                let mut job = LayoutJob::default();
                for (style, text) in &segments {
                    let c = style.foreground;
                    let color = if c.a == 0 { CTX_FG } else { Color32::from_rgb(c.r, c.g, c.b) };
                    job.append(text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
                }
                ui.fonts_mut(|f| f.layout_job(job))
            };
            painter.galley(
                Pos2::new(gx + gutter_old_w + gutter_new_w + sign_w, rect.min.y + (row_h - galley.size().y) / 2.0),
                galley, CTX_FG,
            );
        }
    });

    // -- Minimap --
    let inner = scroll_out.inner_rect;
    let minimap_rect = Rect::from_min_max(
        Pos2::new(inner.max.x - MINIMAP_W, inner.min.y),
        inner.max,
    );
    if total_rows > 1 {
        let track_h = minimap_rect.height();
        if track_h > 0.0 {
            for (i, tag) in tags.iter().enumerate() {
                let color = match tag {
                    ChangeTag::Insert => ADD_FG,
                    ChangeTag::Delete => DEL_FG,
                    ChangeTag::Equal => continue,
                };
                let y = minimap_rect.min.y + i as f32 * track_h / total_rows as f32;
                let h = (track_h / total_rows as f32).max(2.0);
                let marker = Rect::from_min_size(
                    Pos2::new(minimap_rect.min.x + 1.0, y),
                    egui::vec2(MINIMAP_W - 2.0, h),
                );
                ui.painter().rect_filled(marker, 1.0, color);
            }
        }
    }
    let minimap_resp = ui.interact(
        minimap_rect,
        ui.id().with("diff_minimap"),
        egui::Sense::click_and_drag(),
    );
    if minimap_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if (minimap_resp.clicked() || minimap_resp.dragged())
        && let Some(p) = minimap_resp.interact_pointer_pos()
    {
        let frac = ((p.y - minimap_rect.min.y) / minimap_rect.height()).clamp(0.0, 1.0);
        let pending = frac * (total_body_h - inner.height()).max(0.0);
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new(("diff_pending_jump", hunk_state_id)), pending)
        });
        ui.ctx().request_repaint();
    }

    // Sync hunk counter
    if jump_to_row.is_none() && !hunk_starts.is_empty() {
        let top_row = (scroll_out.state.offset.y / row_h).round() as usize;
        let probe = top_row.saturating_add(2);
        let derived = hunk_starts
            .iter()
            .rposition(|&s| s <= probe)
            .or_else(|| if probe < hunk_starts[0] { None } else { Some(0) });
        hunk_idx = derived;
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(hunk_state_id, hunk_idx));
}

fn render_image_block(
    ui: &mut egui::Ui,
    tab: &mut DiffTabData,
    left_path: &str,
    right_path: &str,
    active_idx: usize,
) {
    if tab.image_texture.is_none()
        && let Ok(bytes) = {
            let read_path = tab.repo_path.as_ref()
                .map(|repo| std::path::Path::new(repo).join(&tab.right_path))
                .unwrap_or_else(|| std::path::PathBuf::from(&tab.right_path));
            std::fs::read(&read_path)
        }
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
        tab.image_texture = Some(ui.ctx().load_texture(
            format!("crane_diff_img:{}", tab.right_path),
            color,
            egui::TextureOptions::LINEAR,
        ));
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new(left_path).size(11.0).color(DEL_FG).monospace(),
        );
        ui.label(RichText::new("->").size(11.0).color(MUTED));
        ui.label(
            RichText::new(right_path).size(11.0).color(ADD_FG).monospace(),
        );
    });
    ui.add_space(4.0);
    ui.separator();
    ScrollArea::both()
        .id_salt(("diff_image_scroll", active_idx))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            if let Some(tex) = &tab.image_texture {
                let size = tex.size_vec2();
                ui.add(
                    egui::Image::from_texture(tex)
                        .fit_to_original_size(1.0)
                        .max_size(size),
                );
            } else {
                ui.label(
                    RichText::new("Couldn't decode image")
                        .color(theme::current().error.to_color32()),
                );
            }
        });
}

fn resolve_syntax(path: &str) -> &'static syntect::parsing::SyntaxReference {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    find_syntax_for_ext(ext)
}

fn resolve_theme() -> (
    &'static syntect::parsing::SyntaxSet,
    &'static syntect::highlighting::Theme,
) {
    let ss = syntaxes();
    let all = &themes().themes;
    let requested = theme::current().syntax_theme.clone();
    let bg = theme::current().bg;
    let is_light = bg.r as u32 + bg.g as u32 + bg.b as u32 > 128 * 3;
    let st: &SynTheme = all
        .get(&requested)
        .or_else(|| {
            if is_light {
                all.get("InspiredGithub")
                    .or_else(|| all.get("InspiredGitHub"))
            } else {
                all.get("OneHalfDark")
            }
        })
        .unwrap_or_else(|| all.values().next().unwrap_or(fallback_theme()));
    (ss, st)
}

fn fallback_theme() -> &'static syntect::highlighting::Theme {
    crate::views::file_view::fallback_theme()
}

fn measure_char_w(ui: &mut egui::Ui, font: &FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap("0".to_string(), font.clone(), Color32::WHITE))
        .size()
        .x
}

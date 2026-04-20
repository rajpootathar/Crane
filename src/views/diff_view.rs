use crate::state::layout::DiffPane;
use crate::theme;
use crate::views::file_view::{find_syntax_for_ext, syntaxes, themes};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
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
const HEADER: Color32 = Color32::from_rgb(200, 204, 220);
const TAB_ACTIVE_BG: Color32 = Color32::from_rgb(32, 36, 48);

pub fn render(ui: &mut egui::Ui, pane: &mut DiffPane, font_size: f32, _title: &mut String) {
    // Tab bar — one tab per open diff. Click to focus, × to close.
    render_tab_bar(ui, pane);

    let Some(tab) = pane.active_tab() else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No diff loaded").size(14.0).color(HEADER));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Click a changed file in the Changes sidebar to view its diff here.")
                    .size(11.5)
                    .color(MUTED),
            );
        });
        return;
    };

    let diff = TextDiff::from_lines(&tab.left_text, &tab.right_text);
    let font = FontId::new(font_size, FontFamily::Monospace);
    let left_lines = tab.left_text.lines().count().max(1);
    let right_lines = tab.right_text.lines().count().max(1);

    // Pick a syntax by the destination path's extension. Fall back to
    // Plain Text for unknown / extensionless files — syntect returns
    // a no-op highlighter in that case, which is fine.
    let ext = std::path::Path::new(&tab.right_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let syntax = find_syntax_for_ext(ext);
    let ss = syntaxes();
    let all_themes = &themes().themes;
    let requested = theme::current().syntax_theme.clone();
    let bg = theme::current().bg;
    let is_light = bg.r as u32 + bg.g as u32 + bg.b as u32 > 128 * 3;
    let st_theme: &SynTheme = all_themes
        .get(&requested)
        .or_else(|| {
            if is_light {
                all_themes
                    .get("InspiredGithub")
                    .or_else(|| all_themes.get("InspiredGitHub"))
            } else {
                all_themes.get("OneHalfDark")
            }
        })
        .unwrap_or_else(|| all_themes.values().next().expect("at least one theme"));
    let ldigits = left_lines.to_string().len().max(3);
    let rdigits = right_lines.to_string().len().max(3);
    let char_w = ui
        .fonts_mut(|f| f.layout_no_wrap("0".to_string(), font.clone(), Color32::WHITE))
        .size()
        .x;
    let gutter_old_w = char_w * ldigits as f32 + 10.0;
    let gutter_new_w = char_w * rdigits as f32 + 10.0;
    let sign_w = char_w * 2.0 + 8.0;

    // Collect lightweight per-row data up-front. Cheap strings +
    // indices only — no syntect yet. Used to drive minimap AND the
    // virtualized body so we never iterate the diff twice.
    struct Row {
        tag: ChangeTag,
        old_ln: String,
        new_ln: String,
        content: String,
    }
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

    // Hunk starts: indices of the first changed row in each run of
    // consecutive non-Equal rows. Used by the prev/next arrows in the
    // header to jump between change blocks (JetBrains-style).
    let hunk_starts: Vec<usize> = {
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

    // Header row: paths + hunk counter + prev/next arrows.
    let hunk_state_id = egui::Id::new((
        "diff_hunk_idx",
        tab.left_path.clone(),
        tab.right_path.clone(),
    ));
    // None = user hasn't used prev/next yet (or they just opened the
    // tab). First Down lands on hunk 1 rather than skipping to hunk 2,
    // first Up likewise lands on hunk 1. Once set, normal +1/-1 stepping.
    let mut hunk_idx: Option<usize> = ui
        .ctx()
        .data(|d| d.get_temp::<Option<usize>>(hunk_state_id))
        .unwrap_or(None);
    let mut jump_to_row: Option<usize> = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new(&tab.left_path)
                .size(11.0)
                .color(DEL_FG)
                .monospace(),
        );
        ui.label(RichText::new("->").size(11.0).color(MUTED));
        ui.label(
            RichText::new(&tab.right_path)
                .size(11.0)
                .color(ADD_FG)
                .monospace(),
        );
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
    // Persistence of hunk_idx moved to after the scroll area renders
    // — we want to write whatever value we resolve (arrow click OR
    // derived from scroll offset) in one place, post-scroll.
    ui.add_space(4.0);
    ui.separator();

    // Layout: scroll body on the left, narrow minimap strip pinned to
    // the right (VSCode/JetBrains style). Minimap shows a 2-px colored
    // dash per changed line at its proportional Y position, so the
    // user sees at a glance where adds/removes sit without scrolling.
    const MINIMAP_W: f32 = 10.0;
    let row_h = (font_size * 1.25).ceil();
    let total_rows = tags.len().max(1);
    let total_body_h = total_rows as f32 * row_h;
    let full_rect = ui.available_rect_before_wrap();
    let minimap_rect = egui::Rect::from_min_max(
        egui::pos2(full_rect.max.x - MINIMAP_W, full_rect.min.y),
        full_rect.max,
    );
    let body_rect = egui::Rect::from_min_max(
        full_rect.min,
        egui::pos2(full_rect.max.x - MINIMAP_W - 2.0, full_rect.max.y),
    );

    // Minimap — paint first at absolute coords so it always shows,
    // independent of the scroll area's internal sizing.
    let minimap_bg = Color32::from_rgba_premultiplied(0, 0, 0, 60);
    ui.painter().rect_filled(minimap_rect, 0.0, minimap_bg);
    if total_rows > 0 {
        let track_h = minimap_rect.height();
        let px_per_row = (track_h / total_rows as f32).max(1.0);
        for (i, tag) in tags.iter().enumerate() {
            let color = match tag {
                ChangeTag::Insert => ADD_FG,
                ChangeTag::Delete => DEL_FG,
                ChangeTag::Equal => continue,
            };
            let y = minimap_rect.min.y + i as f32 * track_h / total_rows as f32;
            let h = px_per_row.max(2.0);
            let marker = egui::Rect::from_min_size(
                egui::pos2(minimap_rect.min.x + 1.0, y),
                egui::vec2(MINIMAP_W - 2.0, h),
            );
            ui.painter().rect_filled(marker, 1.0, color);
        }
    }

    // Click on the minimap jumps the body scroll to that fraction.
    let minimap_resp = ui.interact(
        minimap_rect,
        ui.id().with("diff_minimap"),
        egui::Sense::click_and_drag(),
    );
    let jump_y: Option<f32> = if let Some(row) = jump_to_row {
        // Prev/Next arrow click — center the target hunk vertically
        // with a small offset so the first line isn't flush-top.
        let y = (row as f32 * row_h - row_h * 2.0).max(0.0);
        Some(y)
    } else if (minimap_resp.clicked() || minimap_resp.dragged())
        && let Some(p) = minimap_resp.interact_pointer_pos()
    {
        let frac = ((p.y - minimap_rect.min.y) / minimap_rect.height()).clamp(0.0, 1.0);
        Some(frac * (total_body_h - body_rect.height()).max(0.0))
    } else {
        None
    };
    if minimap_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Constrain the scroll body to the reserved rect so it doesn't
    // draw under the minimap strip.
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body_rect));
    // `ScrollArea::show_rows` snapshots `item_spacing.y` BEFORE the
    // body closure runs to compute its virtual row stride. Zero it on
    // `body_ui` up here so stride == row_h exactly; otherwise prev /
    // next jumps and the minimap land a few dozen pixels off and the
    // error compounds with distance.
    body_ui.spacing_mut().item_spacing.y = 0.0;
    let mut scroll = ScrollArea::both()
        .auto_shrink([false; 2])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    if let Some(y) = jump_y {
        scroll = scroll.vertical_scroll_offset(y);
    }
    // Virtualize: only the visible rows run syntect + build a galley.
    // egui's `show` iterates every child even when off-screen, so for
    // a 2k-line diff we were running syntect 2k times per frame. With
    // `show_rows` only the ~50 visible rows pay.
    let scroll_out = scroll.show_rows(&mut body_ui, row_h, rows.len(), |ui, row_range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in row_range {
            let r = &rows[i];
            let (sign, sign_fg, bg) = match r.tag {
                ChangeTag::Delete => ("-", DEL_FG, DEL_BG),
                ChangeTag::Insert => ("+", ADD_FG, ADD_BG),
                ChangeTag::Equal => (" ", CTX_FG, Color32::TRANSPARENT),
            };
            let mut hl = HighlightLines::new(syntax, st_theme);
            let segments: Vec<(SynStyle, String)> = hl
                .highlight_line(&format!("{}\n", r.content), ss)
                .map(|v| {
                    v.into_iter()
                        .map(|(s, t)| (s, t.trim_end_matches('\n').to_string()))
                        .collect()
                })
                .unwrap_or_else(|_| vec![(SynStyle::default(), r.content.clone())]);
            row(
                ui,
                &font,
                sign_fg,
                bg,
                &r.old_ln,
                &r.new_ln,
                sign,
                &segments,
                gutter_old_w,
                gutter_new_w,
                sign_w,
                row_h,
            );
        }
    });

    // Sync the counter + step reference with manual scrolling (wheel,
    // minimap drag, scrollbar). Derive the "current" hunk from the
    // scroll offset: topmost visible row → the hunk_start at or above
    // it. Skip this when the user just pressed an arrow (we already
    // set hunk_idx + scrolled there; deriving now would fight our own
    // write for one frame).
    if jump_to_row.is_none() && !hunk_starts.is_empty() {
        let top_row = (scroll_out.state.offset.y / row_h).round() as usize;
        // Add a small lead (row_h * 2 worth) so "on hunk N" stays
        // true while the hunk is scrolled near the top of the viewport,
        // not only when the very first row sits at y=0. Matches the
        // offset the arrow jump uses to center a hunk.
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

fn render_tab_bar(ui: &mut egui::Ui, pane: &mut DiffPane) {
    if pane.tabs.is_empty() {
        return;
    }
    let mut close_idx: Option<usize> = None;
    let mut focus_idx: Option<usize> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, tab) in pane.tabs.iter().enumerate() {
            let is_active = i == pane.active;
            let bg = if is_active { TAB_ACTIVE_BG } else { Color32::TRANSPARENT };
            ui.scope(|ui| {
                let v = ui.visuals_mut();
                v.widgets.inactive.weak_bg_fill = bg;
                v.widgets.inactive.bg_fill = bg;
                v.widgets.hovered.bg_fill = TAB_ACTIVE_BG;
                v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                let color = if is_active { HEADER } else { MUTED };
                let label_btn = egui::Button::new(
                    RichText::new(&tab.title).size(11.5).color(color),
                )
                .min_size(egui::vec2(0.0, 22.0));
                if ui.add(label_btn).clicked() {
                    focus_idx = Some(i);
                }
                let close_btn = egui::Button::new(
                    RichText::new(icons::X).size(10.0).color(MUTED),
                )
                .min_size(egui::vec2(18.0, 22.0));
                if ui.add(close_btn).clicked() {
                    close_idx = Some(i);
                }
            });
        }
    });
    if let Some(i) = focus_idx {
        pane.active = i;
    }
    if let Some(i) = close_idx {
        pane.close(i);
    }
    ui.separator();
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    font: &FontId,
    sign_fg: Color32,
    bg: Color32,
    old_ln: &str,
    new_ln: &str,
    sign: &str,
    segments: &[(SynStyle, String)],
    gutter_old_w: f32,
    gutter_new_w: f32,
    sign_w: f32,
    row_h: f32,
) {
    // Build a syntect-colored galley for the content. Each segment
    // becomes one LayoutJob section with its own fg color. We don't
    // forward syntect's background because the diff row's own add/del
    // tint already colors the line — layering a syntax bg on top would
    // fight the tint.
    let mut job = LayoutJob::default();
    for (style, text) in segments {
        let c = style.foreground;
        // syntect uses 0 alpha for "theme default" on some themes —
        // fall back to MUTED so we never render invisible text.
        let color = if c.a == 0 {
            CTX_FG
        } else {
            Color32::from_rgb(c.r, c.g, c.b)
        };
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    }
    let content_galley = ui.fonts_mut(|f| f.layout_job(job));
    // Fixed row_h — must match the value passed to ScrollArea::show_rows
    // so prev/next hunk jumps and minimap mapping stay in lockstep with
    // the body. Don't max against galley height: a taller galley would
    // silently shift downstream rows off the scroll grid.
    let content_w = content_galley.size().x;
    let total_w = gutter_old_w + gutter_new_w + sign_w + content_w + 8.0;
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
    let painter = ui.painter();
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, bg);
    }
    painter.text(
        egui::pos2(rect.min.x + gutter_old_w - 4.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        old_ln,
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w - 4.0,
            rect.center().y,
        ),
        egui::Align2::RIGHT_CENTER,
        new_ln,
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w + sign_w / 2.0,
            rect.center().y,
        ),
        egui::Align2::CENTER_CENTER,
        sign,
        font.clone(),
        sign_fg,
    );
    painter.galley(
        egui::pos2(
            rect.min.x + gutter_old_w + gutter_new_w + sign_w,
            rect.min.y + (row_h - content_galley.size().y) / 2.0,
        ),
        content_galley,
        CTX_FG,
    );
}

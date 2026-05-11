use crate::jobs::{JobKey, JobOutput, Pool, Priority, Scope};
use crate::state::layout::DiffTabData;
use crate::theme;
use crate::views::file_util::is_image_path;
use crate::views::file_view::{find_syntax_for_ext, syntaxes, themes};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, RichText, ScrollArea};
use egui_phosphor::regular as icons;
use similar::{ChangeTag, TextDiff};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, Theme as SynTheme};
// DefaultHasher is still used in tab_key (stable per-tab key from
// left_path + right_path). Per-frame input fingerprint hashing was
// removed — DiffTabData::inputs_version replaces it.

const ADD_BG: Color32 = Color32::from_rgb(25, 55, 35);
const DEL_BG: Color32 = Color32::from_rgb(60, 28, 32);
const CTX_FG: Color32 = Color32::from_rgb(180, 186, 198);
const ADD_FG: Color32 = Color32::from_rgb(140, 220, 150);
const DEL_FG: Color32 = Color32::from_rgb(230, 130, 130);
const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
const MINIMAP_W: f32 = 10.0;

pub struct Row {
    pub tag: ChangeTag,
    pub old_ln: String,
    pub new_ln: String,
    pub content: String,
    /// 1-based new line number, or None for deletion rows. Used to
    /// match similar's row-level hunks against git diff's line-range
    /// hunks — without this we'd zip by index and misalign when git
    /// merges adjacent changes by context that similar keeps split
    /// (or vice versa).
    pub new_lno: Option<usize>,
    pub old_lno: Option<usize>,
}

/// Pure result of diffing left_text vs right_text + parsing git's
/// hunk patches. Render reads this with zero allocation; the job
/// thread builds it. See `compute_diff`.
#[allow(dead_code)]
pub struct DiffComputed {
    pub rows: Vec<Row>,
    pub tags: Vec<ChangeTag>,
    pub hunk_starts: Vec<usize>,
    pub hunk_patches: Vec<Option<String>>,
    /// Per-hunk: true if the hunk is already in the index (action is
    /// "unstage"), false otherwise (action is "stage"). Probed once
    /// per compute via `git apply --reverse --cached --check`.
    pub hunk_staged: Vec<bool>,
    pub row_to_hunk: Vec<Option<usize>>,
    /// True for rows that belong to a git hunk shared with an earlier
    /// visual hunk (downstream rows of a multi-visual-hunk group). The
    /// stage-button gutter renders a vertical connector through these
    /// rows so the user can see the changes ship together.
    pub row_in_shared_group: Vec<bool>,
    pub ldigits: usize,
    pub rdigits: usize,
    pub left_lines_count: usize,
    pub right_lines_count: usize,
}

impl DiffComputed {
    /// Sentinel returned when a job is cancelled mid-compute. Render
    /// treats this the same as a cache miss.
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            tags: Vec::new(),
            hunk_starts: Vec::new(),
            hunk_patches: Vec::new(),
            hunk_staged: Vec::new(),
            row_to_hunk: Vec::new(),
            row_in_shared_group: Vec::new(),
            ldigits: 0,
            rdigits: 0,
            left_lines_count: 0,
            right_lines_count: 0,
        }
    }
}

/// The expensive bit. Pulled out of the render path so JobSystem can
/// run it on the I/O pool — `git diff` shells out a subprocess (the
/// real latency culprit) and `TextDiff::from_lines` walks both texts.
///
/// Cancel-checks at phase boundaries so a tab closing mid-compute
/// frees the worker quickly. Cancellation is cooperative — we never
/// abort mid-syscall, only at safe points.
fn compute_diff(
    left_text: String,
    right_text: String,
    repo_path: Option<String>,
    right_path: String,
    cancel: &crate::jobs::CancelToken,
) -> DiffComputed {
    if cancel.is_cancelled() {
        return DiffComputed::empty();
    }
    let diff = TextDiff::from_lines(&left_text, &right_text);
    let left_lines_count = left_text.lines().count().max(1);
    let right_lines_count = right_text.lines().count().max(1);
    let ldigits = left_lines_count.to_string().len().max(3);
    let rdigits = right_lines_count.to_string().len().max(3);

    let rows: Vec<Row> = diff
        .iter_all_changes()
        .map(|c| {
            let old_lno = c.old_index().map(|i| i + 1);
            let new_lno = c.new_index().map(|i| i + 1);
            Row {
                tag: c.tag(),
                old_ln: old_lno
                    .map(|n| format!("{:>w$}", n, w = ldigits))
                    .unwrap_or_else(|| " ".repeat(ldigits)),
                new_ln: new_lno
                    .map(|n| format!("{:>w$}", n, w = rdigits))
                    .unwrap_or_else(|| " ".repeat(rdigits)),
                content: c.value().trim_end_matches('\n').to_string(),
                new_lno,
                old_lno,
            }
        })
        .collect();

    let tags: Vec<ChangeTag> = rows.iter().map(|r| r.tag).collect();
    let total_rows = tags.len().max(1);

    let mut hunk_starts: Vec<usize> = Vec::new();
    let mut in_hunk = false;
    for (i, tag) in tags.iter().enumerate() {
        let changed = !matches!(tag, ChangeTag::Equal);
        if changed && !in_hunk {
            hunk_starts.push(i);
        }
        in_hunk = changed;
    }

    if cancel.is_cancelled() {
        return DiffComputed::empty();
    }

    // Per-hunk patches via `git diff` (subprocess) + parse_hunks. The
    // shelling-out is what made render-frame stalls visible; running
    // it here on the I/O pool keeps the UI thread out of waitpid.
    //
    // Match by line number, not index: git groups adjacent changes by
    // context-line proximity, similar groups by contiguity of changed
    // rows. The two algorithms produce different hunk counts when
    // changes sit within `--unified=3` of each other (git merges
    // them, similar shows them separately), so zipping by index
    // misaligns patches — earlier hunks would unstage the wrong patch
    // and the last hunk would get None and render no icon at all.
    let hunk_patches: Vec<Option<String>> = if let Some(repo) = repo_path.as_ref() {
        let repo_p = std::path::Path::new(repo);
        if let Some(raw) = crate::git::file_diff_raw(repo_p, &right_path) {
            let parsed = crate::git::parse_hunks_detailed(&raw);
            // Dedupe by git-hunk identity. similar splits adjacent
            // changes that git considers one hunk (3-line context);
            // without dedup, two visual hunks resolve to the same
            // patch and clicking either stages both — confusing, and
            // it makes the second appear staged after a refresh even
            // though the user only checked the first.
            let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
            hunk_starts
                .iter()
                .map(|&start_row| {
                    // Probe the hunk's first row plus a few following
                    // for a usable line number. A pure-deletion hunk's
                    // first row has no new_lno; fall back to old_lno.
                    let mut new_target: Option<usize> = None;
                    let mut old_target: Option<usize> = None;
                    for r in &rows[start_row..(start_row + 5).min(rows.len())] {
                        if new_target.is_none() {
                            new_target = r.new_lno;
                        }
                        if old_target.is_none() {
                            old_target = r.old_lno;
                        }
                        if new_target.is_some() && old_target.is_some() {
                            break;
                        }
                    }
                    let matched = parsed
                        .iter()
                        .enumerate()
                        .find(|(_, h)| {
                            if let Some(n) = new_target {
                                let lo = h.new_start;
                                let hi = h.new_start + h.new_count;
                                if n >= lo && n < hi {
                                    return true;
                                }
                            }
                            if let Some(o) = old_target {
                                let lo = h.old_start;
                                let hi = h.old_start + h.old_count;
                                if o >= lo && o < hi {
                                    return true;
                                }
                            }
                            false
                        });
                    match matched {
                        Some((idx, h)) if seen.insert(idx) => Some(h.patch.clone()),
                        _ => None,
                    }
                })
                .collect()
        } else {
            vec![None; hunk_starts.len()]
        }
    } else {
        vec![None; hunk_starts.len()]
    };

    if cancel.is_cancelled() {
        return DiffComputed::empty();
    }

    // Probe each hunk for staged state. Done on the I/O pool with the
    // diff compute so the UI never blocks on N git invocations.
    let hunk_staged: Vec<bool> = if let Some(repo) = repo_path.as_ref() {
        let repo_p = std::path::Path::new(repo);
        hunk_patches
            .iter()
            .map(|p| match p {
                Some(patch) => crate::git::is_hunk_staged(repo_p, patch),
                None => false,
            })
            .collect()
    } else {
        vec![false; hunk_patches.len()]
    };

    let mut row_to_hunk: Vec<Option<usize>> = vec![None; total_rows];
    for (hi, &start) in hunk_starts.iter().enumerate() {
        let end = hunk_starts.get(hi + 1).copied().unwrap_or(total_rows);
        for r in start..end {
            if r < row_to_hunk.len() {
                row_to_hunk[r] = Some(hi);
            }
        }
    }

    // Mark rows that share a git hunk with an earlier visual hunk —
    // i.e. downstream rows of a multi-visual-hunk group. The render
    // layer draws a vertical connector through them so it's obvious
    // the changes are bound to one stage action.
    let mut row_in_shared_group: Vec<bool> = vec![false; total_rows];
    {
        let mut hi = 0;
        while hi < hunk_starts.len() {
            if hunk_patches.get(hi).and_then(|p| p.as_ref()).is_none() {
                hi += 1;
                continue;
            }
            let mut j = hi + 1;
            while j < hunk_starts.len()
                && hunk_patches.get(j).and_then(|p| p.as_ref()).is_none()
            {
                j += 1;
            }
            if j > hi + 1 {
                let anchor_row = hunk_starts[hi];
                let group_end = hunk_starts.get(j).copied().unwrap_or(total_rows);
                for r in (anchor_row + 1)..group_end {
                    if r < row_in_shared_group.len() {
                        row_in_shared_group[r] = true;
                    }
                }
            }
            hi = j;
        }
    }

    DiffComputed {
        rows,
        tags,
        hunk_starts,
        hunk_patches,
        hunk_staged,
        row_to_hunk,
        row_in_shared_group,
        ldigits,
        rdigits,
        left_lines_count,
        right_lines_count,
    }
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

    let font = FontId::new(font_size, FontFamily::Monospace);
    let syntax = resolve_syntax(&tab.right_path);
    let (ss, st_theme) = resolve_theme();
    let char_w = measure_char_w(ui, &font);

    // Cache check via explicit version counter — no per-frame hashing
    // of left_text + right_text. Mutators of those fields MUST call
    // DiffTabData::invalidate() to bump inputs_version. Cache hit →
    // one u64 compare + Arc::clone, zero hashing, zero allocation.
    let current_version = tab.inputs_version;

    if let Some(handle) = tab.compute_job.as_ref() {
        if let Some(out) = handle.try_recv() {
            match out {
                JobOutput::Done(d) => {
                    tab.computed = Some(Arc::new(d));
                    tab.computed_for_version = tab.job_for_version;
                }
                JobOutput::Cancelled => {}
            }
            tab.compute_job = None;
        }
    }

    let cached_ok = tab.computed.is_some() && tab.computed_for_version == current_version;
    let job_ok = tab.compute_job.is_some() && tab.job_for_version == current_version;

    if !cached_ok && !job_ok && let Some(jobs) = crate::jobs::global() {
        // Stable key per diff tab — hash of (left_path, right_path)
        // identifies the tab uniquely without changing on every edit.
        // Earlier in-flight jobs are superseded via key dedup; their
        // cancel tokens flip and their results are dropped.
        let mut hasher = DefaultHasher::new();
        tab.left_path.hash(&mut hasher);
        tab.right_path.hash(&mut hasher);
        let tab_key = hasher.finish();
        let left_text = tab.left_text.clone();
        let right_text = tab.right_text.clone();
        let repo_path = tab.repo_path.clone();
        let right_path = tab.right_path.clone();
        let handle = jobs.submit(
            JobKey::new(Scope::Tab(tab_key), "diff_compute"),
            Priority::Foreground,
            Pool::Io,
            move |tok| compute_diff(left_text, right_text, repo_path, right_path, tok),
        );
        tab.compute_job = Some(handle);
        tab.job_for_version = current_version;
    }

    // Render with the freshest cache we have. If the inputs version
    // moved but a previous Arc<DiffComputed> still exists, keep showing
    // it while the new job runs — prevents the "flash to spinner" the
    // user sees after every hunk stage. Only fall back to the spinner
    // on the very first compute (no cached value at all).
    let computed = match tab.computed.as_ref() {
        Some(c) => Arc::clone(c),
        None => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Computing diff…")
                        .size(11.0)
                        .color(MUTED)
                        .monospace(),
                );
            });
            return;
        }
    };
    let _ = cached_ok;

    let rows = &computed.rows;
    let tags = &computed.tags;
    let hunk_starts = &computed.hunk_starts;
    let hunk_patches = &computed.hunk_patches;
    let total_rows = tags.len().max(1);
    let ldigits = computed.ldigits;
    let rdigits = computed.rdigits;

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

    // Surface stage_hunk failures so they don't fail silently.
    // Previously tab.error was set but never rendered — a failed
    // git apply looked indistinguishable from "click did nothing."
    if let Some(err) = tab.error.clone() {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{}  {}", icons::WARNING, err))
                    .size(11.0)
                    .color(DEL_FG)
                    .monospace(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui
                    .small_button(icons::X)
                    .on_hover_text("Dismiss")
                    .clicked()
                {
                    tab.error = None;
                }
            });
        });
        ui.add_space(2.0);
    }

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

    // row_to_hunk is precomputed in DiffComputed; alias the cached
    // slice instead of rebuilding it every frame.
    let row_to_hunk = &computed.row_to_hunk;

    let gutter_old_w = char_w * ldigits as f32 + 10.0;
    let gutter_new_w = char_w * rdigits as f32 + 10.0;
    let sign_w = char_w * 2.0 + 8.0;
    // Stage-hunk control gutter. Wider than before so it's an
    // actual click target instead of a 20-px sliver, and uses a
    // checkbox glyph (matches Right Panel Changes "stage" affordance)
    // instead of a tiny plus icon.
    let stage_btn_w = 28.0;

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
            let mut stage_btn_paint: Option<(egui::Rect, bool, bool)> = None;
            if is_hunk_start && let Some(hi) = row_to_hunk[i] {
                if let Some(patch) = &hunk_patches[hi] {
                    let is_unstage = computed.hunk_staged.get(hi).copied().unwrap_or(false);
                    let btn_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(stage_btn_w, row_h),
                    );
                    let btn_id = egui::Id::new((
                        "stage_hunk",
                        tab.left_path.clone(),
                        tab.right_path.clone(),
                        hi,
                    ));
                    let btn_resp = ui.interact(btn_rect, btn_id, egui::Sense::click());
                    let btn_hovered = btn_resp.hovered();
                    let btn_clicked = btn_resp.clicked();
                    if btn_hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let hover = if is_unstage { "Unstage this hunk" } else { "Stage this hunk" };
                    btn_resp.on_hover_text(hover);
                    if btn_clicked
                        && let Some(repo) = &tab.repo_path
                    {
                        let repo_path = std::path::Path::new(repo);
                        let res = if is_unstage {
                            crate::git::unstage_hunk(repo_path, patch)
                        } else {
                            crate::git::stage_hunk(repo_path, patch)
                        };
                        match res {
                            Ok(()) => {
                                tab.pending_hunk_stage = true;
                                ui.ctx().data_mut(|d| d.insert_temp(refresh_id, true));
                            }
                            Err(e) => {
                                let verb = if is_unstage { "Unstage" } else { "Stage" };
                                log::warn!("{} hunk failed: {e}", verb.to_lowercase());
                                tab.error = Some(format!("{verb} hunk failed: {e}"));
                            }
                        }
                    }
                    stage_btn_paint = Some((btn_rect, btn_hovered, is_unstage));
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
            // Stage-hunk affordance. A single circle-check glyph in
            // the addition-accent color. Hover paints a subtle round
            // background — no nested box-around-a-box (the earlier
            // pill + square-checkbox combination read as two
            // overlapping boxes).
            // Connector line for downstream rows of a multi-visual-hunk
            // git group. Renders a thin vertical accent through the
            // stage-button gutter so the user can trace which rows the
            // anchor's check button covers — without it, a single icon
            // anchoring two visually separate hunks looks accidental.
            let in_group = computed
                .row_in_shared_group
                .get(i)
                .copied()
                .unwrap_or(false);
            let next_in_group = computed
                .row_in_shared_group
                .get(i + 1)
                .copied()
                .unwrap_or(false);
            if in_group || (stage_btn_paint.is_some() && next_in_group) {
                let cx = rect.min.x + stage_btn_w * 0.5;
                let connector_color = Color32::from_rgba_unmultiplied(
                    ADD_FG.r(), ADD_FG.g(), ADD_FG.b(), 90,
                );
                let top_y = if in_group { rect.min.y } else { rect.center().y };
                painter.line_segment(
                    [
                        Pos2::new(cx, top_y),
                        Pos2::new(cx, rect.max.y),
                    ],
                    egui::Stroke::new(1.5, connector_color),
                );
            }
            // Three visually distinct states so the user can tell at
            // a glance whether a hunk is staged:
            //   - unstaged, idle:  empty CIRCLE,         muted gray
            //   - unstaged, hover: CHECK_CIRCLE preview, green + disc
            //   - staged,   idle:  CHECK_CIRCLE,         bright green
            //   - staged,   hover: CHECK_CIRCLE bolder,  green + disc
            // Same icon family, different glyph for the empty state so
            // "click changes the state" reads visually, not just via
            // a colour shift.
            if let Some((btn_rect, hovered, is_staged)) = &stage_btn_paint {
                let is_staged = *is_staged;
                let hovered = *hovered;
                if hovered {
                    let center = btn_rect.center();
                    painter.circle_filled(center, btn_rect.height() * 0.42, ADD_BG);
                }
                let glyph = if is_staged || hovered {
                    icons::CHECK_CIRCLE
                } else {
                    icons::CIRCLE
                };
                let glyph_color = if is_staged || hovered {
                    ADD_FG
                } else {
                    theme::current().text_muted.to_color32()
                };
                let glyph_size = if is_staged && hovered { 18.0 } else { 16.0 };
                painter.text(
                    btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    FontId::new(glyph_size, FontFamily::Proportional),
                    glyph_color,
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

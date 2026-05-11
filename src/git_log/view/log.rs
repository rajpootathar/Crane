use egui::{Color32, Sense};
use egui_phosphor::regular as icons;

use crate::git_log::state::{GitLogOp, GitLogState};
use crate::ui::util::muted;

const ROW_H: f32 = 22.0;
const COL_W: f32 = 14.0;
const DOT_R: f32 = 4.0;
const GRAPH_PAD_LEFT: f32 = 8.0;

/// Categorised ref names parsed out of git log's `(HEAD -> main,
/// origin/main, tag: v1.0)` decoration string. Each becomes a small
/// rounded pill rendered to the left of the commit subject so the
/// user can see at a glance what's at this commit.
#[derive(Clone)]
struct RefPill {
    label: String,
    bg: Color32,
    fg: Color32,
}

/// Parse refs_decoration into pills. Decoration string format (from
/// `git log --pretty=...%d`):
///   ` (HEAD -> main, origin/main, tag: v1.0)`
/// Always wrapped in `(...)` and prefixed with a space when non-empty.
///
/// Categorisation uses the actual `RefSet` rather than slash-counting
/// the name — a local branch can legitimately contain slashes
/// (`feat/drag-drop`), so the previous `contains('/')` heuristic
/// misclassified those as remote-tracking.
fn parse_ref_pills(
    decoration: &str,
    refs: &crate::git_log::refs::RefSet,
) -> Vec<RefPill> {
    let body = decoration.trim().trim_start_matches('(').trim_end_matches(')');
    if body.is_empty() {
        return Vec::new();
    }

    // Short-name sets for O(1) categorisation. RefSet stores fully
    // qualified `refs/heads/foo`; decoration uses the short form.
    let local_names: std::collections::HashSet<&str> = refs
        .local
        .iter()
        .filter_map(|r| r.name.strip_prefix("refs/heads/"))
        .collect();
    let remote_names: std::collections::HashSet<&str> = refs
        .remote
        .iter()
        .filter_map(|r| r.name.strip_prefix("refs/remotes/"))
        .collect();

    let categorise = |name: &str| -> (Color32, Color32) {
        if local_names.contains(name) {
            // Local branch — purple, white text.
            (Color32::from_rgb(171, 71, 188), Color32::WHITE)
        } else if remote_names.contains(name) {
            // Remote-tracking branch — blue, white text.
            (Color32::from_rgb(66, 165, 245), Color32::WHITE)
        } else {
            // Unknown — neutral grey so it stands apart from miscategorised
            // colour rather than masquerading as the wrong category.
            (Color32::from_rgb(110, 118, 132), Color32::WHITE)
        }
    };

    let mut out = Vec::new();
    for raw in body.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (label, bg, fg) = if let Some(rest) = raw.strip_prefix("HEAD -> ") {
            (
                format!("HEAD → {rest}"),
                Color32::from_rgb(102, 187, 106),
                Color32::BLACK,
            )
        } else if raw == "HEAD" {
            (
                "HEAD".to_string(),
                Color32::from_rgb(102, 187, 106),
                Color32::BLACK,
            )
        } else if let Some(t) = raw.strip_prefix("tag: ") {
            (
                t.to_string(),
                Color32::from_rgb(255, 202, 40),
                Color32::BLACK,
            )
        } else {
            let (bg, fg) = categorise(raw);
            (raw.to_string(), bg, fg)
        };
        out.push(RefPill { label, bg, fg });
    }
    out
}

/// 8-color palette keyed by the lane allocation epoch. Hand-picked
/// to be legible on both light and dark themes.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(102, 187, 106), // green
    Color32::from_rgb(66, 165, 245),  // blue
    Color32::from_rgb(255, 152, 0),   // orange
    Color32::from_rgb(171, 71, 188),  // purple
    Color32::from_rgb(236, 64, 122),  // pink
    Color32::from_rgb(38, 166, 154),  // teal
    Color32::from_rgb(239, 83, 80),   // red
    Color32::from_rgb(255, 202, 40),  // yellow
];

pub fn render(ui: &mut egui::Ui, state: &mut GitLogState) {
    let Some(frame) = state.frame.as_ref() else {
        ui.add_space(8.0);
        if state.is_loading() {
            ui.label(egui::RichText::new("loading…").small().color(muted()));
        } else {
            ui.label(
                egui::RichText::new("no commits to display")
                    .small()
                    .color(muted()),
            );
        }
        return;
    };

    if frame.commits.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("No commits yet").color(muted()));
        return;
    }

    // Filter bar — refined, theme-aware row with consistent control
    // heights. Search field gets a magnifying-glass affordance and a
    // clear (×) button when non-empty. Facet pickers use compact
    // pill-style toggles with a chevron, sized to their label.
    let theme = crate::theme::current();
    let bar_h = 24.0;
    let radius = egui::CornerRadius::same(4);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 6.0;

        // ---- Search input ----------------------------------------
        let search_w = 240.0_f32;
        let (search_rect, _) = ui.allocate_exact_size(
            egui::vec2(search_w, bar_h),
            egui::Sense::hover(),
        );
        ui.painter()
            .rect_filled(search_rect, radius, theme.surface_alt.to_color32());
        ui.painter().rect_stroke(
            search_rect,
            radius,
            egui::Stroke::new(1.0, theme.divider.to_color32()),
            egui::StrokeKind::Inside,
        );
        // Search icon glyph
        ui.painter().text(
            egui::pos2(search_rect.left() + 8.0, search_rect.center().y),
            egui::Align2::LEFT_CENTER,
            icons::MAGNIFYING_GLASS,
            egui::FontId::proportional(12.0),
            theme.text_muted.to_color32(),
        );
        // TextEdit overlay — borderless, inset to leave room for the
        // icon left and clear button right.
        let text_inner = egui::Rect::from_min_max(
            egui::pos2(search_rect.left() + 26.0, search_rect.top() + 2.0),
            egui::pos2(search_rect.right() - 22.0, search_rect.bottom() - 2.0),
        );
        let filter_id = egui::Id::new("git_log_filter_text");
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text_inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let resp = child.add(
            egui::TextEdit::singleline(&mut state.filter.text)
                .hint_text("subject / hash / author")
                .id(filter_id)
                .desired_width(text_inner.width())
                .background_color(Color32::TRANSPARENT),
        );
        if state.pending_focus_filter {
            resp.request_focus();
            state.pending_focus_filter = false;
        }
        // Clear button when the field is non-empty.
        if !state.filter.text.is_empty() {
            let clear_rect = egui::Rect::from_min_max(
                egui::pos2(search_rect.right() - 20.0, search_rect.top() + 2.0),
                egui::pos2(search_rect.right() - 4.0, search_rect.bottom() - 2.0),
            );
            let clear_resp = ui.interact(
                clear_rect,
                egui::Id::new("git_log_filter_clear"),
                egui::Sense::click(),
            );
            if clear_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                ui.painter().rect_filled(
                    clear_rect,
                    egui::CornerRadius::same(3),
                    theme.surface_hi.to_color32(),
                );
            }
            ui.painter().text(
                clear_rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::X,
                egui::FontId::proportional(11.0),
                theme.text_muted.to_color32(),
            );
            if clear_resp.clicked() {
                state.filter.text.clear();
            }
        }

        // ---- Branch facet ----------------------------------------
        let local_branches: Vec<String> = frame
            .refs
            .local
            .iter()
            .map(|r| r.name.trim_start_matches("refs/heads/").to_string())
            .collect();
        let branch_label = state
            .filter
            .branch
            .as_deref()
            .unwrap_or("branch")
            .to_string();
        compact_combo(
            ui,
            "git_log_branch_filter",
            &branch_label,
            state.filter.branch.is_some(),
            &theme,
            |ui| {
                ui.selectable_value(&mut state.filter.branch, None, "all branches");
                ui.separator();
                for b in &local_branches {
                    ui.selectable_value(&mut state.filter.branch, Some(b.clone()), b);
                }
            },
        );

        // ---- User facet ------------------------------------------
        let mut authors: Vec<String> =
            frame.commits.iter().map(|c| c.author.clone()).collect();
        authors.sort();
        authors.dedup();
        let user_label = state
            .filter
            .user
            .as_deref()
            .unwrap_or("user")
            .to_string();
        compact_combo(
            ui,
            "git_log_user_filter",
            &user_label,
            state.filter.user.is_some(),
            &theme,
            |ui| {
                ui.selectable_value(&mut state.filter.user, None, "all users");
                ui.separator();
                for u in &authors {
                    ui.selectable_value(&mut state.filter.user, Some(u.clone()), u);
                }
            },
        );

        // Filter-status indicator pushed to the right edge.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if state.filter.text.is_empty()
                && state.filter.branch.is_none()
                && state.filter.user.is_none()
            {
                ui.label(
                    egui::RichText::new(format!("{} commits", frame.commits.len()))
                        .size(11.0)
                        .color(theme.text_muted.to_color32()),
                );
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "{} of {}",
                        state.last_visible_count,
                        frame.commits.len()
                    ))
                    .size(11.0)
                    .color(theme.text_muted.to_color32()),
                );
            }
        });
    });
    ui.add_space(2.0);

    // Apply filters.
    let needle = state.filter.text.to_lowercase();
    let branch_filter = state.filter.branch.clone();
    let user_filter = state.filter.user.clone();

    // For the branch / tag filter we want every commit REACHABLE from
    // that ref's tip via parents, not just the one decorated with the
    // ref name (which is only the tip itself). Resolve the tip SHA
    // from frame.refs and BFS the parent graph in-memory; ~10k
    // commits fit in microseconds.
    let reachable: Option<std::collections::HashSet<String>> =
        branch_filter.as_ref().and_then(|name| {
            let tip = frame
                .refs
                .local
                .iter()
                .chain(frame.refs.remote.iter())
                .chain(frame.refs.tags.iter())
                .find(|r| {
                    r.name.trim_start_matches("refs/heads/") == name.as_str()
                        || r.name.trim_start_matches("refs/remotes/") == name.as_str()
                        || r.name.trim_start_matches("refs/tags/") == name.as_str()
                })?
                .sha
                .clone();
            let parent_map: std::collections::HashMap<&str, &Vec<String>> = frame
                .commits
                .iter()
                .map(|c| (c.sha.as_str(), &c.parents))
                .collect();
            let mut set = std::collections::HashSet::new();
            let mut stack = vec![tip];
            while let Some(sha) = stack.pop() {
                if !set.insert(sha.clone()) {
                    continue;
                }
                if let Some(parents) = parent_map.get(sha.as_str()) {
                    for p in parents.iter() {
                        if !set.contains(p) {
                            stack.push(p.clone());
                        }
                    }
                }
            }
            Some(set)
        });

    let visible: Vec<usize> = (0..frame.commits.len())
        .filter(|&i| {
            let c = &frame.commits[i];
            if !needle.is_empty() {
                let hay = format!("{} {} {}", c.subject, c.sha, c.author).to_lowercase();
                if !hay.contains(&needle) {
                    return false;
                }
            }
            if let Some(set) = &reachable {
                if !set.contains(&c.sha) {
                    return false;
                }
            }
            if let Some(u) = &user_filter {
                if &c.author != u {
                    return false;
                }
            }
            true
        })
        .collect();

    // When a filter is active, recompute lanes from ONLY the visible
    // commits so the graph reflects what's on screen — otherwise
    // lanes for filtered-out commits would persist as passthroughs
    // and "octopus lines" from other branches would still draw.
    let filter_active = !needle.is_empty()
        || branch_filter.is_some()
        || user_filter.is_some();
    let local_lanes: Option<crate::git_log::graph::LaneFrame> = if filter_active {
        // Cache by (filter signature, frame generation). When the
        // user types or scrolls, the same filter + same source frame
        // yields the same lanes — without this cache, every frame
        // clones the visible CommitRecords and re-runs graph::layout.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&needle, &mut hasher);
        std::hash::Hash::hash(&branch_filter, &mut hasher);
        std::hash::Hash::hash(&user_filter, &mut hasher);
        let filter_sig = std::hash::Hasher::finish(&hasher);
        let frame_gen = frame.generation;
        let cache_hit = state
            .filter_lane_cache
            .as_ref()
            .is_some_and(|(sig, g, _)| *sig == filter_sig && *g == frame_gen);
        if !cache_hit {
            let visible_commits: Vec<crate::git_log::data::CommitRecord> = visible
                .iter()
                .map(|&i| frame.commits[i].clone())
                .collect();
            let lanes = crate::git_log::graph::layout(&visible_commits);
            state.filter_lane_cache = Some((filter_sig, frame_gen, lanes));
        }
        // Cheap clone of LaneFrame (it's small — Vec<usize> + lane
        // metadata, not commit data). Keeps the cache intact for
        // the next frame.
        state
            .filter_lane_cache
            .as_ref()
            .map(|(_, _, l)| l.clone())
    } else {
        // No filter: drop the cache to free its memory.
        state.filter_lane_cache = None;
        None
    };
    let lanes_ref: &crate::git_log::graph::LaneFrame =
        local_lanes.as_ref().unwrap_or(&frame.lanes);

    let max_lane = lanes_ref.max_lane.max(1) as f32;
    let graph_width = GRAPH_PAD_LEFT + (max_lane + 1.0) * COL_W;
    let total = visible.len();
    let meta_w = state.col_log_meta_width.clamp(120.0, 360.0);
    state.last_visible_count = total;

    // Auto-scroll target: when a fresh selection lands (e.g. user
    // clicked a branch in the refs panel which set selected_commit
    // and pending_scroll_to_selected), find the visible-row index
    // and ask the ScrollArea to scroll it into view.
    let scroll_to_visible_idx: Option<usize> = if state.pending_scroll_to_selected {
        state.pending_scroll_to_selected = false;
        state.selected_commit.as_ref().and_then(|sha| {
            visible
                .iter()
                .position(|&i| frame.commits[i].sha == *sha)
        })
    } else {
        None
    };

    let mut clicked_sha: Option<String> = None;
    let mut picked_op: Option<GitLogOp> = None;

    // Keyboard nav: arrow keys move the selection through the
    // currently visible (filtered) row list. Only fires when the log
    // column has focus — we approximate that by checking that no
    // egui widget currently holds keyboard focus (so Arrow keys
    // don't fight the filter TextEdit).
    let any_focus = ui.ctx().memory(|m| m.focused().is_some());
    if !any_focus && !visible.is_empty() {
        let cur_visible = state
            .selected_commit
            .as_ref()
            .and_then(|sha| {
                visible
                    .iter()
                    .position(|&i| frame.commits[i].sha == *sha)
            });
        let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::J));
        let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::K));
        if down || up {
            let next_idx = match cur_visible {
                Some(idx) if down => (idx + 1).min(visible.len() - 1),
                Some(idx) if up => idx.saturating_sub(1),
                None => 0,
                _ => 0,
            };
            state.selected_commit =
                Some(frame.commits[visible[next_idx]].sha.clone());
            state.selected_file = None;
        }
    }

    let mut scroll_area = egui::ScrollArea::vertical()
        .id_salt("git_log_commits")
        .auto_shrink([false, false]);
    if let Some(idx) = scroll_to_visible_idx {
        // Centre the target row in the viewport.
        let target_y = idx as f32 * ROW_H;
        scroll_area = scroll_area.vertical_scroll_offset(target_y);
    }
    scroll_area
        .show_rows(ui, ROW_H, total, |ui, range| {
            for vi in range {
                // `vi` indexes into the filtered `visible` slice; map
                // back to the canonical commit index for CommitRecord
                // lookup. Lane data comes from `lanes_ref` which is
                // either the canonical frame.lanes (no filter) or a
                // freshly-laid-out frame from just the visible
                // commits — in both cases `vi` is the right index.
                let i = visible[vi];
                let c = &frame.commits[i];
                let lane = lanes_ref.rows.get(vi);
                let next_lane = lanes_ref.rows.get(vi + 1);

                let row_resp = ui.allocate_response(
                    egui::vec2(ui.available_width(), ROW_H),
                    Sense::click(),
                );

                let is_selected = state.selected_commit.as_deref() == Some(c.sha.as_str());
                let theme_now = crate::theme::current();
                let bg = if is_selected {
                    theme_now.surface_hi.to_color32()
                } else if row_resp.hovered() {
                    theme_now.surface_alt.to_color32()
                } else {
                    Color32::TRANSPARENT
                };
                if bg != Color32::TRANSPARENT {
                    ui.painter().rect_filled(row_resp.rect, 0.0, bg);
                }

                // Graph painter (dots + parent connections).
                if let Some(lane_row) = lane {
                    paint_lane(ui, &row_resp.rect, lane_row, next_lane);
                }

                // Ref pills (HEAD / branches / tags) painted to the
                // left of the subject. Width is estimated from char
                // count (no fonts RwLock entry on the hot path — we
                // don't call layout_no_wrap inside show_rows, only
                // the top-level Painter::text path that's already in
                // use for the subject below).
                let mut text_x = row_resp.rect.left() + graph_width + 4.0;
                let text_y = row_resp.rect.top() + 4.0;
                let pills = parse_ref_pills(&c.refs_decoration, &frame.refs);
                let pill_font = egui::FontId::proportional(10.5);
                let pill_h = ROW_H - 8.0;
                for pill in &pills {
                    // Approx width: char count × monospace-ish factor.
                    // Slightly generous so the pill doesn't visually
                    // clip the label even with proportional chars.
                    let est_w = pill.label.chars().count() as f32 * 6.2 + 10.0;
                    let pill_rect = egui::Rect::from_min_size(
                        egui::pos2(text_x, row_resp.rect.top() + 4.0),
                        egui::vec2(est_w, pill_h),
                    );
                    ui.painter().rect_filled(pill_rect, 4.0, pill.bg);
                    ui.painter().text(
                        egui::pos2(pill_rect.center().x, pill_rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        &pill.label,
                        pill_font.clone(),
                        pill.fg,
                    );
                    text_x += est_w + 4.0;
                }

                ui.painter().text(
                    egui::pos2(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    &c.subject,
                    egui::FontId::proportional(12.5),
                    crate::theme::current().text.to_color32(),
                );

                let date_short = c.date.split('T').next().unwrap_or("");
                let meta = format!("{}  {}", c.author, date_short);
                let meta_x = row_resp.rect.right() - meta_w;
                if meta_x > text_x + 80.0 {
                    ui.painter().text(
                        egui::pos2(meta_x, text_y),
                        egui::Align2::LEFT_TOP,
                        &meta,
                        egui::FontId::proportional(11.5),
                        muted(),
                    );
                }

                if row_resp.clicked() {
                    clicked_sha = Some(c.sha.clone());
                }

                let row_sha = c.sha.clone();
                row_resp.context_menu(|ui| {
                    if ui
                        .button(format!("{}  Checkout this commit", icons::ARROW_RIGHT))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::Checkout(row_sha.clone()));
                        ui.close();
                    }
                    if ui
                        .button(format!("{}  Create branch from here…", icons::GIT_BRANCH))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::BranchFrom(row_sha.clone()));
                        ui.close();
                    }
                    if ui
                        .button(format!("{}  Create worktree from here…", icons::FOLDER_PLUS))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::WorktreeFrom(row_sha.clone()));
                        ui.close();
                    }
                    if ui
                        .button(format!("{}  Cherry-pick onto current", icons::GIT_DIFF))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::CherryPick(row_sha.clone()));
                        ui.close();
                    }
                    if ui
                        .button(format!("{}  Revert", icons::ARROW_COUNTER_CLOCKWISE))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::Revert(row_sha.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(format!("{}  Copy hash", icons::COPY))
                        .clicked()
                    {
                        picked_op = Some(GitLogOp::CopyHash(row_sha.clone()));
                        ui.close();
                    }
                });
            }
        });

    if let Some(sha) = clicked_sha {
        state.selected_commit = Some(sha);
        state.selected_file = None;
    }
    if let Some(op) = picked_op {
        state.pending_op = Some(op);
    }
}

/// Compact ComboBox styled to match the filter bar. Wraps the
/// standard ComboBox in a scoped visuals override so the trigger
/// reads as a flat pill on the bar's surface (no chunky brown
/// background, no oversized border). Active state tints the stroke
/// with the theme accent.
fn compact_combo<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    is_active: bool,
    theme: &crate::theme::Theme,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) {
    let radius = egui::CornerRadius::same(4);
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.bg_fill = theme.surface_alt.to_color32();
    visuals.widgets.inactive.weak_bg_fill = theme.surface_alt.to_color32();
    visuals.widgets.hovered.bg_fill = theme.surface_hi.to_color32();
    visuals.widgets.hovered.weak_bg_fill = theme.surface_hi.to_color32();
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    let stroke_col = if is_active {
        theme.accent.to_color32()
    } else {
        theme.divider.to_color32()
    };
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, stroke_col);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, stroke_col);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(
            egui::RichText::new(label)
                .size(12.0)
                .color(if is_active {
                    theme.text.to_color32()
                } else {
                    theme.text_muted.to_color32()
                }),
        )
        .show_ui(ui, add_contents);
}

/// Paint the dot for `lane_row` and connecting lines down to its
/// parents at `next_lane_row`'s level. Uses a quadratic Bezier for
/// off-axis parents to give branches a smooth curve.
fn paint_lane(
    ui: &egui::Ui,
    rect: &egui::Rect,
    lane_row: &crate::git_log::graph::LaneRow,
    next_lane_row: Option<&crate::git_log::graph::LaneRow>,
) {
    let color = PALETTE[(lane_row.color as usize) % PALETTE.len()];
    let dot_x = rect.left() + GRAPH_PAD_LEFT + (lane_row.own_lane as f32) * COL_W + COL_W * 0.5;
    let dot_y = rect.center().y;

    // Passthrough lanes: a vertical line spanning the full row in the
    // lane's branch-stable color. We extend each segment 1 px past the
    // row's top/bottom so adjacent rows' segments overlap — without
    // this, anti-aliasing between successive line_segment calls
    // leaves a 1 px sliver that reads as a dashed line. The bottom
    // extension is only safe when there IS a next row to bridge to;
    // on the last loaded row it spills into empty space below the
    // log, so we clamp the bottom to rect.bottom() exactly.
    let bottom_y = if next_lane_row.is_some() {
        rect.bottom() + 1.0
    } else {
        rect.bottom()
    };
    for &(pt_lane, pt_color) in &lane_row.passthrough_lanes {
        let pt_x = rect.left() + GRAPH_PAD_LEFT + (pt_lane as f32) * COL_W + COL_W * 0.5;
        let pt_color = PALETTE[(pt_color as usize) % PALETTE.len()];
        ui.painter().line_segment(
            [
                egui::pos2(pt_x, rect.top() - 1.0),
                egui::pos2(pt_x, bottom_y),
            ],
            egui::Stroke::new(1.5, pt_color),
        );
    }

    if let Some(next) = next_lane_row {
        let next_dot_y = dot_y + ROW_H;
        for &p_lane in &lane_row.parent_lanes {
            // Color of the line: whatever lane p_lane is on the next
            // row. Three cases:
            //   1. next is on lane p_lane (own_lane match) — use
            //      next's commit color (linear continuation).
            //   2. lane p_lane is a passthrough on next (it's alive
            //      through next but next is on a different lane) —
            //      use the passthrough's per-lane color from the
            //      layout.
            //   3. Neither — fall back to this row's color.
            //
            // The previous logic adopted next.color whenever next's
            // parent_lanes contained p_lane, which is wrong for
            // "merge into existing lane" rows: a red branch
            // terminating into the blue mainline made the mainline
            // segment above the terminator render red because next
            // was the red row whose first parent was lane 0.
            let next_color = if next.own_lane == p_lane {
                PALETTE[(next.color as usize) % PALETTE.len()]
            } else if let Some(&(_, c)) = next
                .passthrough_lanes
                .iter()
                .find(|(l, _)| *l == p_lane)
            {
                PALETTE[(c as usize) % PALETTE.len()]
            } else {
                color
            };
            let p_x = rect.left() + GRAPH_PAD_LEFT + (p_lane as f32) * COL_W + COL_W * 0.5;
            if p_lane == lane_row.own_lane {
                ui.painter().line_segment(
                    [
                        egui::pos2(dot_x, dot_y),
                        egui::pos2(p_x, next_dot_y),
                    ],
                    egui::Stroke::new(1.5, next_color),
                );
            } else {
                // The bezier ends at the next dot's center. Anti-
                // aliasing on the bezier's endpoint plus the filled
                // dot drawn on top leaves a 1–2 px gap where the curve
                // visually fails to meet the lane below — the user
                // sees the branch "not touching" the lane it merges
                // into. Extend the bezier endpoint ~DOT_R past the
                // dot center so the curve visibly enters the dot,
                // then the filled dot covers the overshoot cleanly.
                let mid_y = dot_y + ROW_H * 0.5;
                let cp = egui::pos2(p_x, mid_y);
                let bezier = egui::epaint::QuadraticBezierShape {
                    points: [
                        egui::pos2(dot_x, dot_y),
                        cp,
                        egui::pos2(p_x, next_dot_y + DOT_R),
                    ],
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: egui::Stroke::new(1.5, next_color).into(),
                };
                ui.painter().add(bezier);

                // Anchor segment: short vertical line at the merge
                // lane from the next row's top edge to the dot
                // center. Guarantees a visible connection even when
                // the next row renders its dot WITHOUT a passthrough
                // (which is the "merge into existing lane" case).
                ui.painter().line_segment(
                    [
                        egui::pos2(p_x, next_dot_y - ROW_H * 0.5),
                        egui::pos2(p_x, next_dot_y),
                    ],
                    egui::Stroke::new(1.5, next_color),
                );
            }
        }
    }

    // Lane caps for branches that terminate at this row.
    for &term in &lane_row.terminating_lanes {
        let t_x = rect.left() + GRAPH_PAD_LEFT + (term as f32) * COL_W + COL_W * 0.5;
        ui.painter().circle_stroke(
            egui::pos2(t_x, rect.top() + 2.0),
            DOT_R - 1.0,
            egui::Stroke::new(1.0, muted()),
        );
    }

    // Dot for this commit — drawn LAST so it sits on top of incoming
    // lines from the row above.
    ui.painter().circle_filled(egui::pos2(dot_x, dot_y), DOT_R, color);
}


use egui::{Color32, Sense};
use egui_phosphor::regular as icons;

use crate::git_log::state::{GitLogOp, GitLogState};
use crate::ui::util::muted;

const ROW_H: f32 = 22.0;
const COL_W: f32 = 14.0;
const DOT_R: f32 = 4.0;
const GRAPH_PAD_LEFT: f32 = 8.0;

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

    // Filter bar — single row above the commit list.
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let filter_id = egui::Id::new("git_log_filter_text");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.filter.text)
                .hint_text("filter subject / hash / author")
                .id(filter_id)
                .desired_width(220.0),
        );
        if state.pending_focus_filter {
            resp.request_focus();
            state.pending_focus_filter = false;
        }

        // Branch facet (built from local refs).
        let local_branches: Vec<String> = state
            .frame
            .as_ref()
            .map(|f| {
                f.refs
                    .local
                    .iter()
                    .map(|r| r.name.trim_start_matches("refs/heads/").to_string())
                    .collect()
            })
            .unwrap_or_default();
        let branch_label = state
            .filter
            .branch
            .clone()
            .unwrap_or_else(|| "branch".to_string());
        egui::ComboBox::from_id_salt("git_log_branch_filter")
            .selected_text(branch_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.filter.branch, None, "all branches");
                for b in &local_branches {
                    ui.selectable_value(&mut state.filter.branch, Some(b.clone()), b);
                }
            });

        // User facet (unique authors).
        let mut authors: Vec<String> = state
            .frame
            .as_ref()
            .map(|f| f.commits.iter().map(|c| c.author.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        authors.sort();
        authors.dedup();
        let user_label = state
            .filter
            .user
            .clone()
            .unwrap_or_else(|| "user".to_string());
        egui::ComboBox::from_id_salt("git_log_user_filter")
            .selected_text(user_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.filter.user, None, "all users");
                for u in &authors {
                    ui.selectable_value(&mut state.filter.user, Some(u.clone()), u);
                }
            });
    });
    ui.separator();

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
        let visible_commits: Vec<crate::git_log::data::CommitRecord> = visible
            .iter()
            .map(|&i| frame.commits[i].clone())
            .collect();
        Some(crate::git_log::graph::layout(&visible_commits))
    } else {
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

                // Subject + metadata.
                let text_x = row_resp.rect.left() + graph_width + 4.0;
                let text_y = row_resp.rect.top() + 4.0;

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
            // Use the next row's color where the parent will continue.
            let next_color = if (next.own_lane == p_lane)
                || next.parent_lanes.iter().any(|&l| l == p_lane)
            {
                PALETTE[(next.color as usize) % PALETTE.len()]
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
                let mid_y = dot_y + ROW_H * 0.5;
                let cp = egui::pos2(p_x, mid_y);
                let bezier = egui::epaint::QuadraticBezierShape {
                    points: [
                        egui::pos2(dot_x, dot_y),
                        cp,
                        egui::pos2(p_x, next_dot_y),
                    ],
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: egui::Stroke::new(1.5, next_color).into(),
                };
                ui.painter().add(bezier);
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


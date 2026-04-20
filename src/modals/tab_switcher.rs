//! Cmd+~ tab switcher — the alt+tab-equivalent for Crane's
//! project/workspace/tab tree. Hold Cmd, tap ~ to cycle forward
//! (Shift+~ backward), release Cmd to commit. Escape cancels.
//!
//! Cycle-open path (not a lingering search modal) is the whole point;
//! a single tap of Cmd+~ lands on the previously-focused tab, mimicking
//! alt+tab's muscle-memory "quick back-and-forth" behavior.

use crate::state::{App, TabSwitcherState};
use egui::RichText;

/// Open the switcher or advance the highlight if it's already open.
/// Returns `true` if the caller should suppress further shortcut
/// handling this frame (so Cmd+~ doesn't also close the settings
/// modal or similar).
pub fn advance_or_open(app: &mut App, backward: bool) -> bool {
    let entries = collect_entries(app);
    if entries.len() < 2 {
        return false;
    }
    match app.tab_switcher.as_mut() {
        None => {
            // First tap: open with highlight on "previous" tab — index
            // 1 in MRU order (index 0 is the current). That way a
            // single tap + release flips you to the prior tab.
            let highlight = if entries.len() > 1 { 1 } else { 0 };
            app.tab_switcher = Some(TabSwitcherState {
                entries,
                highlight,
                cmd_was_held: true,
            });
        }
        Some(state) => {
            let len = state.entries.len();
            if len == 0 {
                return false;
            }
            state.highlight = if backward {
                (state.highlight + len - 1) % len
            } else {
                (state.highlight + 1) % len
            };
        }
    }
    true
}

/// Render the overlay + commit-on-Cmd-release. Returns `true` when a
/// commit happened this frame (so the caller can skip other key work).
pub fn render(ctx: &egui::Context, app: &mut App) -> bool {
    let Some(state) = app.tab_switcher.as_ref() else {
        return false;
    };
    // Prefer the NSEvent-sourced Cmd flag on macOS — egui's own
    // modifier state can miss a release when the frame loop is idle.
    // Keep the egui check as a fallback on other platforms.
    #[cfg(target_os = "macos")]
    let cmd_held = crate::mac_keys::is_cmd_held();
    #[cfg(not(target_os = "macos"))]
    let cmd_held = ctx.input(|i| i.modifiers.mac_cmd || i.modifiers.command);
    // Consume so Escape doesn't leak past the overlay into the
    // terminal (where \x1b would cancel whatever's running).
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    // Keep the frame loop ticking while the overlay is open so Cmd
    // release is observed promptly even if the user isn't moving the
    // mouse.
    ctx.request_repaint_after(std::time::Duration::from_millis(30));

    // Snapshot what we need for rendering before mutating state below.
    let entries = state.entries.clone();
    let highlight = state.highlight;
    let was_held = state.cmd_was_held;

    // Build labels "project / workspace / tab" off the current App.
    let labels: Vec<String> = entries
        .iter()
        .map(|(pid, wid, tid)| label_for(app, *pid, *wid, *tid))
        .collect();

    let mut clicked: Option<usize> = None;
    egui::Window::new("Tab switcher")
        .id(egui::Id::new("crane_tab_switcher"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .inner_margin(egui::Margin::symmetric(14, 10)),
        )
        .show(ctx, |ui| {
            ui.set_min_width(460.0);
            ui.label(
                RichText::new("Switch tab")
                    .size(11.0)
                    .color(crate::theme::current().text_muted.to_color32()),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(340.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        let is_hl = i == highlight;
                        let bg = if is_hl {
                            crate::theme::current().row_active.to_color32()
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 22.0),
                            egui::Sense::click(),
                        );
                        if bg != egui::Color32::TRANSPARENT {
                            ui.painter().rect_filled(rect, 4.0, bg);
                        }
                        ui.painter().text(
                            egui::pos2(rect.min.x + 8.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::new(
                                12.0,
                                egui::FontFamily::Proportional,
                            ),
                            crate::theme::current().text.to_color32(),
                        );
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Cmd+` next · Cmd+~ previous · release Cmd to commit · Esc cancel",
                )
                .size(10.5)
                .color(crate::theme::current().text_muted.to_color32()),
            );
        });

    // Resolve close conditions in order: Esc > click > Cmd release.
    if esc {
        app.tab_switcher = None;
        return true;
    }
    if let Some(i) = clicked {
        commit(app, i);
        return true;
    }
    if was_held && !cmd_held {
        commit(app, highlight);
        return true;
    }
    // Keep state's cmd_was_held up to date for the next frame.
    if let Some(state) = app.tab_switcher.as_mut() {
        state.cmd_was_held = cmd_held;
    }
    false
}

fn commit(app: &mut App, idx: usize) {
    if let Some(state) = app.tab_switcher.take()
        && let Some(target) = state.entries.get(idx).copied()
    {
        // Ensure the target still exists — the tab could have been
        // closed while the overlay was open (unlikely but possible
        // via another keybinding).
        if app
            .projects
            .iter()
            .find(|p| p.id == target.0)
            .and_then(|p| p.workspaces.iter().find(|w| w.id == target.1))
            .and_then(|w| w.tabs.iter().find(|t| t.id == target.2))
            .is_some()
        {
            app.active = Some(target);
            app.last_workspace = Some((target.0, target.1));
            if let Some(p) = app.projects.iter_mut().find(|p| p.id == target.0) {
                p.last_active_workspace = Some(target.1);
            }
            if let Some(w) = app
                .projects
                .iter_mut()
                .find(|p| p.id == target.0)
                .and_then(|p| p.workspaces.iter_mut().find(|w| w.id == target.1))
            {
                w.active_tab = Some(target.2);
            }
        }
    }
}

fn collect_entries(app: &App) -> Vec<(crate::state::ProjectId, crate::state::WorkspaceId, crate::state::TabId)> {
    // Start with MRU, filter to live tabs, then append any tabs not yet
    // in MRU (newly created, never focused). This keeps recent tabs at
    // the top while still exposing everything.
    let mut out: Vec<_> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &e in &app.tab_mru {
        if live(app, e) && seen.insert(e) {
            out.push(e);
        }
    }
    for p in &app.projects {
        for w in &p.workspaces {
            for t in &w.tabs {
                let e = (p.id, w.id, t.id);
                if seen.insert(e) {
                    out.push(e);
                }
            }
        }
    }
    out
}

fn live(
    app: &App,
    (pid, wid, tid): (crate::state::ProjectId, crate::state::WorkspaceId, crate::state::TabId),
) -> bool {
    app.projects
        .iter()
        .find(|p| p.id == pid)
        .and_then(|p| p.workspaces.iter().find(|w| w.id == wid))
        .and_then(|w| w.tabs.iter().find(|t| t.id == tid))
        .is_some()
}

fn label_for(
    app: &App,
    pid: crate::state::ProjectId,
    wid: crate::state::WorkspaceId,
    tid: crate::state::TabId,
) -> String {
    let project_name = app
        .projects
        .iter()
        .find(|p| p.id == pid)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let (ws_label, tab_name) = app
        .projects
        .iter()
        .find(|p| p.id == pid)
        .and_then(|p| p.workspaces.iter().find(|w| w.id == wid))
        .map(|w| {
            let ws = w.label();
            let tab = w
                .tabs
                .iter()
                .find(|t| t.id == tid)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            (ws, tab)
        })
        .unwrap_or_default();
    format!("{project_name} / {ws_label} / {tab_name}")
}

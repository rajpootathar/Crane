//! Global keyboard shortcut dispatch — extracted from
//! `main.rs::CraneApp::handle_shortcuts`. Runs once per frame before
//! panels render, so split/close/focus intents take effect the same
//! frame they're pressed.

use crate::state::App;
use crate::state::layout::{Dir, PaneId};

pub fn handle(
    ctx: &egui::Context,
    app: &mut App,
    pending_close: &mut Option<PaneId>,
) {
    // When any modal is open, Cmd+W closes the modal instead of the
    // pane underneath. Also absorb Escape. Everything else falls
    // through so Cmd+S etc. still work inside modals.
    //
    // Consume the key (not just peek) — otherwise the raw keypress
    // leaks down to the terminal view's event handler, which turns
    // Escape into `\x1b` (killing e.g. a running `claude` CLI) and
    // Cmd+W into a literal character, all while the user thinks they
    // just dismissed a modal.
    let modal_open = app.show_settings
        || app.show_help
        || app.new_workspace_modal.is_some()
        || app.pending_remove_worktree.is_some()
        || app.pending_close_tab.is_some()
        || !app.missing_project_modals.is_empty()
        || pending_close.is_some();
    if modal_open {
        let (cmd_w, esc) = ctx.input_mut(|i| {
            let cmd_w = i.consume_key(egui::Modifiers::COMMAND, egui::Key::W)
                || i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::W);
            let esc = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            (cmd_w, esc)
        });
        if cmd_w || esc {
            if app.show_settings {
                app.show_settings = false;
            }
            if app.show_help {
                app.show_help = false;
            }
            if esc && app.new_workspace_modal.is_some() {
                app.new_workspace_modal = None;
            }
            if esc && app.pending_remove_worktree.is_some() {
                app.pending_remove_worktree = None;
            }
            if esc && app.pending_close_tab.is_some() {
                app.pending_close_tab = None;
            }
            if esc && !app.missing_project_modals.is_empty() {
                app.missing_project_modals.clear();
            }
            if esc && pending_close.is_some() {
                *pending_close = None;
            }
            return;
        }
    }

    let (split_terminal, new_tab, split_h, split_v, close_pane, next_pane, prev_pane,
         zoom_in, zoom_out, zoom_reset, toggle_left, toggle_right, close_tab) =
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            (
                cmd && !shift && i.key_pressed(egui::Key::T),
                cmd && shift && i.key_pressed(egui::Key::T),
                cmd && !shift && i.key_pressed(egui::Key::D),
                cmd && shift && i.key_pressed(egui::Key::D),
                cmd && !shift && i.key_pressed(egui::Key::W),
                cmd && i.key_pressed(egui::Key::CloseBracket),
                cmd && i.key_pressed(egui::Key::OpenBracket),
                cmd && (i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)),
                cmd && i.key_pressed(egui::Key::Minus),
                cmd && i.key_pressed(egui::Key::Num0),
                cmd && i.key_pressed(egui::Key::B),
                cmd && i.key_pressed(egui::Key::Slash),
                cmd && shift && i.key_pressed(egui::Key::W),
            )
        });

    if split_terminal
        && let Some(ws) = app.active_layout()
    {
        ws.split_focused_with_terminal(ctx, Dir::Horizontal);
    }
    if new_tab {
        app.new_tab_in_active_workspace(ctx);
    }
    if split_h
        && let Some(ws) = app.active_layout()
    {
        ws.split_focused_with_terminal(ctx, Dir::Horizontal);
    }
    if split_v
        && let Some(ws) = app.active_layout()
    {
        ws.split_focused_with_terminal(ctx, Dir::Vertical);
    }
    if close_pane {
        let focus = app.active_layout_ref().and_then(|l| l.focus);
        if let Some(id) = focus {
            // Files pane with multiple tabs: Cmd+W closes the active
            // tab instead of the whole pane. Last-tab close still
            // falls through to close the pane itself — user's
            // expectation.
            let closed_file_tab = if let Some(ws) = app.active_layout()
                && let Some(pane) = ws.panes.get_mut(&id)
                && let crate::state::layout::PaneContent::Files(files) = &mut pane.content
                && files.tabs.len() > 1
            {
                let idx = files.active.min(files.tabs.len() - 1);
                files.close(idx);
                true
            } else {
                false
            };
            if !closed_file_tab {
                if terminal_is_running(app, id) {
                    *pending_close = Some(id);
                } else if let Some(ws) = app.active_layout() {
                    ws.close_focused();
                }
            }
        }
    }
    if close_tab {
        app.close_active_tab();
    }
    if next_pane
        && let Some(ws) = app.active_layout()
    {
        ws.focus_next();
    }
    if prev_pane
        && let Some(ws) = app.active_layout()
    {
        ws.focus_prev();
    }
    if zoom_in {
        app.font_size = (app.font_size + 1.0).min(40.0);
    }
    if zoom_out {
        app.font_size = (app.font_size - 1.0).max(8.0);
    }
    if zoom_reset {
        app.font_size = 14.0;
    }
    if toggle_left {
        app.show_left = !app.show_left;
    }
    if toggle_right {
        app.show_right = !app.show_right;
    }
}

fn terminal_is_running(app: &App, id: PaneId) -> bool {
    let Some(layout) = app.active_layout_ref() else {
        return false;
    };
    let Some(pane) = layout.panes.get(&id) else {
        return false;
    };
    match &pane.content {
        crate::state::layout::PaneContent::Terminal(t) => t.has_foreground_process(),
        _ => false,
    }
}

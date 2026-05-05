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
        || app.pending_delete_file.is_some()
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
            if esc && app.pending_delete_file.is_some() {
                app.pending_delete_file = None;
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
         zoom_in, zoom_out, zoom_reset, toggle_left, toggle_right, close_tab,
         browser_new_tab) =
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let alt = i.modifiers.alt;
            (
                cmd && !shift && !alt && i.key_pressed(egui::Key::T),
                cmd && shift && !alt && i.key_pressed(egui::Key::T),
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
                // Cmd+Option+T — add a tab to the focused Browser pane.
                // Cmd+T alone is reserved for new-terminal-split, so
                // Option is the escape hatch to disambiguate. No-op
                // when the focused pane isn't a Browser; user focuses
                // the browser first (click or Cmd+[ / Cmd+]).
                cmd && alt && !shift && i.key_pressed(egui::Key::T),
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
    if browser_new_tab
        && let Some(ws) = app.active_layout()
        && let Some(focus) = ws.focus
        && let Some(pane) = ws.panes.get_mut(&focus)
        && let crate::state::layout::PaneContent::Browser(browser) = &mut pane.content
    {
        browser.new_tab();
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

    // Whether any widget currently holds keyboard focus — used to
    // guard shortcuts that would otherwise steal keystrokes from a
    // TextEdit or terminal pane.
    let any_focus = ctx.memory(|m| m.focused().is_some());
    if toggle_left && !any_focus {
        app.show_left = !app.show_left;
    }
    if toggle_right && !any_focus {
        app.show_right = !app.show_right;
    }
    let delete_selected = ctx.input(|i| {
        let cmd = i.modifiers.command;
        cmd && i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)
    });
    if delete_selected && !any_focus
        && let Some(path) = app.selected_file.clone()
    {
        // Stage a confirm modal — the user always sees what they're
        // about to discard. The modal handles `trash::delete`, the
        // file-op history push, and tab cleanup on confirm.
        app.pending_delete_file = Some(crate::state::PendingDeleteFile { path });
    }

    // Cmd+Z: undo the most recent Files-Pane move/trash. Same focus
    // guard as Cmd+Backspace so we don't steal undo from a TextEdit
    // or terminal pane that's editing text.
    let undo_pressed = ctx.input(|i| {
        i.modifiers.command
            && !i.modifiers.shift
            && i.key_pressed(egui::Key::Z)
    });
    if undo_pressed && !any_focus {
        app.undo_last_file_op();
    }

    // Cmd+O: open external file via native file picker
    let open_file = ctx.input_mut(|i| {
        let pressed = (i.modifiers.command || i.modifiers.mac_cmd)
            && i.key_pressed(egui::Key::O)
            && !i.modifiers.shift;
        if pressed {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::O);
            i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::O);
        }
        pressed
    });
    if open_file {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            app.open_external_file(ctx, &path);
        }
    }

    // Cmd+Shift+O: open folder as project workspace
    let open_folder = ctx.input_mut(|i| {
        let pressed = (i.modifiers.command || i.modifiers.mac_cmd)
            && i.modifiers.shift
            && i.key_pressed(egui::Key::O);
        if pressed {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::O);
            i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::O);
        }
        pressed
    });
    if open_folder {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            app.add_project_from_path(path, ctx);
        }
    }

    // Cmd+9: toggle the Git Log bottom-docked Pane on the active Tab.
    // Matches IntelliJ's Git tool-window binding so users coming from
    // there have muscle memory.
    let toggle_log = ctx.input_mut(|i| {
        let pressed = (i.modifiers.command || i.modifiers.mac_cmd)
            && i.key_pressed(egui::Key::Num9)
            && !i.modifiers.shift;
        if pressed {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num9);
            i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::Num9);
        }
        pressed
    });
    if toggle_log {
        app.toggle_git_log(ctx);
    }

    // Cmd+F: focus the Git Log filter TextEdit when the pane is open.
    // Only fires when no widget currently holds focus so it doesn't
    // steal Cmd+F from the system Find menu in editors etc.
    let focus_log_filter = ctx.input_mut(|i| {
        let pressed = (i.modifiers.command || i.modifiers.mac_cmd)
            && i.key_pressed(egui::Key::F)
            && !i.modifiers.shift
            && !i.modifiers.alt;
        let already_focused = ctx.memory(|m| m.focused().is_some());
        if pressed && !already_focused {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::F);
            i.consume_key(egui::Modifiers::MAC_CMD, egui::Key::F);
            true
        } else {
            false
        }
    });
    if focus_log_filter {
        if let Some(state) = app
            .active_tab_mut()
            .and_then(|t| t.git_log_state.as_mut())
        {
            state.pending_focus_filter = true;
        }
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
        crate::state::layout::PaneContent::Terminal(t) => {
            t.tabs.iter().any(|x| x.terminal.has_foreground_process())
        }
        _ => false,
    }
}

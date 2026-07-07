//! Native macOS application menu bar for the warpui frontend.
//!
//! Unlike the egui path (`src/platform_menu.rs`, which used `muda` +
//! per-frame `drain_events()` polling), warpui exposes a first-class menu API
//! (`warpui::platform::menu`) whose item callbacks receive an `&mut AppContext`.
//! That lets each menu item reach the live `CraneShellView` through the shared
//! `ShellSlot` and drive it exactly like an in-app shortcut — no polling loop.
//!
//! Every custom item dispatches an existing [`CraneShellAction`] via
//! `CraneShellView::handle_action`, so the menu is a thin native shell over the
//! same action pipeline the keyboard already uses. Quit routes through the OS
//! terminate path (`AppContext::terminate_app(Cancellable)`) so the running
//! `on_should_terminate_app` guard can raise the ConfirmQuit modal.
//!
//! Linux / Windows get a no-op `install` — those platforms already have the
//! in-app Settings / Help buttons in the status bar.

use std::cell::RefCell;
use std::rc::Rc;

use warpui::{ViewHandle, WindowId};

use crate::warpui::shell::CraneShellView;

/// Shared slot filled once the root view exists (see `mod.rs::run`). Reused here
/// so menu-item callbacks can reach the shell on the main thread.
pub type ShellSlot = Rc<RefCell<Option<(WindowId, ViewHandle<CraneShellView>)>>>;

/// Install the native application menu. Must be called on the `AppBuilder`
/// before `run()` (the builder stores the menu-bar constructor and invokes it
/// at launch). No-op off macOS.
#[cfg(target_os = "macos")]
pub fn install(builder: &mut warpui::platform::AppBuilder, shell: ShellSlot) {
    use warpui::platform::mac::AppExt;
    builder.set_menu_bar_builder(move |_ctx| mac::build_menu_bar(&shell));
}

#[cfg(not(target_os = "macos"))]
pub fn install(_builder: &mut warpui::platform::AppBuilder, _shell: ShellSlot) {}

#[cfg(target_os = "macos")]
mod mac {
    use super::ShellSlot;
    use crate::warpui::shell::CraneShellAction;

    use warpui::keymap::Keystroke;
    use warpui::platform::menu::{
        CustomMenuItem, Menu, MenuBar, MenuItem, MenuItemProperties, MenuItemPropertyChanges,
    };
    use warpui::platform::TerminationMode;
    use warpui::{AppContext, TypedActionView};

    /// Build a `Keystroke` directly (avoids `Keystroke::parse`, whose debug build
    /// panics on `shift` + a lowercase letter). warpui delivers Cmd+Shift+T as
    /// `key = "t"` + `shift = true`, so we mirror that convention — lowercase
    /// key, explicit modifier flags — which AppKit renders as `⇧⌘T`.
    fn ks(cmd: bool, shift: bool, key: &str) -> Keystroke {
        Keystroke {
            cmd,
            shift,
            key: key.to_string(),
            ..Default::default()
        }
    }

    /// Updater used by every item — none of ours change title/state at runtime.
    fn no_updates(_props: &MenuItemProperties, _app: &mut AppContext) -> MenuItemPropertyChanges {
        MenuItemPropertyChanges::default()
    }

    /// Reach the live `CraneShellView` through the slot and run `action` down the
    /// normal `handle_action` pipeline (same path the keymap uses). Cloning the
    /// handle drops the `RefCell` borrow before the re-entrant `update`.
    fn dispatch(shell: &ShellSlot, app: &mut AppContext, action: CraneShellAction) {
        let handle = shell.borrow().as_ref().map(|(_, h)| h.clone());
        if let Some(handle) = handle {
            handle.update(app, |view, vctx| view.handle_action(&action, vctx));
        }
    }

    /// A custom item that dispatches `action`. `key` is optional — items whose
    /// chord is handled contextually in-app (e.g. Cmd+/) omit it so the menu
    /// doesn't hijack the keystroke off the main menu.
    fn item(
        shell: &ShellSlot,
        name: &str,
        key: Option<Keystroke>,
        action: CraneShellAction,
    ) -> MenuItem {
        let shell = shell.clone();
        MenuItem::Custom(CustomMenuItem::new(
            name,
            move |app| dispatch(&shell, app, action.clone()),
            no_updates,
            key,
        ))
    }

    /// Assemble the full menu bar. Each entry documents the existing action it
    /// dispatches.
    pub fn build_menu_bar(shell: &ShellSlot) -> MenuBar {
        // Crane
        let app_menu = Menu::new(
            "Crane",
            vec![
                // About Crane → open the Settings modal (About lives there).
                item(
                    shell,
                    "About Crane",
                    None,
                    CraneShellAction::OpenSettings,
                ),
                MenuItem::Separator,
                item(
                    shell,
                    "Settings…",
                    Some(ks(true, false, ",")),
                    CraneShellAction::OpenSettings,
                ),
                MenuItem::Separator,
                // Custom Quit (not StandardAction::Quit, whose label is hardcoded
                // "Quit Warp"). Cancellable termination triggers
                // applicationShouldTerminate → on_should_terminate_app → the
                // ConfirmQuit guard wired in mod.rs::run.
                {
                    MenuItem::Custom(CustomMenuItem::new(
                        "Quit Crane",
                        move |app| app.terminate_app(TerminationMode::Cancellable, None),
                        no_updates,
                        Some(ks(true, false, "q")),
                    ))
                },
            ],
        );

        // File
        let file_menu = Menu::new(
            "File",
            vec![
                item(
                    shell,
                    "New Tab",
                    Some(ks(true, true, "t")),
                    CraneShellAction::NewTab,
                ),
                MenuItem::Separator,
                item(
                    shell,
                    "Open File…",
                    Some(ks(true, false, "o")),
                    CraneShellAction::OpenExternalFile,
                ),
                item(
                    shell,
                    "Add Project…",
                    Some(ks(true, true, "o")),
                    CraneShellAction::AddProject,
                ),
            ],
        );

        // View
        let view_menu = Menu::new(
            "View",
            vec![
                item(
                    shell,
                    "Toggle Left Panel",
                    Some(ks(true, false, "b")),
                    CraneShellAction::ToggleLeft,
                ),
                // No Cmd+/ key equivalent: in-app Cmd+/ is contextual
                // (CommentOrToggleRight — comment the line in an editor, else
                // toggle the Right Panel). Registering it here would hijack the
                // chord off the main menu and break editor line-commenting, so
                // the item toggles the Right Panel on click only.
                item(
                    shell,
                    "Toggle Right Panel",
                    None,
                    CraneShellAction::ToggleRight,
                ),
                MenuItem::Separator,
                item(
                    shell,
                    "Zoom In",
                    Some(ks(true, false, "=")),
                    CraneShellAction::FontZoomIn,
                ),
                item(
                    shell,
                    "Zoom Out",
                    Some(ks(true, false, "-")),
                    CraneShellAction::FontZoomOut,
                ),
                item(
                    shell,
                    "Reset Zoom",
                    Some(ks(true, false, "0")),
                    CraneShellAction::FontZoomReset,
                ),
            ],
        );

        // Help
        let help_menu = Menu::new(
            "Help",
            vec![item(
                shell,
                "Keyboard Shortcuts",
                None,
                CraneShellAction::OpenHelp,
            )],
        );

        MenuBar::new(vec![app_menu, file_menu, view_menu, help_menu])
    }
}

//! Native macOS application menu. Exposes `Crane → Settings…` and a
//! Help submenu with our Keyboard Shortcuts action. The egui layer
//! polls `drain_events()` once per frame and flips the corresponding
//! flags on `App`.
//!
//! Linux / Windows don't get a native menu — they already have the
//! in-app Settings + Help buttons in the status bar.

#[cfg(target_os = "macos")]
mod mac {
    use muda::{AboutMetadata, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Stable ids we match against in the poll loop.
    pub const ID_SETTINGS: &str = "crane.settings";
    pub const ID_SHORTCUTS: &str = "crane.shortcuts";
    pub const ID_CHECK_UPDATES: &str = "crane.check_updates";

    // muda::Menu wraps an Rc internally and isn't Sync — can't live in
    // a static. We Box::leak after init (menu must outlive the app
    // anyway) and use a plain AtomicBool to guard idempotency.
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    /// Install the application menu on macOS. Idempotent — the
    /// installed Menu lives for the app lifetime (leaked intentionally
    /// so NSApp keeps its callbacks).
    pub fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        let menu = Menu::new();

        let app_submenu = Submenu::new("Crane", true);
        let _ = app_submenu.append_items(&[
            &PredefinedMenuItem::about(
                Some("About Crane"),
                Some(AboutMetadata {
                    name: Some("Crane".into()),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    ..Default::default()
                }),
            ),
            &MenuItem::with_id(
                MenuId::new(ID_CHECK_UPDATES),
                "Check for Updates…",
                true,
                None,
            ),
            &PredefinedMenuItem::separator(),
            // `Settings…` with the canonical Cmd+, shortcut so macOS
            // users find it instinctively.
            &MenuItem::with_id(
                MenuId::new(ID_SETTINGS),
                "Settings…",
                true,
                Some(muda::accelerator::Accelerator::new(
                    Some(muda::accelerator::Modifiers::SUPER),
                    muda::accelerator::Code::Comma,
                )),
            ),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ]);

        // Edit submenu with standard clipboard accelerators. Required
        // for the embedded WKWebView — without menu items wired to
        // `copy:` / `paste:` / `cut:` / `selectAll:`, AppKit's
        // responder chain has nothing to route Cmd+C/V/X/A to, and the
        // webview silently swallows the keys. PredefinedMenuItem uses
        // the standard AppKit actions so the focused NSView (WKWebView
        // when the browser pane is focused) handles them natively.
        //
        // Trade-off: in pure-egui panes (Terminal, modals) these
        // shortcuts now go through AppKit first. egui's terminal Cmd+C
        // (copy selection) uses that same key, but because the egui
        // window's content view doesn't implement `copy:`, the menu
        // item is disabled when no webview has focus and the key
        // falls through to winit/egui as a regular key event — so
        // terminal selection copy still works. If that assumption
        // turns out to be wrong in practice, the fix is to intercept
        // the menu action ourselves and route by focused-pane type.
        let edit = Submenu::new("Edit", true);
        let _ = edit.append_items(&[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ]);

        let window = Submenu::new("Window", true);
        let _ = window.append_items(&[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::fullscreen(None),
        ]);

        let help = Submenu::new("Help", true);
        let _ = help.append_items(&[&MenuItem::with_id(
            MenuId::new(ID_SHORTCUTS),
            "Keyboard Shortcuts",
            true,
            None,
        )]);

        let _ = menu.append_items(&[&app_submenu, &edit, &window, &help]);
        menu.init_for_nsapp();
        // Intentionally leak: NSApp holds a weak-ish reference to the
        // menu via init_for_nsapp, and muda's Menu Drop would tear
        // down the registration.
        Box::leak(Box::new(menu));
    }

    /// Drain any pending menu events accumulated since the last call.
    /// Returns a list of ids that fired; main's render loop matches
    /// against `ID_SETTINGS` / `ID_SHORTCUTS` to toggle modals.
    pub fn drain_events() -> Vec<String> {
        let rx = muda::MenuEvent::receiver();
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev.id.0);
        }
        out
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(not(target_os = "macos"))]
pub fn drain_events() -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
pub const ID_SETTINGS: &str = "crane.settings";

#[cfg(not(target_os = "macos"))]
pub const ID_SHORTCUTS: &str = "crane.shortcuts";

#[cfg(not(target_os = "macos"))]
pub const ID_CHECK_UPDATES: &str = "crane.check_updates";

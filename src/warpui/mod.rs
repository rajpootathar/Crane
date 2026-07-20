//! warpui frontend for Crane — the GPU-rendered UI and Crane's sole frontend.
//! It reuses the rest of the `crane` crate's logic (`crate::git`,
//! `crate::lsp`, `crate::theme`, `crate::format`, `crate::syntax`, …) and owns
//! its own NSApplication / event loop. Launched unconditionally from `main()`;
//! the legacy egui frontend has been removed.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use warpui::geometry::vector::vec2f;
use warpui::{platform, AddWindowOptions, AssetProvider};

pub mod browser;
pub mod browser_view;
pub mod bundled_fonts;
pub mod color;
pub mod controller;
pub mod diff_view;
pub mod editor_view;
pub mod file_pane;
pub mod file_watcher;
pub mod find_bar_element;
pub mod file_tree;
pub mod formatter;
pub mod fontsize;
pub mod git;
pub mod git_log;
pub mod git_log_element;
pub mod markdown_view;
pub mod grid_element;
pub mod gutter_element;
pub mod scrollbar_element;
pub mod history_store;
pub mod icons;
pub mod image_view;
pub mod input;
pub mod layout;
pub mod line_edit;
pub mod persist;
pub mod platform_menu;
pub mod projects;
pub mod rect_probe;
pub mod shell;
pub mod shell_init;
pub mod split;
pub mod theme;
pub mod update;
pub mod view;
pub mod welcome_view;

use shell::CraneShellView;

#[derive(Clone, Copy, RustEmbed)]
#[folder = "src/warpui/assets"]
pub struct Assets;

pub static ASSETS: Assets = Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|f| f.data)
            .ok_or_else(|| anyhow!("no asset exists at path {}", path))
    }
}

/// Set the macOS Dock / app icon from the bundled `crane.png`. warpui exposes no
/// window-icon API and a bare `cargo run` binary isn't an `.app` bundle, so the
/// Dock shows a generic icon; set it directly on `NSApplication` at startup.
#[cfg(target_os = "macos")]
fn set_app_icon() {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bytes: &[u8] = include_bytes!("../../crane.png");
    let data = NSData::with_bytes(bytes);
    unsafe {
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let app = NSApplication::sharedApplication(mtm);
            app.setApplicationIconImage(Some(&image));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn set_app_icon() {}

/// Run the warpui frontend (owns its own NSApplication/event loop).
pub fn run() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use warpui::platform::app::ApproveTerminateResult;
    use warpui::{AppContext, ViewHandle, WindowId};

    // Shared slot the init closure fills once the root view exists; the OS
    // terminate / close-window guards read it to reach the CraneShellView so a
    // quit with a running terminal pops the ConfirmQuit modal instead of tearing
    // everything down. Everything runs on the main thread — Rc<RefCell> is safe.
    type ShellSlot = Rc<RefCell<Option<(WindowId, ViewHandle<CraneShellView>)>>>;
    let shell: ShellSlot = Rc::new(RefCell::new(None));

    // The guard: cancel the quit and raise ConfirmQuit if a terminal is running;
    // otherwise allow it. Shared by both the app-terminate (Cmd+Q / menu Quit)
    // and window-close (red-X) hooks — a single-window app, so both mean "quit".
    fn approve(shell: &ShellSlot, app: &mut AppContext) -> ApproveTerminateResult {
        let handle = shell.borrow().as_ref().map(|(_, h)| h.clone());
        let Some(handle) = handle else {
            return ApproveTerminateResult::Terminate;
        };
        handle.update(app, |view, vctx| {
            if view.approve_terminate(vctx) {
                ApproveTerminateResult::Terminate
            } else {
                ApproveTerminateResult::Cancel
            }
        })
    }

    let mut callbacks = platform::AppCallbacks::default();
    let shell_term = shell.clone();
    callbacks.on_should_terminate_app =
        Some(Box::new(move |app: &mut AppContext| approve(&shell_term, app)));
    let shell_close = shell.clone();
    callbacks.on_should_close_window = Some(Box::new(
        move |_wid: WindowId, app: &mut AppContext| approve(&shell_close, app),
    ));
    // Window resize happens without a shell action, and ChildView caches each
    // pane child's element tree — terminals would keep their stale grid size
    // (no SIGWINCH) until the next click. Nudge every pane child on resize.
    let shell_resize = shell.clone();
    callbacks.on_window_resized = Some(Box::new(move |app: &mut AppContext| {
        let handle = shell_resize.borrow().as_ref().map(|(_, h)| h.clone());
        if let Some(handle) = handle {
            handle.update(app, |view, vctx| {
                use warpui::TypedActionView as _;
                view.handle_action(&shell::CraneShellAction::RelayoutPanes, vctx);
            });
        }
    }));

    let mut app_builder = platform::AppBuilder::new(callbacks, Box::new(ASSETS), None);
    // Install the native macOS menu bar (no-op off macOS). Its item callbacks
    // reach the same shell slot the terminate/close guards use, dispatching
    // existing CraneShellActions through CraneShellView::handle_action.
    platform_menu::install(&mut app_builder, shell.clone());
    let shell_init = shell.clone();
    let _ = app_builder.run(move |ctx| {
        set_app_icon();
        // Use persisted window size if available; otherwise fall back to the
        // default 1480×920. Minimum size (800×500) is not expressible via
        // WindowBounds::ExactSize — it only sets the initial size.
        // TODO min-size: wire up once warpui exposes a set_min_size API.
        let initial_size = persist::load()
            .filter(|st| st.window_w > 0.0 && st.window_h > 0.0)
            .map(|st| vec2f(st.window_w, st.window_h))
            .unwrap_or_else(|| vec2f(1480.0, 920.0));
        let (wid, handle) = ctx.add_window(
            AddWindowOptions {
                title: Some("Crane".to_string()),
                window_bounds: platform::WindowBounds::ExactSize(initial_size),
                ..Default::default()
            },
            CraneShellView::new,
        );
        *shell_init.borrow_mut() = Some((wid, handle));
    });
}

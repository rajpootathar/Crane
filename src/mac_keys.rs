//! macOS-only NSEvent local monitor that intercepts a handful of key
//! chords before winit/egui see them:
//!
//! * **Cmd+V (image paste)** — egui-winit calls `arboard.get()` on
//!   Cmd+V, which returns None for image clipboards. We detect that
//!   case, write the image to a PNG under
//!   `~/.crane/paste-images/<uuid>.png`, and queue the path for the
//!   next render frame. Same mechanism Ghostty / iTerm2 / Warp use.
//!
//! * **Cmd+C / Cmd+V / Cmd+X / Cmd+A (Browser pane clipboard)** —
//!   when a Browser pane is focused we forward the corresponding
//!   AppKit selector (`copy:` / `paste:` / `cut:` / `selectAll:`)
//!   directly to the focused WKWebView and swallow. We do NOT install
//!   these shortcuts as menu-item key equivalents because AppKit
//!   would then eat them for every other focus context too and
//!   egui's TextEdit / terminal selection copy would silently break.
//!
//! * **Shift+Tab / Tab / Cmd+`** — see comments at each path below.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::msg_send;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSPasteboard,
};
use objc2_foundation::{NSData, NSDictionary, NSString};
use parking_lot::Mutex;
use std::sync::atomic::AtomicPtr;
use std::sync::OnceLock;

static PENDING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static PENDING_SHIFT_TAB: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static PENDING_TAB: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static TERMINAL_FOCUSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Net pending Cmd+Backtick presses: +1 per Cmd+` (forward, no
/// shift), -1 per Cmd+~ (backward, shift). Signed so rapid
/// double-taps cancel cleanly. Drained each frame by the
/// tab-switcher dispatch.
static PENDING_TAB_CYCLE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
/// Live Cmd modifier state, tracked off `flagsChanged` NSEvents.
/// egui's own `i.modifiers.command` can miss a release when no other
/// key event wakes the frame loop between hold and release, leaving
/// the tab switcher hanging open. This atomic plus a forced
/// `request_repaint()` on every flagsChanged gives us a reliable
/// commit-on-release.
static CMD_HELD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Pointer to the currently-focused WKWebView (embedded via wry in a
/// Browser pane), or null when no browser pane owns focus. Holds a
/// +1 retain while non-null so the object stays alive between the
/// egui render frame that stored it and the NSEvent handler that
/// reads it. Written only from the main thread (`set_focused_webview`
/// from `BrowserHost::sync`); read only from the main thread (the
/// NSEvent local monitor). AtomicPtr is used for lock-free access,
/// not cross-thread synchronization.
static FOCUSED_WEBVIEW: AtomicPtr<AnyObject> = AtomicPtr::new(std::ptr::null_mut());

fn pending() -> &'static Mutex<Vec<String>> {
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drain any image paths that the Cmd+V monitor has written since the
/// last call. Called from the terminal render loop.
pub fn drain_pending_image_paths() -> Vec<String> {
    let mut q = pending().lock();
    std::mem::take(&mut *q)
}

/// Consume the count of Shift+Tab presses the NSEvent monitor has
/// captured since the last call. Each count maps to one CSI Z write.
pub fn drain_pending_shift_tab() -> usize {
    PENDING_SHIFT_TAB.swap(0, std::sync::atomic::Ordering::Relaxed)
}

pub fn drain_pending_tab() -> usize {
    PENDING_TAB.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// Consume the net Cmd+Backtick delta. Positive = forward cycle count,
/// negative = backward. Caller loops the absolute value, picking
/// direction by sign. Macos-only because winit/egui on macOS routes
/// this chord to the native "switch app windows" handler before the
/// app ever sees it.
pub fn drain_pending_tab_cycle() -> i32 {
    PENDING_TAB_CYCLE.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// True while the Command modifier is currently pressed. Sourced
/// from NSEvent `flagsChanged` so the transition is observed even
/// when no other key event is driving the frame loop.
pub fn is_cmd_held() -> bool {
    CMD_HELD.load(std::sync::atomic::Ordering::Relaxed)
}

/// Must be called every render frame from the terminal view with
/// `true` when the terminal pane owns focus, `false` otherwise. The
/// NSEvent monitor only swallows Shift+Tab when this is `true` —
/// without the gate, pressing Shift+Tab inside a TextEdit (tab rename,
/// find bar, commit message) would silently disappear.
pub fn set_terminal_focused(focused: bool) {
    TERMINAL_FOCUSED.store(focused, std::sync::atomic::Ordering::Relaxed);
}

/// Register (or clear) the currently-focused WKWebView. Called from
/// `BrowserHost::sync` every frame — `Some(ptr)` when a Browser pane
/// is focused and its webview is visible, `None` otherwise. The
/// NSEvent monitor reads this to decide whether Cmd+C/V/X/A should
/// be forwarded to the webview or passed through to egui.
///
/// We take a raw pointer (not an objc2 `Retained`) because wry's
/// public API returns a `Retained<WryWebView>` from a *different*
/// objc2 major version than ours (wry 0.55 pulls in objc2 0.6; we
/// use 0.5). Raw pointers + the ABI-stable `objc_retain`/`objc_release`
/// in `objc-sys` are version-agnostic. The caller is responsible for
/// keeping the object alive at least long enough for this function
/// to issue the retain.
///
/// Internally we hold a +1 retain while the pointer is stored so the
/// object can't deallocate between the set and the NSEvent lookup.
/// Replacing (or clearing) releases the old retain.
pub fn set_focused_webview(view: Option<std::ptr::NonNull<AnyObject>>) {
    let new_ptr = match view {
        Some(p) => {
            // SAFETY: `objc_retain` accepts any valid Obj-C object
            // pointer and bumps its refcount. The caller guarantees
            // `p` is live at call time.
            unsafe {
                objc2::ffi::objc_retain(p.as_ptr() as *mut _) as *mut AnyObject
            }
        }
        None => std::ptr::null_mut(),
    };
    let old = FOCUSED_WEBVIEW.swap(new_ptr, std::sync::atomic::Ordering::Relaxed);
    if !old.is_null() {
        // SAFETY: `old` carried a +1 retain we issued on a previous
        // `set_focused_webview` call. Balancing with release here.
        unsafe {
            objc2::ffi::objc_release(old as *mut _);
        }
    }
}

/// Register the NSEvent local monitor. Must be called on the main
/// thread after the NSApp has been initialized (eframe does that
/// before calling our App::new, so we call this from eframe's
/// CreationContext).
pub fn install_cmd_v_monitor() {
    // Idempotent — the OnceLock below doubles as a "registered" flag.
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.get().is_some() {
        return;
    }
    let _ = INSTALLED.set(());

    // The block returns Option<Retained<NSEvent>>: Some(event) → pass
    // through to the app; None → swallow (winit never sees it).
    let handler = RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        let event = unsafe { event.as_ref() };
        let passthrough = event as *const NSEvent as *mut NSEvent;
        unsafe {
            let etype = event.r#type();
            // Track Cmd state from flagsChanged so the tab switcher's
            // commit-on-release never gets stuck waiting for another
            // key event to wake egui. Pass the event through either
            // way — we're only observing, not blocking.
            if etype == NSEventType::FlagsChanged {
                let f = event.modifierFlags();
                let cmd =
                    f.contains(NSEventModifierFlags::NSEventModifierFlagCommand);
                CMD_HELD.store(cmd, std::sync::atomic::Ordering::Relaxed);
                return passthrough;
            }
            if etype != NSEventType::KeyDown {
                return passthrough;
            }
            let flags = event.modifierFlags();
            // Keep CMD_HELD in sync on keyDown too, in case we missed
            // a flagsChanged (first keypress after focus change, etc.).
            CMD_HELD.store(
                flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
                std::sync::atomic::Ordering::Relaxed,
            );

            // --- Shift+Tab path -------------------------------------
            // egui's focus navigator eats Shift+Tab (back-focus) before
            // our terminal handler can see it, even with `consume_key`
            // in the same frame. Catch it at NSEvent level so TUIs
            // (zsh reverse menu, Claude Code, fzf) actually get CSI Z.
            // Only swallow when the terminal pane owns focus; in a
            // TextEdit we want egui's normal back-focus behavior.
            let key_code = event.keyCode();
            const TAB_KEY_CODE: u16 = 0x30;
            const BACKTICK_KEY_CODE: u16 = 0x32;
            if key_code == TAB_KEY_CODE
                && flags.contains(NSEventModifierFlags::NSEventModifierFlagShift)
                && !flags.intersects(
                    NSEventModifierFlags::NSEventModifierFlagCommand
                        | NSEventModifierFlags::NSEventModifierFlagControl
                        | NSEventModifierFlags::NSEventModifierFlagOption,
                )
                && TERMINAL_FOCUSED.load(std::sync::atomic::Ordering::Relaxed)
            {
                PENDING_SHIFT_TAB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return std::ptr::null_mut();
            }

            // --- Plain Tab path -------------------------------------
            // egui's focus navigator eats plain Tab to cycle between
            // interactive widgets — which means terminal autocomplete
            // (zsh, fzf, Claude Code) never sees the key. Same fix as
            // Shift+Tab: intercept at NSEvent level when the terminal
            // pane owns focus, queue it, and write `\t` from the
            // terminal view's drain. Gated on TERMINAL_FOCUSED so Tab
            // inside a TextEdit (rename, find bar) behaves normally.
            if key_code == TAB_KEY_CODE
                && !flags.intersects(
                    NSEventModifierFlags::NSEventModifierFlagShift
                        | NSEventModifierFlags::NSEventModifierFlagCommand
                        | NSEventModifierFlags::NSEventModifierFlagControl
                        | NSEventModifierFlags::NSEventModifierFlagOption,
                )
                && TERMINAL_FOCUSED.load(std::sync::atomic::Ordering::Relaxed)
            {
                PENDING_TAB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return std::ptr::null_mut();
            }

            // --- Cmd+Backtick tab-switcher path ---------------------
            // macOS routes Cmd+` / Cmd+~ to its native "cycle windows
            // in app" handler before winit/egui ever sees the key.
            // We intercept at NSEvent, queue the signed cycle count,
            // and swallow so macOS doesn't also steal the focus.
            // Cmd+~ (shift held) = forward (+1). Cmd+` (no shift) =
            // backward (-1). No Ctrl / Alt allowed.
            if key_code == BACKTICK_KEY_CODE
                && flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand)
                && !flags.intersects(
                    NSEventModifierFlags::NSEventModifierFlagControl
                        | NSEventModifierFlags::NSEventModifierFlagOption,
                )
            {
                // Match native macOS cycling direction: plain Cmd+`
                // is forward, Cmd+~ (shift held) is backward.
                let backward =
                    flags.contains(NSEventModifierFlags::NSEventModifierFlagShift);
                let delta: i32 = if backward { -1 } else { 1 };
                PENDING_TAB_CYCLE.fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
                return std::ptr::null_mut();
            }

            // --- Cmd+V image paste path -----------------------------
            if !flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand) {
                return passthrough;
            }
            if flags.intersects(
                NSEventModifierFlags::NSEventModifierFlagShift
                    | NSEventModifierFlags::NSEventModifierFlagOption
                    | NSEventModifierFlags::NSEventModifierFlagControl,
            ) {
                return passthrough;
            }
            let chars = match event.charactersIgnoringModifiers() {
                Some(s) => s.to_string(),
                None => return passthrough,
            };

            // --- Browser-pane clipboard forwarding ------------------
            // Cmd+C / V / X / A never reach the WKWebView via the
            // winit NSView (which doesn't implement those selectors)
            // and we deliberately don't use a menu-item key equivalent
            // (see platform_menu.rs for why). Instead, when a Browser
            // pane is focused we dispatch the standard AppKit action
            // directly to the stored webview and swallow the event.
            let lower = chars.to_ascii_lowercase();
            if matches!(lower.as_str(), "c" | "v" | "x" | "a") {
                let wv_ptr = FOCUSED_WEBVIEW.load(std::sync::atomic::Ordering::Relaxed);
                if !wv_ptr.is_null() {
                    // SAFETY: `wv_ptr` holds a +1 retain for as long
                    // as FOCUSED_WEBVIEW carries it. We only borrow,
                    // never take ownership, so the retain stays with
                    // the slot until set_focused_webview swaps it.
                    // (Outer `unsafe` block covers the whole handler.)
                    let view: &AnyObject = &*wv_ptr;
                    let nil: *mut AnyObject = std::ptr::null_mut();
                    match lower.as_str() {
                        "c" => {
                            let _: () = msg_send![view, copy: nil];
                        }
                        "v" => {
                            let _: () = msg_send![view, paste: nil];
                        }
                        "x" => {
                            let _: () = msg_send![view, cut: nil];
                        }
                        "a" => {
                            let _: () = msg_send![view, selectAll: nil];
                        }
                        _ => {}
                    }
                    // Consume so egui/winit never also processes it.
                    return std::ptr::null_mut();
                }
            }

            if chars != "v" && chars != "V" {
                return passthrough;
            }

            // Cmd+V with no other modifiers. Check NSPasteboard for
            // image content; if present write PNG and swallow the
            // event so egui-winit doesn't log its arboard error.
            match try_write_pasteboard_image_to_file() {
                Some(path) => {
                    pending().lock().push(path);
                    std::ptr::null_mut()
                }
                None => passthrough,
            }
        }
    });

    unsafe {
        let _monitor: Option<Retained<objc2::runtime::AnyObject>> =
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::FlagsChanged,
                &handler,
            );
        // The returned token is kept alive by the NSApp; we
        // deliberately leak our reference because we never remove the
        // monitor for the process lifetime.
        std::mem::forget(_monitor);
    }
}

/// Read the general NSPasteboard, look for image data (from a
/// screenshot, Preview copy, Finder "Copy image", browser "Copy
/// image", etc.), and write the first result as a PNG under
/// `~/.crane/paste-images/<uuid>.png`. Returns the absolute path on
/// success.
fn try_write_pasteboard_image_to_file() -> Option<String> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        // Prefer PNG if present (screenshots, browser copy-image), fall
        // back to TIFF (Preview / native macOS copy) and re-encode as
        // PNG. Reading raw pasteboard data avoids the NSImage /
        // readObjectsForClasses generics dance that objc2's bindings
        // make clumsy.
        let png_type = NSString::from_str("public.png");
        let tiff_type = NSString::from_str("public.tiff");

        let data: Retained<NSData> = if let Some(d) = pb.dataForType(&png_type) {
            d
        } else if let Some(tiff) = pb.dataForType(&tiff_type) {
            // Decode TIFF via NSBitmapImageRep and re-emit as PNG.
            let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
            let empty: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> =
                NSDictionary::new();
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &empty)?
        } else {
            return None;
        };

        let home = std::env::var_os("HOME")?;
        let dir = std::path::PathBuf::from(home)
            .join(".crane")
            .join("paste-images");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}.png", uuid::Uuid::new_v4()));

        let bytes: &[u8] = std::slice::from_raw_parts(data.bytes().as_ptr(), data.length());
        std::fs::write(&path, bytes).ok()?;
        Some(path.to_string_lossy().into_owned())
    }
}

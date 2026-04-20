//! macOS-only Cmd+V hook that bypasses egui-winit's text-only paste
//! handler. egui-winit calls `arboard.get()` on Cmd+V, which returns
//! None for image clipboards — egui-winit then `return`s without
//! pushing the Key event, so nothing downstream ever learns that V was
//! pressed. We install an NSEvent local monitor that sees Cmd+V before
//! winit, detects image clipboard content, writes it to a PNG under
//! `~/.crane/paste-images/<uuid>.png`, and queues the path for the
//! next render frame to consume. Non-image Cmd+V is passed through
//! untouched so normal text paste still works.
//!
//! Same mechanism Ghostty / iTerm2 / Warp use for image paste.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSPasteboard,
};
use objc2_foundation::{NSData, NSDictionary, NSString};
use parking_lot::Mutex;
use std::sync::OnceLock;

static PENDING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static PENDING_SHIFT_TAB: std::sync::atomic::AtomicUsize =
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

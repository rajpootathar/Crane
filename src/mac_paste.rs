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

fn pending() -> &'static Mutex<Vec<String>> {
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drain any image paths that the Cmd+V monitor has written since the
/// last call. Called from the terminal render loop.
pub fn drain_pending_image_paths() -> Vec<String> {
    let mut q = pending().lock();
    std::mem::take(&mut *q)
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
        unsafe {
            if event.r#type() != NSEventType::KeyDown {
                return event as *const NSEvent as *mut NSEvent;
            }
            let flags = event.modifierFlags();
            // Cmd must be held; if any of Shift / Alt / Ctrl are also
            // held we bail — users press those combos for other things.
            if !flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand) {
                return event as *const NSEvent as *mut NSEvent;
            }
            if flags.intersects(
                NSEventModifierFlags::NSEventModifierFlagShift
                    | NSEventModifierFlags::NSEventModifierFlagOption
                    | NSEventModifierFlags::NSEventModifierFlagControl,
            ) {
                return event as *const NSEvent as *mut NSEvent;
            }
            let chars = match event.charactersIgnoringModifiers() {
                Some(s) => s.to_string(),
                None => return event as *const NSEvent as *mut NSEvent,
            };
            if chars != "v" && chars != "V" {
                return event as *const NSEvent as *mut NSEvent;
            }

            // Cmd+V with no other modifiers. Check NSPasteboard for
            // image content; if present write PNG and swallow the
            // event so egui-winit doesn't log its arboard error.
            match try_write_pasteboard_image_to_file() {
                Some(path) => {
                    pending().lock().push(path);
                    std::ptr::null_mut()
                }
                None => event as *const NSEvent as *mut NSEvent,
            }
        }
    });

    unsafe {
        let _monitor: Option<Retained<objc2::runtime::AnyObject>> =
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown,
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

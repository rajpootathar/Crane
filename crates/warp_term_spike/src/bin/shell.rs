use anyhow::Result;

use warp_term_spike::shell::CraneShellView;
use warp_term_spike::ASSETS;
use warpui::{platform, AddWindowOptions};

/// Set the Crane dock/app icon from `crane.png`. warpui only installs an icon
/// when its `dev_icon` is set (default none), so we message the shared
/// NSApplication directly — same ObjC runtime, independent of warpui.
#[cfg(target_os = "macos")]
fn set_app_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crane.png"));
    let data = NSData::with_bytes(bytes);
    let app = NSApplication::sharedApplication(mtm);
    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { app.setApplicationIconImage(Some(&img)) };
    }
}

#[cfg(not(target_os = "macos"))]
fn set_app_icon() {}

fn main() -> Result<()> {
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
        // After warpui has created its NSApplication (with its ivars).
        set_app_icon();
        ctx.add_window(
            AddWindowOptions {
                title: Some("crane × warpui — app shell".to_string()),
                ..Default::default()
            },
            CraneShellView::new,
        );
    });
    Ok(())
}

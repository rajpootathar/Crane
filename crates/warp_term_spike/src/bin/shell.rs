use anyhow::Result;

use warp_term_spike::shell::CraneShellView;
use warp_term_spike::ASSETS;
use warpui::{platform, AddWindowOptions};

fn main() -> Result<()> {
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
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

use anyhow::Result;

use warp_term_spike::view::TerminalView;
use warp_term_spike::ASSETS;
use warpui::{platform, AddWindowOptions};

fn main() -> Result<()> {
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
        ctx.add_window(
            AddWindowOptions {
                title: Some("crane × warpui — live terminal".to_string()),
                ..Default::default()
            },
            TerminalView::new,
        );
    });
    Ok(())
}

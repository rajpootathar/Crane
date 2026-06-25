//! Framework-agnostic repaint seam.
//!
//! The model/logic layer (state, session, git_log, terminal, lsp) only needs
//! a way to ask the active frontend to repaint. egui supplies this via
//! `egui::Context::request_repaint`; warpui via its own redraw token. Instead
//! of threading a concrete `egui::Context` through the logic (which couples it
//! to egui), we thread a `WakeHandle` — exactly the `Arc<dyn Fn()+Send+Sync>`
//! type `JobSystem::new` already consumes — and build it from whichever
//! frontend is active at the boundary.
//!
//! This is the seam the egui -> warpui migration leans on: the logic becomes
//! egui-free, and each frontend constructs its own `WakeHandle`.

use std::sync::Arc;

/// A cheap, thread-safe "please repaint" callback shared across worker threads.
pub type WakeHandle = Arc<dyn Fn() + Send + Sync>;

/// Build a [`WakeHandle`] from an egui context. Used by the egui frontend at
/// the boundary; deleted when egui is removed.
pub fn wake_from_egui(ctx: &egui::Context) -> WakeHandle {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

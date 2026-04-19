//! In-memory app model (`App`), layout tree, and their on-disk
//! persistence (session / settings / per-project cache).

mod state;
pub mod layout;
pub mod session;
pub mod settings;
pub mod project_cache;

// Re-export `App` + public siblings so existing callers keep writing
// `crate::state::App` instead of `crate::state::state::App`.
pub use state::*;

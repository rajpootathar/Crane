//! Alt-screen wrapper.
//!
//! v1 just re-uses [`Term`] with `ALT_SCREEN` set in its mode bag.
//! That mode bit gates the scrollback push in `Term::scroll_up_one`,
//! so the alt grid never feeds history. Cursor save/restore is
//! handled by the parent `Term` in subsequent work.
//!
//! Kept as its own module for future divergence — Warp uses a
//! distinct wrapper for selection state and pending-scroll
//! accumulators that don't apply on the main screen.

#[allow(dead_code)]
pub struct AltScreen;

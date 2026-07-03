//! App-wide UI font size, adjustable via Cmd+= / Cmd+- / Cmd+0 (mirrors old
//! egui `App.font_size`). Stored as an `AtomicU32` holding the `f32` bits so the
//! terminal grid and the editor can read it lock-free each frame. The terminal
//! renders at `base()`; the editor renders one point smaller (preserving the
//! prior 14 vs 13 relationship). Zoom clamps to [MIN, MAX].

use std::sync::atomic::{AtomicU32, Ordering};

const DEFAULT: f32 = 14.0;
const MIN: f32 = 8.0;
const MAX: f32 = 40.0;

static BASE: AtomicU32 = AtomicU32::new(0); // 0 = uninitialised → treated as DEFAULT

fn read() -> f32 {
    let bits = BASE.load(Ordering::Relaxed);
    if bits == 0 {
        DEFAULT
    } else {
        f32::from_bits(bits)
    }
}

/// Terminal font size (the base app font size).
pub fn base() -> f32 {
    read()
}

/// Editor font size — one point below the base, matching the prior 14/13 split.
pub fn editor() -> f32 {
    (read() - 1.0).max(MIN - 1.0)
}

/// Set the base font size, clamped to [MIN, MAX].
pub fn set(size: f32) {
    let clamped = size.clamp(MIN, MAX);
    BASE.store(clamped.to_bits(), Ordering::Relaxed);
}

/// Zoom by `delta` points (Cmd+= = +1, Cmd+- = -1). Returns the new base.
pub fn zoom(delta: f32) -> f32 {
    let next = (read() + delta).clamp(MIN, MAX);
    set(next);
    next
}

/// Reset to the default size (Cmd+0).
pub fn reset() {
    set(DEFAULT);
}

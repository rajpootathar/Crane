//! App-wide UI zoom (Cmd+= / Cmd+- / Cmd+0). warpui's global
//! `AppContext::set_zoom_factor` magnifies EVERY rendered element uniformly —
//! panels, tabs, breadcrumb, status bar, menus, terminal, editor. We just track
//! the current level here so it can be stepped + persisted (the `ZoomFactor`
//! type has no public getter), and expose the base font sizes the terminal /
//! editor render at (the global zoom multiplies on top of these).

use std::sync::atomic::{AtomicU32, Ordering};

const TERMINAL_FONT: f32 = 14.0;
const EDITOR_FONT: f32 = 13.0;
const DEFAULT_ZOOM: f32 = 1.0;
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 4.0;
const STEP: f32 = 0.1;
/// Settable base-size bounds (Settings > Appearance steppers) — the old egui
/// slider's 9..=28 range.
const MIN_BASE: f32 = 9.0;
const MAX_BASE: f32 = 28.0;

static ZOOM: AtomicU32 = AtomicU32::new(0); // 0 = unset → DEFAULT_ZOOM
static TERM_BASE: AtomicU32 = AtomicU32::new(0); // 0 = unset → TERMINAL_FONT
static EDITOR_BASE: AtomicU32 = AtomicU32::new(0); // 0 = unset → EDITOR_FONT

fn read(cell: &AtomicU32, default: f32) -> f32 {
    let bits = cell.load(Ordering::Relaxed);
    if bits == 0 {
        default
    } else {
        f32::from_bits(bits)
    }
}

/// Terminal base font size (before global zoom). Views read this per paint,
/// so a Settings change takes effect on the next repaint.
pub fn base() -> f32 {
    read(&TERM_BASE, TERMINAL_FONT)
}

/// Editor base font size (before global zoom).
pub fn editor() -> f32 {
    read(&EDITOR_BASE, EDITOR_FONT)
}

/// Set the terminal base size (Settings stepper / persisted restore).
pub fn set_base(v: f32) {
    TERM_BASE.store(v.clamp(MIN_BASE, MAX_BASE).to_bits(), Ordering::Relaxed);
}

/// Set the editor base size.
pub fn set_editor(v: f32) {
    EDITOR_BASE.store(v.clamp(MIN_BASE, MAX_BASE).to_bits(), Ordering::Relaxed);
}

/// Current zoom level (1.0 = 100%). Persisted; drives `set_zoom_factor`.
pub fn zoom_level() -> f32 {
    let bits = ZOOM.load(Ordering::Relaxed);
    if bits == 0 {
        DEFAULT_ZOOM
    } else {
        f32::from_bits(bits)
    }
}

/// Set the zoom level, clamped to warpui's supported range.
pub fn set_level(z: f32) {
    ZOOM.store(z.clamp(MIN_ZOOM, MAX_ZOOM).to_bits(), Ordering::Relaxed);
}

/// Step the zoom by `delta` (Cmd+= = +STEP, Cmd+- = -STEP). Returns the new level.
pub fn zoom(delta: f32) -> f32 {
    let next = (zoom_level() + delta).clamp(MIN_ZOOM, MAX_ZOOM);
    set_level(next);
    next
}

/// Reset to 100% (Cmd+0). Returns the level.
pub fn reset() -> f32 {
    set_level(DEFAULT_ZOOM);
    DEFAULT_ZOOM
}

/// The per-keystroke zoom step.
pub fn step() -> f32 {
    STEP
}

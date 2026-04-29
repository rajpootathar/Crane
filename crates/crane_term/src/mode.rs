//! DEC private modes we honor, packed as one bitflag set.
//!
//! Subset of xterm's mode space — covers what real-world TUIs and
//! shells exercise. New flags get added as we hit them.

use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct TermMode: u32 {
        const SHOW_CURSOR        = 1 << 0;
        const APP_CURSOR         = 1 << 1;
        const APP_KEYPAD         = 1 << 2;
        /// DECOM. Cursor addressing is relative to the scroll region.
        const ORIGIN             = 1 << 3;
        const INSERT             = 1 << 4;
        /// Auto-wrap at right margin (DECAWM).
        const LINE_WRAP          = 1 << 5;
        /// Bracketed paste mode (xterm 2004).
        const BRACKETED_PASTE    = 1 << 6;
        const ALT_SCREEN         = 1 << 7;
        /// Swap to alt screen + save cursor + clear (1049).
        const ALT_SCREEN_1049    = 1 << 8;
        /// X10 single-click mouse reporting.
        const MOUSE_REPORT_CLICK = 1 << 9;
        /// Drag (down + motion + up) reporting.
        const MOUSE_DRAG         = 1 << 10;
        /// Any-event motion reporting.
        const MOUSE_MOTION       = 1 << 11;
        const MOUSE_SGR          = 1 << 12;
        const MOUSE_UTF8         = 1 << 13;
        /// Focus-in / focus-out events.
        const FOCUS_IN_OUT       = 1 << 14;
        /// Synchronized output is being buffered. Set by the
        /// processor when `?2026h` is parsed; cleared when `?2026l`
        /// fires or the safety timeout / buffer cap trips.
        const SYNC_OUTPUT        = 1 << 15;
        const URGENCY_HINTS      = 1 << 16;
    }
}

impl Default for TermMode {
    fn default() -> Self {
        TermMode::SHOW_CURSOR
            | TermMode::LINE_WRAP
            | TermMode::URGENCY_HINTS
    }
}

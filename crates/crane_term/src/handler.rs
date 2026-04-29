//! Crane's `Handler` trait — narrow surface the [`crate::term::Term`]
//! exposes for parsed VT events.
//!
//! Two design choices distinguish this from `vte::ansi::Handler`:
//!
//! 1. Scroll-producing methods return [`ScrollDelta`]. The renderer
//!    and selection layer can adjust without re-deriving how many
//!    rows the operation actually shifted. Alacritty's trait returns
//!    `()` here, which is why a wrapper around its `Term` can never
//!    fix the TUI scrollback bug — the scroll decision is made
//!    inside `Term` and not surfaced.
//!
//! 2. [`ProcessorInput`] carries an `is_sync_frame` flag through
//!    [`Handler::on_finish_byte_processing`]. Used by listeners that
//!    care about render-batching boundaries; the grid itself does
//!    not gate scrollback writes on this flag — that's `linefeed`'s
//!    cursor-position check.

/// Reported by every Handler method whose semantics may shift rows
/// off the top of the scroll region into scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollDelta {
    #[default]
    Zero,
    Up { lines: usize },
    Down { lines: usize },
}

impl ScrollDelta {
    pub fn zero() -> Self {
        ScrollDelta::Zero
    }
}

/// Boundary marker passed to `on_finish_byte_processing`. Lets the
/// caller distinguish "frame from a `?2026` flush" from "frame from
/// streaming output", e.g. to defer a render pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessorInput<'a> {
    pub bytes: &'a [u8],
    pub is_sync_frame: bool,
}

/// Trait the [`crate::processor::Processor`] drives. v1 is
/// intentionally minimal — methods get added as we wire up real CSI
/// dispatch in [`crate::perform`]. Default impls are no-ops so
/// in-progress wiring still compiles.
#[allow(unused_variables)]
pub trait Handler {
    // ---- character / cursor ----

    fn input(&mut self, c: char) {}
    fn goto(&mut self, line: usize, col: usize) {}
    fn goto_line(&mut self, line: usize) {}
    fn goto_col(&mut self, col: usize) {}
    fn move_up(&mut self, n: usize) {}
    fn move_down(&mut self, n: usize) {}
    fn move_forward(&mut self, n: usize) {}
    fn move_backward(&mut self, n: usize) {}
    fn move_up_and_cr(&mut self, n: usize) {}
    fn move_down_and_cr(&mut self, n: usize) {}
    fn backspace(&mut self) {}
    fn carriage_return(&mut self) {}
    fn put_tab(&mut self, count: u16) {}
    fn save_cursor(&mut self) {}
    fn restore_cursor(&mut self) {}

    // ---- scroll-producing ----

    fn linefeed(&mut self) -> ScrollDelta {
        ScrollDelta::Zero
    }
    fn newline(&mut self) {
        let _ = self.linefeed();
    }
    fn reverse_index(&mut self) -> ScrollDelta {
        ScrollDelta::Zero
    }
    fn scroll_up(&mut self, n: usize) -> ScrollDelta {
        ScrollDelta::Zero
    }
    fn scroll_down(&mut self, n: usize) -> ScrollDelta {
        ScrollDelta::Zero
    }
    fn insert_blank_lines(&mut self, n: usize) -> ScrollDelta {
        ScrollDelta::Zero
    }
    fn delete_lines(&mut self, n: usize) -> ScrollDelta {
        ScrollDelta::Zero
    }

    // ---- mutation, no scroll ----

    fn insert_blank(&mut self, n: usize) {}
    fn erase_chars(&mut self, n: usize) {}
    fn delete_chars(&mut self, n: usize) {}
    fn clear_line(&mut self, mode: vte::ansi::LineClearMode) {}
    fn clear_screen(&mut self, mode: vte::ansi::ClearMode) {}
    fn clear_tabs(&mut self, mode: vte::ansi::TabulationClearMode) {}
    fn set_horizontal_tabstop(&mut self) {}

    // ---- modes ----

    fn set_mode(&mut self, mode: vte::ansi::Mode) {}
    fn unset_mode(&mut self, mode: vte::ansi::Mode) {}
    fn set_private_mode(&mut self, mode: vte::ansi::PrivateMode) {}
    fn unset_private_mode(&mut self, mode: vte::ansi::PrivateMode) {}
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {}
    fn set_keypad_application_mode(&mut self) {}
    fn unset_keypad_application_mode(&mut self) {}
    fn reset_state(&mut self) {}

    // ---- attribute ----

    fn terminal_attribute(&mut self, attr: vte::ansi::Attr) {}

    // ---- title / bell ----

    fn set_title(&mut self, title: Option<String>) {}
    fn bell(&mut self) {}

    // ---- queries the parser asks us to answer back to the PTY ----

    /// CSI Ps n. The implementation should push the appropriate
    /// reply bytes onto its outbound queue (e.g. `\e[<row>;<col>R`
    /// for `n == 6`). The Processor / PTY reader drains the queue
    /// after each parse pass.
    fn device_status(&mut self, _n: usize) {}

    /// CSI Ps c. Primary device-attribute reply; default impl
    /// answers with VT102 (`\e[?6c`). Override only if you need
    /// to advertise specific capabilities.
    fn identify_terminal(&mut self, _intermediate: Option<char>) {}

    // ---- lifecycle ----

    /// Called once per non-sync byte chunk and again after a sync
    /// flush replays its buffered bytes. Implementations typically
    /// use this to mark a render-frame boundary.
    fn on_finish_byte_processing(&mut self, input: &ProcessorInput) {}

    /// Called by the [`crate::processor::Processor`] around a
    /// `?2026h ... ?2026l` replay. While `true`, scroll-producing
    /// methods that would otherwise evict a row to scrollback must
    /// drop the row instead — TUI redraws emit LFs that land at
    /// the screen bottom while stepping back through their own
    /// region, and those rows aren't "real history" to be
    /// preserved. Cleared back to `false` after the replay so
    /// streaming output following the sync block continues to
    /// feed scrollback normally.
    fn set_sync_frame(&mut self, _active: bool) {}
}

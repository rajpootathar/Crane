//! Synchronized-Output–aware VT handler wrapper.
//!
//! Ink-based TUIs (Claude Code and every `ink`-powered CLI) repaint
//! their prompt region by emitting `\e[?2026h`, then
//! `cursor-up N ... write ... linefeed ... write ... linefeed ...`,
//! then `\e[?2026l`. A terminal with a true shadow grid (iTerm2)
//! buffers that whole block against a snapshot and, at commit,
//! diff-applies cells back into the live grid — no LF ever actually
//! scrolls the visible grid. `alacritty_terminal` 0.25 only stashes
//! bytes at the parser and replays them at commit, so the LFs still
//! execute and push the bottom row into scrollback. Result: each
//! redraw leaks one copy of the prompt into history.
//!
//! This module gives us iTerm-ish semantics *without* forking
//! alacritty_terminal, by wrapping `Term` with a Handler impl that:
//!
//! 1. Tracks whether we're inside a `?2026` sync block
//!    (`in_sync` — driven by the PTY reader, see
//!    [`strip_sync_and_track`]).
//! 2. Counts how many rows the TUI just cursored upward via
//!    `move_up` / `move_up_and_cr` / `reverse_index`
//!    (`move_up_pending`).
//! 3. Converts exactly that many subsequent `linefeed` / `newline` /
//!    `index`-equivalent calls to non-scrolling `move_down(1)` — i.e.
//!    the LFs that correspond to the TUI *stepping back down through
//!    its own redraw region* cannot push rows into history.
//! 4. Delegates every other Handler method straight to `Term` so all
//!    legitimate output (streaming text, prompt rendering outside of
//!    a redraw) behaves exactly as before.
//!
//! Once the counter reaches zero, LFs scroll normally — streaming
//! response text still appends to scrollback the usual way.
//!
//! Scope limit: this is a heuristic, not a true shadow grid. It
//! handles the exact pattern Ink uses (cursor-up + LF-stepping) and
//! leaves every other TUI unchanged. A proper shadow grid remains
//! filed in CLAUDE.md as long-term work.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink,
    KeyboardModes, KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys,
    PrivateMode, Rgb, ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};
// CursorIcon leaks in via the `cursor_icon` crate that `vte` re-exposes
// through its Handler trait signature. vte doesn't re-export it, so
// we depend on the same crate directly.
use cursor_icon::CursorIcon;

pub struct SyncAwareHandler<'a, L: EventListener> {
    pub inner: &'a mut Term<L>,
    pub in_sync: bool,
    /// Budget of LFs whose scroll should be suppressed. Incremented by
    /// upward cursor motion inside a sync block; decremented by each
    /// subsequent LF-family call that we downgrade to `move_down`.
    /// Capped by the row count so a runaway TUI can't starve legit
    /// LFs forever.
    pub move_up_pending: usize,
}

impl<L: EventListener> SyncAwareHandler<'_, L> {
    /// True when the next LF should be suppress-scrolled. Combines
    /// the sync flag with the motion budget so we never downgrade
    /// LFs outside a sync block (streaming text must still scroll).
    #[inline]
    fn should_suppress_lf(&self) -> bool {
        self.in_sync && self.move_up_pending > 0
    }

    #[inline]
    fn consume_lf(&mut self) {
        self.move_up_pending = self.move_up_pending.saturating_sub(1);
    }
}

impl<L: EventListener> Handler for SyncAwareHandler<'_, L> {
    // ---- Overrides: the whole point of this wrapper ----

    fn linefeed(&mut self) {
        if self.should_suppress_lf() {
            self.consume_lf();
            self.inner.move_down(1);
        } else {
            self.inner.linefeed();
        }
    }

    fn newline(&mut self) {
        if self.should_suppress_lf() {
            self.consume_lf();
            self.inner.carriage_return();
            self.inner.move_down(1);
        } else {
            self.inner.newline();
        }
    }

    fn reverse_index(&mut self) {
        // RI is the "opposite" move — cursor up one, with scrolling
        // at top-of-region. Inside a sync block we count these as
        // additional upward motion, because a TUI using RI-stepping
        // instead of cursor-up would still redraw the region.
        if self.in_sync {
            self.move_up_pending = self.move_up_pending.saturating_add(1);
        }
        self.inner.reverse_index();
    }

    fn move_up(&mut self, n: usize) {
        if self.in_sync {
            self.move_up_pending = self.move_up_pending.saturating_add(n);
        }
        self.inner.move_up(n);
    }

    fn move_up_and_cr(&mut self, n: usize) {
        if self.in_sync {
            self.move_up_pending = self.move_up_pending.saturating_add(n);
        }
        self.inner.move_up_and_cr(n);
    }

    fn goto(&mut self, line: i32, col: usize) {
        // Absolute positioning inside a sync block breaks the
        // "counted upward motion" invariant — if the TUI jumps to an
        // absolute row, we can't know how many LFs are safe to
        // suppress anymore. Clear the budget and defer to the normal
        // handler. Correctness > aggressive suppression.
        if self.in_sync {
            self.move_up_pending = 0;
        }
        self.inner.goto(line, col);
    }

    fn goto_line(&mut self, line: i32) {
        if self.in_sync {
            self.move_up_pending = 0;
        }
        self.inner.goto_line(line);
    }

    fn save_cursor_position(&mut self) {
        // DECSC-style cursor saves inside a sync block confuse the
        // budget (a later restore may land anywhere). Same treatment
        // as an absolute goto: flush the budget.
        if self.in_sync {
            self.move_up_pending = 0;
        }
        self.inner.save_cursor_position();
    }

    fn restore_cursor_position(&mut self) {
        if self.in_sync {
            self.move_up_pending = 0;
        }
        self.inner.restore_cursor_position();
    }

    // ---- Everything below is plain delegation ----

    fn set_title(&mut self, t: Option<String>) { self.inner.set_title(t); }
    fn set_cursor_style(&mut self, s: Option<CursorStyle>) { self.inner.set_cursor_style(s); }
    fn set_cursor_shape(&mut self, s: CursorShape) { self.inner.set_cursor_shape(s); }
    fn input(&mut self, c: char) { self.inner.input(c); }
    fn goto_col(&mut self, col: usize) { self.inner.goto_col(col); }
    fn insert_blank(&mut self, n: usize) { self.inner.insert_blank(n); }
    fn move_down(&mut self, n: usize) { self.inner.move_down(n); }
    fn identify_terminal(&mut self, i: Option<char>) { self.inner.identify_terminal(i); }
    fn device_status(&mut self, n: usize) { self.inner.device_status(n); }
    fn move_forward(&mut self, n: usize) { self.inner.move_forward(n); }
    fn move_backward(&mut self, n: usize) { self.inner.move_backward(n); }
    fn move_down_and_cr(&mut self, n: usize) { self.inner.move_down_and_cr(n); }
    fn put_tab(&mut self, n: u16) { self.inner.put_tab(n); }
    fn backspace(&mut self) { self.inner.backspace(); }
    fn carriage_return(&mut self) { self.inner.carriage_return(); }
    fn bell(&mut self) { self.inner.bell(); }
    fn substitute(&mut self) { self.inner.substitute(); }
    fn set_horizontal_tabstop(&mut self) { self.inner.set_horizontal_tabstop(); }
    fn scroll_up(&mut self, n: usize) { self.inner.scroll_up(n); }
    fn scroll_down(&mut self, n: usize) { self.inner.scroll_down(n); }
    fn insert_blank_lines(&mut self, n: usize) { self.inner.insert_blank_lines(n); }
    fn delete_lines(&mut self, n: usize) { self.inner.delete_lines(n); }
    fn erase_chars(&mut self, n: usize) { self.inner.erase_chars(n); }
    fn delete_chars(&mut self, n: usize) { self.inner.delete_chars(n); }
    fn move_backward_tabs(&mut self, n: u16) { self.inner.move_backward_tabs(n); }
    fn move_forward_tabs(&mut self, n: u16) { self.inner.move_forward_tabs(n); }
    fn clear_line(&mut self, m: LineClearMode) { self.inner.clear_line(m); }
    fn clear_screen(&mut self, m: ClearMode) { self.inner.clear_screen(m); }
    fn clear_tabs(&mut self, m: TabulationClearMode) { self.inner.clear_tabs(m); }
    fn set_tabs(&mut self, i: u16) { self.inner.set_tabs(i); }
    fn reset_state(&mut self) { self.inner.reset_state(); }
    fn terminal_attribute(&mut self, a: Attr) { self.inner.terminal_attribute(a); }
    fn set_mode(&mut self, m: Mode) { self.inner.set_mode(m); }
    fn unset_mode(&mut self, m: Mode) { self.inner.unset_mode(m); }
    fn report_mode(&mut self, m: Mode) { self.inner.report_mode(m); }
    fn set_private_mode(&mut self, m: PrivateMode) { self.inner.set_private_mode(m); }
    fn unset_private_mode(&mut self, m: PrivateMode) { self.inner.unset_private_mode(m); }
    fn report_private_mode(&mut self, m: PrivateMode) { self.inner.report_private_mode(m); }
    fn set_scrolling_region(&mut self, top: usize, bot: Option<usize>) {
        self.inner.set_scrolling_region(top, bot);
    }
    fn set_keypad_application_mode(&mut self) { self.inner.set_keypad_application_mode(); }
    fn unset_keypad_application_mode(&mut self) { self.inner.unset_keypad_application_mode(); }
    fn set_active_charset(&mut self, i: CharsetIndex) { self.inner.set_active_charset(i); }
    fn configure_charset(&mut self, i: CharsetIndex, s: StandardCharset) {
        self.inner.configure_charset(i, s);
    }
    fn set_color(&mut self, i: usize, c: Rgb) { self.inner.set_color(i, c); }
    fn dynamic_color_sequence(&mut self, s: String, i: usize, t: &str) {
        self.inner.dynamic_color_sequence(s, i, t);
    }
    fn reset_color(&mut self, i: usize) { self.inner.reset_color(i); }
    fn clipboard_store(&mut self, b: u8, d: &[u8]) { self.inner.clipboard_store(b, d); }
    fn clipboard_load(&mut self, b: u8, s: &str) { self.inner.clipboard_load(b, s); }
    fn decaln(&mut self) { self.inner.decaln(); }
    fn push_title(&mut self) { self.inner.push_title(); }
    fn pop_title(&mut self) { self.inner.pop_title(); }
    fn text_area_size_pixels(&mut self) { self.inner.text_area_size_pixels(); }
    fn text_area_size_chars(&mut self) { self.inner.text_area_size_chars(); }
    fn set_hyperlink(&mut self, h: Option<Hyperlink>) { self.inner.set_hyperlink(h); }
    fn set_mouse_cursor_icon(&mut self, i: CursorIcon) { self.inner.set_mouse_cursor_icon(i); }
    fn report_keyboard_mode(&mut self) { self.inner.report_keyboard_mode(); }
    fn push_keyboard_mode(&mut self, m: KeyboardModes) { self.inner.push_keyboard_mode(m); }
    fn pop_keyboard_modes(&mut self, n: u16) { self.inner.pop_keyboard_modes(n); }
    fn set_keyboard_mode(&mut self, m: KeyboardModes, b: KeyboardModesApplyBehavior) {
        self.inner.set_keyboard_mode(m, b);
    }
    fn set_modify_other_keys(&mut self, m: ModifyOtherKeys) {
        self.inner.set_modify_other_keys(m);
    }
    fn report_modify_other_keys(&mut self) { self.inner.report_modify_other_keys(); }
    fn set_scp(&mut self, c: ScpCharPath, u: ScpUpdateMode) { self.inner.set_scp(c, u); }
}

/// Per-call byte stream analysis: detect `?2026h/l/$p` boundaries,
/// mutate `in_sync` through them, and return a filtered buffer with
/// those sequences stripped. The stripped sequences are what keep
/// alacritty_terminal's parser-level sync stash from activating —
/// with them gone, our handler gets called live for every byte and
/// the LF-suppression heuristic can actually run.
///
/// Returns `Ok(Cow::Borrowed)` on the fast path (no sync sequence in
/// the chunk), or `Cow::Owned` when a copy was required.
pub fn strip_sync_and_track<'a>(
    buf: &'a [u8],
    in_sync: &mut bool,
) -> std::borrow::Cow<'a, [u8]> {
    const BEGIN: &[u8] = b"\x1b[?2026h";
    const END: &[u8] = b"\x1b[?2026l";
    const QUERY: &[u8] = b"\x1b[?2026$p";
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    if !contains(BEGIN) && !contains(END) && !contains(QUERY) {
        return std::borrow::Cow::Borrowed(buf);
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(BEGIN) {
            *in_sync = true;
            i += BEGIN.len();
        } else if buf[i..].starts_with(END) {
            *in_sync = false;
            i += END.len();
        } else if buf[i..].starts_with(QUERY) {
            i += QUERY.len();
        } else {
            out.push(buf[i]);
            i += 1;
        }
    }
    std::borrow::Cow::Owned(out)
}

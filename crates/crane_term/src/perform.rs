//! Bridge from `vte::ansi::Handler` to our [`Handler`].
//!
//! `vte` parses bytes and dispatches typed events through its own
//! `Handler` trait. We adapt those callbacks into our trait shape.
//! The translation is mechanical; the only conceptual difference
//! is that our scroll-producing methods return [`ScrollDelta`], so
//! we capture and discard those returns when `vte::ansi::Handler`
//! expects `()`.
//!
//! Forwarding every method we care about is critical: vte's
//! default trait methods are no-ops. Any method we don't override
//! here silently swallows the parsed event, which is how `\e[2J`
//! (clear screen) and SGR colors went missing in the first
//! integration cut. Anything we DON'T need (kitty image protocol,
//! tmux control mode, etc.) intentionally falls through to the
//! no-op default — those have no Crane equivalent.

use crate::handler::Handler;

pub struct Bridge<'a, H: Handler> {
    pub inner: &'a mut H,
}

impl<H: Handler> vte::ansi::Handler for Bridge<'_, H> {
    // ---- character / cursor ----

    fn input(&mut self, c: char) {
        self.inner.input(c);
    }

    fn goto(&mut self, line: i32, col: usize) {
        let line = line.max(0) as usize;
        self.inner.goto(line, col);
    }

    fn goto_line(&mut self, line: i32) {
        self.inner.goto_line(line.max(0) as usize);
    }

    fn goto_col(&mut self, col: usize) {
        self.inner.goto_col(col);
    }

    fn move_up(&mut self, n: usize) {
        self.inner.move_up(n);
    }

    fn move_down(&mut self, n: usize) {
        self.inner.move_down(n);
    }

    fn move_forward(&mut self, n: usize) {
        self.inner.move_forward(n);
    }

    fn move_backward(&mut self, n: usize) {
        self.inner.move_backward(n);
    }

    fn move_up_and_cr(&mut self, n: usize) {
        self.inner.move_up_and_cr(n);
    }

    fn move_down_and_cr(&mut self, n: usize) {
        self.inner.move_down_and_cr(n);
    }

    fn backspace(&mut self) {
        self.inner.backspace();
    }

    fn carriage_return(&mut self) {
        self.inner.carriage_return();
    }

    fn put_tab(&mut self, count: u16) {
        self.inner.put_tab(count);
    }

    fn save_cursor_position(&mut self) {
        self.inner.save_cursor();
    }

    fn restore_cursor_position(&mut self) {
        self.inner.restore_cursor();
    }

    // ---- scroll-producing (return type erased) ----

    fn linefeed(&mut self) {
        let _ = self.inner.linefeed();
    }

    fn newline(&mut self) {
        self.inner.newline();
    }

    fn reverse_index(&mut self) {
        let _ = self.inner.reverse_index();
    }

    fn scroll_up(&mut self, n: usize) {
        let _ = self.inner.scroll_up(n);
    }

    fn scroll_down(&mut self, n: usize) {
        let _ = self.inner.scroll_down(n);
    }

    fn insert_blank_lines(&mut self, n: usize) {
        let _ = self.inner.insert_blank_lines(n);
    }

    fn delete_lines(&mut self, n: usize) {
        let _ = self.inner.delete_lines(n);
    }

    // ---- in-line mutation ----

    fn insert_blank(&mut self, n: usize) {
        self.inner.insert_blank(n);
    }

    fn erase_chars(&mut self, n: usize) {
        self.inner.erase_chars(n);
    }

    fn delete_chars(&mut self, n: usize) {
        self.inner.delete_chars(n);
    }

    fn clear_line(&mut self, mode: vte::ansi::LineClearMode) {
        self.inner.clear_line(mode);
    }

    fn clear_screen(&mut self, mode: vte::ansi::ClearMode) {
        self.inner.clear_screen(mode);
    }

    fn clear_tabs(&mut self, mode: vte::ansi::TabulationClearMode) {
        self.inner.clear_tabs(mode);
    }

    fn set_horizontal_tabstop(&mut self) {
        self.inner.set_horizontal_tabstop();
    }

    // ---- attribute / mode ----

    fn terminal_attribute(&mut self, attr: vte::ansi::Attr) {
        self.inner.terminal_attribute(attr);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // vte uses 1-based line indices for the CSI parameter.
        self.inner
            .set_scrolling_region(top.saturating_sub(1), bottom);
    }

    fn set_mode(&mut self, mode: vte::ansi::Mode) {
        self.inner.set_mode(mode);
    }

    fn unset_mode(&mut self, mode: vte::ansi::Mode) {
        self.inner.unset_mode(mode);
    }

    fn set_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        self.inner.set_private_mode(mode);
    }

    fn unset_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        self.inner.unset_private_mode(mode);
    }

    fn set_keypad_application_mode(&mut self) {
        self.inner.set_keypad_application_mode();
    }

    fn unset_keypad_application_mode(&mut self) {
        self.inner.unset_keypad_application_mode();
    }

    fn reset_state(&mut self) {
        self.inner.reset_state();
    }

    // ---- terminal queries (push outbound replies via Handler) ----

    fn device_status(&mut self, n: usize) {
        self.inner.device_status(n);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        self.inner.identify_terminal(intermediate);
    }

    // ---- title / bell ----

    fn set_title(&mut self, title: Option<String>) {
        self.inner.set_title(title);
    }

    fn bell(&mut self) {
        self.inner.bell();
    }
}

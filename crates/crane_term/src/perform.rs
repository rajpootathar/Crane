//! Bridge from `vte::ansi::Handler` to our [`Handler`].
//!
//! `vte` parses bytes and dispatches typed events through its own
//! `Handler` trait. We adapt those callbacks into our trait shape.
//! The translation is mostly mechanical; the only conceptual
//! difference is that our scroll-producing methods return a
//! [`ScrollDelta`], so we capture and discard those returns when
//! `vte::ansi::Handler` expects `()`.
//!
//! v1 wires only the cursor / scroll / mode / input subset that
//! the linefeed-routing test suite exercises. The remaining methods
//! are filled in as we land specific TUI fixtures.

use crate::handler::Handler;

pub struct Bridge<'a, H: Handler> {
    pub inner: &'a mut H,
}

impl<H: Handler> vte::ansi::Handler for Bridge<'_, H> {
    fn input(&mut self, c: char) {
        self.inner.input(c);
    }

    fn linefeed(&mut self) {
        let _ = self.inner.linefeed();
    }

    fn carriage_return(&mut self) {
        self.inner.carriage_return();
    }

    fn backspace(&mut self) {
        self.inner.backspace();
    }

    fn move_up(&mut self, n: usize) {
        self.inner.move_up(n);
    }

    fn move_down(&mut self, n: usize) {
        self.inner.move_down(n);
    }

    fn goto(&mut self, line: i32, col: usize) {
        let line = line.max(0) as usize;
        self.inner.goto(line, col);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // vte uses 1-based line indices for the CSI parameter; the
        // top is already converted before the parser hands it off.
        // The off-by-one is the caller's responsibility, so we
        // forward as-is.
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

    fn device_status(&mut self, n: usize) {
        // Replies accumulate on the Term itself; Crane's PTY
        // reader thread drains them via Term::take_pty_replies()
        // after each parse pass. One place owns outbound
        // bookkeeping.
        self.inner.device_status(n);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        self.inner.identify_terminal(intermediate);
    }
}

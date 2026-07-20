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
//!
//! [`OscWatcher`] is a separate, low-level `vte::Perform` impl that
//! only reacts to OSC 9 / OSC 777 (desktop-notification escapes the
//! `ansi::Handler` trait does not surface). The [`crate::processor`]
//! drives both adapters from the same byte stream so the existing
//! grid/scrollback parse path is unchanged.

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

    // ---- cursor presentation ----

    fn set_cursor_style(&mut self, style: Option<vte::ansi::CursorStyle>) {
        // Map vte's shape set onto our three DECSCUSR-reachable
        // shapes. `HollowBlock` / `Hidden` are only produced by
        // escapes we don't surface, but collapse them onto `Block` so
        // the mapping is total. `None` (DECSCUSR 0) is forwarded as-is
        // and interpreted as "reset to default" by the Term.
        let mapped = style.map(|s| {
            let shape = match s.shape {
                vte::ansi::CursorShape::Underline => crate::handler::CursorShape::Underline,
                vte::ansi::CursorShape::Beam => crate::handler::CursorShape::Beam,
                vte::ansi::CursorShape::Block
                | vte::ansi::CursorShape::HollowBlock
                | vte::ansi::CursorShape::Hidden => crate::handler::CursorShape::Block,
            };
            crate::handler::CursorStyle {
                shape,
                blink: s.blinking,
            }
        });
        self.inner.set_cursor_style(mapped);
    }

    // ---- title / bell ----

    fn set_title(&mut self, title: Option<String>) {
        self.inner.set_title(title);
    }

    fn bell(&mut self) {
        self.inner.bell();
    }
}

// ---------------------------------------------------------------------------
// OSC notification watcher
// ---------------------------------------------------------------------------

/// Low-level `vte::Perform` adapter that intercepts OSC 9 / OSC 777
/// (desktop-notification escapes) and forwards their payload to our
/// [`Handler::osc_notification`]. Every other Perform callback is a
/// no-op — the existing [`Bridge`] / `vte::ansi::Processor` path owns
/// real grid mutation. Running this as a separate parser instance is
/// cheaper than reimplementing `ansi::Handler` and keeps the OSC
/// surface independent of vte's `Handler` trait shape, which has no
/// callback for 9 / 777.
///
/// **OSC 9** is the de-facto iTerm2 notification convention:
///   `\e]9;<utf8 text>\a`
///
/// **OSC 777** is the urgency-aware variant used by some senders
/// (xdotool, libnotify-bridges):
///   `\e]777;notify;<title>;<body>\a`
/// We treat any OSC 777 with a `notify` sub-action as urgent and join
/// remaining params with " — " for display.
pub struct OscWatcher<'a, H: Handler> {
    pub inner: &'a mut H,
}

impl<H: Handler> vte::Perform for OscWatcher<'_, H> {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() || params[0].is_empty() {
            return;
        }
        match params[0] {
            // OSC 9 — iTerm2-style notification. Remaining params
            // joined with ';' (a body may legitimately contain
            // semicolons when an upstream embedded them; we round-
            // trip).
            b"9" => {
                if params.len() < 2 {
                    return;
                }
                let body = join_params_utf8(&params[1..]);
                if !body.is_empty() {
                    self.inner.osc_notification(&body, false);
                }
            }
            // OSC 777 — urgency-aware. Expected shape:
            //   777 ; notify ; title ; body
            // but the trailing fields are optional. We forward
            // whatever is present, joining with " — " when both
            // title and body are non-empty so the toast shows
            // "title — body".
            b"777" => {
                if params.len() < 2 {
                    return;
                }
                let action = std::str::from_utf8(params[1]).unwrap_or("");
                if !action.eq_ignore_ascii_case("notify") {
                    return;
                }
                let title = params
                    .get(2)
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .unwrap_or_default();
                let body = params
                    .get(3)
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .unwrap_or_default();
                let combined = match (title.is_empty(), body.is_empty()) {
                    (true, true) => return,
                    (false, true) => title,
                    (true, false) => body,
                    (false, false) => format!("{title} — {body}"),
                };
                self.inner.osc_notification(&combined, true);
            }
            // OSC 10 / 11 / 12 — dynamic colour query. When an app sends
            //   OSC 11 ; ? ST
            // it is asking for the terminal's default background so it can
            // pick readable text for a light vs dark theme. We only answer the
            // *query* form (a `?` payload); the set form (an app changing our
            // colours) is ignored. Without this reply, apps assume a dark
            // background and render light text — unreadable on a light theme.
            b"10" | b"11" | b"12" => {
                if params.get(1).map(|p| *p == b"?").unwrap_or(false) {
                    // SAFETY: matched literals above are valid ASCII digits.
                    let index = std::str::from_utf8(params[0])
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok());
                    if let Some(index) = index {
                        self.inner.osc_color_query(index);
                    }
                }
            }
            _ => {}
        }
    }
}

fn join_params_utf8(parts: &[&[u8]]) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push(';');
        }
        out.push_str(&String::from_utf8_lossy(p));
    }
    out
}

#[cfg(test)]
mod osc_tests {
    use super::*;
    use crate::handler::Handler;

    #[derive(Default)]
    struct Collector {
        events: Vec<(String, bool)>,
        color_queries: Vec<u16>,
    }

    impl Handler for Collector {
        fn osc_notification(&mut self, body: &str, urgent: bool) {
            self.events.push((body.to_string(), urgent));
        }
        fn osc_color_query(&mut self, index: u16) {
            self.color_queries.push(index);
        }
    }

    fn run(bytes: &[u8]) -> Vec<(String, bool)> {
        let mut parser = vte::Parser::new();
        let mut sink = Collector::default();
        let mut watcher = OscWatcher { inner: &mut sink };
        parser.advance(&mut watcher, bytes);
        sink.events
    }

    fn run_color_queries(bytes: &[u8]) -> Vec<u16> {
        let mut parser = vte::Parser::new();
        let mut sink = Collector::default();
        let mut watcher = OscWatcher { inner: &mut sink };
        parser.advance(&mut watcher, bytes);
        sink.color_queries
    }

    #[test]
    fn osc9_bel_terminated() {
        let evts = run(b"\x1b]9;Claude is done\x07");
        assert_eq!(evts, vec![("Claude is done".into(), false)]);
    }

    #[test]
    fn osc9_st_terminated() {
        let evts = run(b"\x1b]9;hello world\x1b\\");
        assert_eq!(evts, vec![("hello world".into(), false)]);
    }

    #[test]
    fn osc777_notify_title_and_body_marked_urgent() {
        let evts = run(b"\x1b]777;notify;Crane;Build failed\x07");
        assert_eq!(evts, vec![("Crane \u{2014} Build failed".into(), true)]);
    }

    #[test]
    fn osc777_non_notify_action_ignored() {
        let evts = run(b"\x1b]777;set;title;body\x07");
        assert!(evts.is_empty());
    }

    #[test]
    fn unrelated_osc_codes_ignored() {
        let evts = run(b"\x1b]2;new title\x07\x1b]4;1;rgb:ff/00/00\x07");
        assert!(evts.is_empty());
    }

    #[test]
    fn osc11_background_query_surfaced() {
        assert_eq!(run_color_queries(b"\x1b]11;?\x07"), vec![11]);
    }

    #[test]
    fn osc10_and_osc12_queries_surfaced() {
        assert_eq!(run_color_queries(b"\x1b]10;?\x1b\\"), vec![10]);
        assert_eq!(run_color_queries(b"\x1b]12;?\x07"), vec![12]);
    }

    #[test]
    fn osc11_set_form_is_not_a_query() {
        // An app *setting* the background (no `?`) must not trigger a reply.
        assert!(run_color_queries(b"\x1b]11;rgb:00/00/00\x07").is_empty());
    }

    #[test]
    fn empty_osc9_body_dropped() {
        let evts = run(b"\x1b]9;\x07");
        assert!(evts.is_empty());
    }

    /// Regression guard for the integration path: text → OSC 9 → more
    /// text in one chunk. The parser must surface exactly one
    /// notification and leave the surrounding text undisturbed. We
    /// only assert on the notification side here — grid mutation is
    /// driven by the separate `ansi::Processor` in
    /// `crate::Processor`.
    #[test]
    fn osc9_inline_with_text_emits_once() {
        let evts = run(b"hello \x1b]9;ping\x07 world");
        assert_eq!(evts, vec![("ping".into(), false)]);
    }
}

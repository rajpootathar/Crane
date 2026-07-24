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

use crate::handler::{Handler, ShellIntegrationEvent};

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
            // OSC 633 — VS Code shell-integration. Reports prompt boundaries,
            // the command line, cwd, and exit code so Crane can record history.
            b"633" => {
                let Some(sub) = params.get(1).and_then(|p| p.first().copied()) else {
                    return;
                };
                let event = match sub {
                    b'A' => Some(ShellIntegrationEvent::PromptStart),
                    b'B' => Some(ShellIntegrationEvent::CommandStart),
                    b'C' => Some(ShellIntegrationEvent::PreExec),
                    b'D' => {
                        let exit = params
                            .get(2)
                            .and_then(|p| std::str::from_utf8(p).ok())
                            .and_then(|s| s.trim().parse::<i32>().ok());
                        Some(ShellIntegrationEvent::CommandFinished { exit })
                    }
                    b'E' => params
                        .get(2)
                        .map(|p| ShellIntegrationEvent::CommandLine(unescape_osc633(p))),
                    b'P' => params.get(2).and_then(|p| {
                        // 633;P carries `<key>=<value>` properties. `Cwd=` and
                        // `Keymap=` are the two we consume; any other property
                        // (VS Code emits several) falls through to `None` and is
                        // silently ignored.
                        let s = String::from_utf8_lossy(p);
                        if let Some(cwd) = s.strip_prefix("Cwd=") {
                            Some(ShellIntegrationEvent::Cwd(cwd.to_string()))
                        } else {
                            s.strip_prefix("Keymap=")
                                .map(|k| ShellIntegrationEvent::Keymap(k.to_string()))
                        }
                    }),
                    _ => None,
                };
                if let Some(event) = event {
                    self.inner.shell_integration(event);
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

/// Reverse VS Code's OSC 633 payload escaping: `\xHH` hex escapes back to
/// their byte, so a command line containing `;` (encoded `\x3b`) or a newline
/// (`\x0a`) round-trips intact. Unknown/malformed escapes are passed through
/// literally.
///
/// Decoded output is accumulated as **bytes**, not `char`s: VS Code escapes
/// non-ASCII text byte-by-byte, so a single multi-byte UTF-8 character (e.g.
/// `é` = `\xc3\xa9`) arrives as a sequence of single-byte escapes. Pushing
/// each decoded byte through `byte as char` would reinterpret it as a
/// Latin-1 codepoint and produce mojibake (`Ã©`) instead of reconstructing
/// the original character. Buffering raw bytes and converting once at the
/// end with `String::from_utf8_lossy` lets adjacent escapes recombine into
/// their intended multi-byte sequence; genuinely invalid UTF-8 degrades to
/// replacement characters instead of silently wrong text.
fn unescape_osc633(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let mut buf: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.peek() == Some(&'x') {
                chars.next();
                let h1 = chars.next();
                let h2 = chars.next();
                if let (Some(a), Some(b)) = (h1, h2) {
                    if let Ok(byte) = u8::from_str_radix(&format!("{a}{b}"), 16) {
                        buf.push(byte);
                        continue;
                    }
                }
                // Malformed escape — emit what we consumed literally.
                buf.push(b'\\');
                buf.push(b'x');
                if let Some(a) = h1 {
                    buf.extend_from_slice(a.encode_utf8(&mut [0u8; 4]).as_bytes());
                }
                if let Some(b) = h2 {
                    buf.extend_from_slice(b.encode_utf8(&mut [0u8; 4]).as_bytes());
                }
                continue;
            }
            if chars.peek() == Some(&'\\') {
                chars.next();
                buf.push(b'\\');
                continue;
            }
        }
        buf.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod osc_tests {
    use super::*;
    use crate::handler::{Handler, ShellIntegrationEvent};

    #[derive(Default)]
    struct Collector {
        events: Vec<(String, bool)>,
        color_queries: Vec<u16>,
        shell_events: Vec<ShellIntegrationEvent>,
    }

    impl Handler for Collector {
        fn osc_notification(&mut self, body: &str, urgent: bool) {
            self.events.push((body.to_string(), urgent));
        }
        fn osc_color_query(&mut self, index: u16) {
            self.color_queries.push(index);
        }
        fn shell_integration(&mut self, event: ShellIntegrationEvent) {
            self.shell_events.push(event);
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

    fn run_shell_events(bytes: &[u8]) -> Vec<ShellIntegrationEvent> {
        let mut parser = vte::Parser::new();
        let mut sink = Collector::default();
        let mut watcher = OscWatcher { inner: &mut sink };
        parser.advance(&mut watcher, bytes);
        sink.shell_events
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

    #[test]
    fn osc633_boundaries_and_payloads_decode() {
        use ShellIntegrationEvent::*;
        assert!(matches!(run_shell_events(b"\x1b]633;A\x07").as_slice(), [PromptStart]));
        assert!(matches!(run_shell_events(b"\x1b]633;B\x07").as_slice(), [CommandStart]));
        assert!(matches!(run_shell_events(b"\x1b]633;C\x07").as_slice(), [PreExec]));
        assert!(matches!(
            run_shell_events(b"\x1b]633;D;0\x07").as_slice(),
            [CommandFinished { exit: Some(0) }]
        ));
        assert!(matches!(
            run_shell_events(b"\x1b]633;D;130\x07").as_slice(),
            [CommandFinished { exit: Some(130) }]
        ));
        assert_eq!(
            run_shell_events(b"\x1b]633;E;git commit\x07"),
            vec![CommandLine("git commit".into())]
        );
        assert_eq!(
            run_shell_events(b"\x1b]633;P;Cwd=/Users/x/proj\x07"),
            vec![Cwd("/Users/x/proj".into())]
        );
        assert_eq!(
            run_shell_events(b"\x1b]633;P;Keymap=vi\x07"),
            vec![Keymap("vi".into())]
        );
        assert_eq!(
            run_shell_events(b"\x1b]633;P;Keymap=emacs\x07"),
            vec![Keymap("emacs".into())]
        );
    }

    #[test]
    fn osc633_unknown_p_property_ignored() {
        // A `P;<other>=` property Crane does not consume (VS Code emits several)
        // must decode to nothing rather than erroring or mis-classifying.
        assert!(run_shell_events(b"\x1b]633;P;IsWindows=True\x07").is_empty());
    }

    #[test]
    fn osc633_command_line_unescapes_semicolons_and_newlines() {
        // VS Code encodes ; as \x3b, newline as \x0a, backslash as \x5c.
        assert_eq!(
            run_shell_events(b"\x1b]633;E;echo a\\x3bb\\x0ac\x07"),
            vec![ShellIntegrationEvent::CommandLine("echo a;b\nc".into())]
        );
    }

    #[test]
    fn osc633_unknown_subcommand_ignored() {
        assert!(run_shell_events(b"\x1b]633;Z;whatever\x07").is_empty());
    }

    #[test]
    fn osc633_command_line_unescapes_backslash() {
        // VS Code encodes a literal backslash as \x5c. Nothing previously
        // pinned this third escape — a refactor of `unescape_osc633` could
        // silently regress it.
        assert_eq!(
            run_shell_events(b"\x1b]633;E;echo a\\x5cb\x07"),
            vec![ShellIntegrationEvent::CommandLine("echo a\\b".into())]
        );
    }

    #[test]
    fn osc633_command_line_reconstructs_multibyte_utf8() {
        // \xc3\xa9 is 'é' UTF-8-encoded and then escaped byte-by-byte, the
        // way VS Code escapes non-ASCII text. The two escapes must
        // recombine into the original UTF-8 sequence, not decode as two
        // separate Latin-1 codepoints (`Ã©`).
        assert_eq!(
            run_shell_events(b"\x1b]633;E;echo \\xc3\\xa9\x07"),
            vec![ShellIntegrationEvent::CommandLine("echo é".into())]
        );
    }
}

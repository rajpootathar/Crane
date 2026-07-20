//! Framework-agnostic terminal controller. Owns the PTY (portable-pty),
//! drives `crane_term`, and runs the reader thread. Ported from Crane's
//! `src/terminal/term.rs`, with the only egui coupling (`Context` for
//! `request_repaint`) replaced by a `Wake` callback.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crane_term::{Processor, Term, TermNotification};

/// Called from the reader thread when the grid changes and the UI should
/// repaint. Must be cheap and thread-safe (e.g. send on a channel).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct TerminalController {
    pub term: Arc<Mutex<Term>>,
    pub parser: Arc<Mutex<Processor>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader_handle: Option<thread::JoinHandle<()>>,
    pub cols: usize,
    pub rows: usize,
    alive: Arc<AtomicBool>,
    /// Latched BEL (0x07): the reader thread drains `Term::take_bell()` into
    /// this atomic (which also guarantees a UI wake even for a bare bell that
    /// doesn't otherwise dirty the grid) and the UI drains it via `take_bell`.
    bell: Arc<AtomicBool>,
    /// Second, independent BEL latch used ONLY to drive the Left-Panel attention
    /// pulse. Set alongside `bell` in the reader thread and drained by the
    /// TerminalView's repaint stream (`take_bell_notify`). Kept separate from
    /// `bell` so consuming it for attention never steals the audible beep the
    /// paint path drains from `bell`.
    bell_notify: Arc<AtomicBool>,
    /// Desktop notifications (OSC 9 / OSC 777) drained off the `Term` queue by
    /// the reader thread and buffered here until the UI thread forwards them to
    /// the shell (mirrors the `bell` latch: draining on the reader thread also
    /// guarantees a UI wake even when the notification didn't dirty the grid).
    notif_queue: Arc<Mutex<Vec<TermNotification>>>,
    /// The directory the shell was spawned in — persisted so a restored
    /// session reopens the terminal in the same place (old Crane parity).
    pub cwd: std::path::PathBuf,
}

impl TerminalController {
    pub fn new(cols: usize, rows: usize, cwd: Option<&Path>, wake: Wake) -> std::io::Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let mut cmd = {
            #[cfg(unix)]
            {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
                CommandBuilder::new(shell)
            }
            #[cfg(not(unix))]
            {
                CommandBuilder::new_default_prog()
            }
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Crane");
        cmd.env_remove("VTE_VERSION");
        // COLORFGBG is the static (env-var) counterpart to the OSC 11 query
        // above: the rxvt convention many CLIs read at startup to detect a
        // light vs dark terminal. Format "<fg>;<bg>" as ANSI indices; the bg
        // field is what matters — index 15 (white) → light, 0 (black) → dark.
        // Fixed at spawn (a live theme switch won't update it, but apps read it
        // once at launch anyway; the OSC 11 path covers the live case).
        {
            let th = crate::theme::current();
            let lum = 0.299 * th.terminal_bg.r as f32
                + 0.587 * th.terminal_bg.g as f32
                + 0.114 * th.terminal_bg.b as f32;
            cmd.env("COLORFGBG", if lum > 128.0 { "0;15" } else { "15;0" });
        }
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        } else if let Some(home) = std::env::var_os("HOME") {
            cmd.cwd(home);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| std::io::Error::other(e.to_string()))?,
        ));

        let term = Arc::new(Mutex::new(Term::new(rows, cols)));
        // Seed the Term with the active theme's colours so OSC 10/11/12 queries
        // answer with the truth — an app that asks "is the background light or
        // dark?" gets the real answer and picks readable text, instead of
        // assuming dark and rendering light-on-light on a light theme.
        {
            let th = crate::theme::current();
            let fg = (th.terminal_fg.r, th.terminal_fg.g, th.terminal_fg.b);
            let bg = (th.terminal_bg.r, th.terminal_bg.g, th.terminal_bg.b);
            term.lock().set_default_colors(fg, bg, fg);
        }
        let parser = Arc::new(Mutex::new(Processor::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let bell = Arc::new(AtomicBool::new(false));
        let bell_notify = Arc::new(AtomicBool::new(false));
        let notif_queue: Arc<Mutex<Vec<TermNotification>>> = Arc::new(Mutex::new(Vec::new()));

        // Reader thread: PTY -> crane_term, write back replies, wake the UI.
        // Lock order is ALWAYS parser-then-term (deadlock-critical).
        let reader_handle = {
            let term = term.clone();
            let parser = parser.clone();
            let writer = writer.clone();
            let alive = alive.clone();
            let bell = bell.clone();
            let bell_notify = bell_notify.clone();
            let notif_queue = notif_queue.clone();
            Some(thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 8192];
                let mut last_epoch = 0u64;
                let mut last_cursor = (usize::MAX, usize::MAX);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let (replies, epoch, cursor, rang, notes);
                            {
                                let mut p = parser.lock();
                                let mut t = term.lock();
                                p.parse_bytes(&mut *t, &buf[..n]);
                                replies = t.take_pty_replies();
                                epoch = t.dirty_epoch;
                                cursor = (t.grid.cursor.row, t.grid.cursor.col);
                                // Drain the BEL latch here so a bare bell (which
                                // doesn't dirty the grid) still forces a wake and
                                // reaches the UI via the atomic below.
                                rang = t.take_bell();
                                // Drain OSC 9 / OSC 777 desktop notifications for
                                // the same reason: a bare notification may not
                                // dirty the grid, so buffer + force a wake below.
                                notes = t.take_notifications();
                            }
                            // Write replies BEFORE the next read (P10k workaround).
                            if !replies.is_empty() {
                                let mut w = writer.lock();
                                let _ = w.write_all(&replies);
                                let _ = w.flush();
                            }
                            if rang {
                                bell.store(true, Ordering::Relaxed);
                                bell_notify.store(true, Ordering::Relaxed);
                            }
                            let has_notes = !notes.is_empty();
                            if has_notes {
                                notif_queue.lock().extend(notes);
                            }
                            if epoch != last_epoch || cursor != last_cursor || rang || has_notes {
                                last_epoch = epoch;
                                last_cursor = cursor;
                                wake();
                            }
                        }
                    }
                }
                alive.store(false, Ordering::Relaxed);
                wake();
            }))
        };

        let cwd = cwd
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .unwrap_or_default();
        Ok(Self {
            term,
            parser,
            writer,
            master: pair.master,
            child: Some(child),
            reader_handle,
            cols,
            rows,
            alive,
            bell,
            bell_notify,
            notif_queue,
            cwd,
        })
    }

    /// Drain the desktop notifications (OSC 9 / OSC 777) buffered by the reader
    /// thread since the last call. `&self` (interior `Arc<Mutex>`) so the render
    /// / repaint path can forward them without `&mut`. Each is routed to the
    /// shell as a `CraneShellAction::TermNotification`; the toast itself is
    /// rendered by the shell, not here.
    pub fn take_notifications(&self) -> Vec<TermNotification> {
        std::mem::take(&mut *self.notif_queue.lock())
    }

    /// The terminal's window title (OSC 0 / OSC 2), if the shell or a program
    /// set one; `None` until the first title escape arrives.
    pub fn title(&self) -> Option<String> {
        self.term.lock().window_title().map(|s| s.to_string())
    }

    /// Read-and-clear the BEL latch: `true` when a bell rang since the last
    /// call. Drained by the reader thread into an atomic so it always survives
    /// to the UI even when the bell didn't otherwise dirty the grid.
    pub fn take_bell(&self) -> bool {
        self.bell.swap(false, Ordering::Relaxed)
    }

    /// Read-and-clear the attention-only BEL latch. Independent of `take_bell`
    /// so draining it to pulse the sidebar never suppresses the audible beep.
    pub fn take_bell_notify(&self) -> bool {
        self.bell_notify.swap(false, Ordering::Relaxed)
    }

    /// Render the current grid + scrollback to an ANSI snapshot (for session
    /// persistence). Reuses `crane_term::Term::snapshot_ansi`.
    pub fn snapshot(&self) -> String {
        self.term.lock().snapshot_ansi()
    }

    /// Replay a persisted ANSI history into the terminal (session restore).
    /// Feeds the bytes through the VT parser so colors/decorations survive, then
    /// homes the cursor so the live shell prompt appends cleanly after it.
    pub fn replay(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut parser = self.parser.lock();
        let mut term = self.term.lock();
        parser.parse_bytes(&mut *term, text.as_bytes());
        parser.parse_bytes(&mut *term, b"\r\n");
    }

    /// Write input bytes to the PTY. `&self` (interior Arc<Mutex>) so the
    /// render closure can call it without `&mut`.
    pub fn write_input(&self, data: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(data);
        let _ = w.flush();
        drop(w);
        // Typing snaps the viewport back to the live screen (like old Crane's
        // pending_scroll_to_bottom).
        self.term.lock().scroll_to_bottom();
    }

    /// Resize the PTY + grid. NOTE arg order: controller is (cols, rows),
    /// but `Term::resize` is (rows, cols).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        // Commit only on a successful kernel resize, so a failure isn't
        // latched by the early-return guard (it retries next frame).
        if let Err(e) = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            eprintln!("warp_term_spike: pty resize failed: {e}");
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(rows, cols);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// True when an alt-screen TUI (vim, htop, less, etc.) is active.
    ///
    /// Used as a proxy for `has_foreground_process()`: crane_term has no
    /// foreground-pgid API, but an alt-screen implies a TUI owns the viewport,
    /// which is exactly the case where we must not send a full cursor-home +
    /// erase-display sequence. Bare shells never enter alt-screen, so the proxy
    /// is correct for the clear-screen use-case. Limitation: a CPU-spinning
    /// background process that does not use alt-screen is indistinguishable from
    /// an idle shell prompt — same behaviour as iTerm2 / Terminal.app.
    pub fn has_foreground_process(&self) -> bool {
        self.term.lock().is_alt_screen()
    }

    /// Two-regime Cmd+K clear (matches old egui Crane `src/terminal/view.rs`
    /// lines 1569-1599):
    ///
    /// • **TUI / alt-screen active** (`has_foreground_process()` == true):
    ///   erase scrollback only — `\x1b[3J` — so the TUI widget is left intact
    ///   and its next write lands in the right place.
    ///
    /// • **Bare shell** (no alt-screen): cursor home + erase display + erase
    ///   scrollback (`\x1b[H\x1b[2J\x1b[3J`) parsed directly into crane_term,
    ///   then `\x0c` (Ctrl+L) sent to the PTY so zsh/bash repaints the prompt
    ///   at row 0. `\x1b[3J` triggers `ClearMode::Saved` in crane_term which
    ///   calls `scrollback.clear()` — no separate byte-log flush is needed.
    pub fn clear_screen_two_regime(&self) {
        let tui_active = self.has_foreground_process();
        {
            // Lock order: parser then term (same order as the reader thread —
            // critical for deadlock avoidance).
            let mut p = self.parser.lock();
            let mut t = self.term.lock();
            if tui_active {
                p.parse_bytes(&mut *t, b"\x1b[3J");
            } else {
                p.parse_bytes(&mut *t, b"\x1b[H\x1b[2J\x1b[3J");
                t.scroll_to_bottom();
            }
        }
        if !tui_active {
            // Ask the shell to repaint its prompt at row 0.
            self.write_input(b"\x0c");
        }
    }
}

impl Drop for TerminalController {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // With the child dead, the master read side hits EOF; join the
        // reader so it can't outlive the controller (order-independent,
        // not relying on struct field drop order).
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

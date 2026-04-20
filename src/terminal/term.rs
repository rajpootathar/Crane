use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone)]
pub struct WakeListener {
    ctx: egui::Context,
    /// Queue of VT-parser replies (CSI 6n / DSR / title ack) the
    /// Terminal drains and writes back to the PTY on each render.
    /// Deliberately a queue rather than a direct write: making these
    /// replies synchronous (write from the listener in-line) exposed
    /// a Powerlevel10k width-miscount bug where P10k computes its
    /// RPROMPT cursor-back against byte-width instead of column-width
    /// of Nerd Font icons, landing the cursor a few columns short of
    /// the prompt end. With a small delay, P10k's internal timeout
    /// falls through to an absolute-positioning code path that
    /// doesn't rely on width counting.
    pty_replies: Arc<Mutex<Vec<u8>>>,
}

impl EventListener for WakeListener {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(s) = event {
            let bytes = s.as_bytes();
            // Suppress CSI 6n / DSR cursor-position reports (CPR).
            // Powerlevel10k computes RPROMPT cursor-back math against
            // byte-width instead of column-width for Nerd-Font icons;
            // when we reply with a correct CPR it lands the cursor a
            // few columns short of the prompt end, so typed chars
            // overwrite the prompt. The in-place "queue then drain per
            // frame" delay isn't long enough at 60fps to beat P10k's
            // ~20ms internal timeout. Dropping CPRs lets P10k always
            // fall through to its absolute-positioning fallback that
            // doesn't rely on width counting. Other replies (DA, title
            // ack, etc.) still pass through.
            // CPR format: `\x1b[<row>;<col>R` or `\x1b[<row>R`.
            let is_cpr = bytes.starts_with(b"\x1b[")
                && bytes.ends_with(b"R")
                && bytes[2..bytes.len() - 1]
                    .iter()
                    .all(|&b| b.is_ascii_digit() || b == b';');
            if !is_cpr {
                self.pty_replies.lock().extend_from_slice(bytes);
            }
        }
        self.ctx.request_repaint();
    }
}

const HISTORY_MAX: usize = 256 * 1024;

pub struct Terminal {
    pub term: Arc<Mutex<Term<WakeListener>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub cols: usize,
    pub rows: usize,
    pub cwd: std::path::PathBuf,
    pub history: Arc<Mutex<Vec<u8>>>,
    pub last_click: Option<(std::time::Instant, i32, usize)>,
    pub click_count: u8,
    master: Box<dyn MasterPty + Send>,
    shell_pid: Option<u32>,
    /// Shared with WakeListener; flushed once per render frame via
    /// `flush_pty_replies`.
    pty_replies: Arc<Mutex<Vec<u8>>>,
    /// Set by `write_input` when the user types; drained by
    /// `flush_scroll_to_bottom` after ui.input releases to avoid a
    /// deadlock against egui's Context lock.
    pending_scroll_to_bottom: std::sync::atomic::AtomicBool,
}

impl Terminal {
    /// Drain any VT replies the parser has queued and forward them
    /// to the PTY. Called once per render.
    pub fn flush_pty_replies(&mut self) {
        let mut q = self.pty_replies.lock();
        if q.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut *q);
        drop(q);
        let mut w = self.writer.lock();
        let _ = w.write_all(&bytes);
        let _ = w.flush();
    }
}

impl Terminal {
    pub fn spawn(
        ctx: egui::Context,
        cols: usize,
        rows: usize,
        cwd: Option<&Path>,
    ) -> std::io::Result<Self> {
        Self::spawn_inner(ctx, cols, rows, cwd, None)
    }

    fn spawn_inner(
        ctx: egui::Context,
        cols: usize,
        rows: usize,
        cwd: Option<&Path>,
        transcript: Option<String>,
    ) -> std::io::Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        } else if let Ok(home) = std::env::var("HOME") {
            cmd.cwd(home);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let shell_pid = child.process_id();
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

        let pty_replies = Arc::new(Mutex::new(Vec::<u8>::with_capacity(64)));
        let listener = WakeListener {
            ctx: ctx.clone(),
            pty_replies: pty_replies.clone(),
        };
        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &TermSize {
                columns: cols,
                screen_lines: rows,
            },
            listener,
        )));

        let history = Arc::new(Mutex::new(Vec::<u8>::with_capacity(HISTORY_MAX / 2)));

        // If the caller provided transcript text (from a previous
        // session), write it into alacritty's scrollback BEFORE the
        // reader thread starts — followed by enough blank lines to
        // push the whole transcript up into the history buffer, and
        // finally an explicit cursor-home so the shell's subsequent
        // prompt-drawing starts from a known (0,0) state. This gives
        // us unified scroll: alacritty's own scrollbar spans both
        // transcript + live content. Cursor correctness is preserved
        // because the shell boots with a clean cursor position; any
        // PtyWrite replies accumulated during this pre-injection get
        // written immediately by the WakeListener and are harmless
        // (empty Term queries have no cursor queries yet).
        if let Some(text) = transcript.as_deref()
            && !text.is_empty()
        {
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut guard = term.lock();
            processor.advance(&mut *guard, text.as_bytes());
            if !text.ends_with('\n') {
                processor.advance(&mut *guard, b"\r\n");
            }
            // Pad with screen_lines blank rows so every transcript
            // line ends up in scrollback (none left in visible grid).
            let padding = "\r\n".repeat(rows);
            processor.advance(&mut *guard, padding.as_bytes());
            // Home cursor — shell boots with a known-good state.
            processor.advance(&mut *guard, b"\x1b[H");
            // Make sure the viewport is at the bottom so the shell's
            // first prompt is visible without the user needing to scroll.
            guard.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
        }

        let term_clone = term.clone();
        let history_clone = history.clone();
        let ctx_clone = ctx.clone();
        thread::spawn(move || {
            let mut reader = reader;
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut t = term_clone.lock();
                        processor.advance(&mut *t, &buf[..n]);
                        drop(t);
                        let mut h = history_clone.lock();
                        h.extend_from_slice(&buf[..n]);
                        if h.len() > HISTORY_MAX {
                            let drop_n = h.len() - HISTORY_MAX;
                            h.drain(0..drop_n);
                        }
                        drop(h);
                        // Immediate repaint: typing latency > throughput;
                        // egui coalesces multiple requests within a frame
                        // so this is cheap for bursty output too.
                        ctx_clone.request_repaint();
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            term,
            writer,
            cols,
            rows,
            cwd: cwd.map(|p| p.to_path_buf()).unwrap_or_default(),
            history,
            last_click: None,
            click_count: 0,
            master: pair.master,
            shell_pid,
            pty_replies,
            pending_scroll_to_bottom: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// True if the PTY's foreground process group is not the shell itself —
    /// i.e. something is actively running (vim, a build, a long-running test).
    /// Unix-only; other platforms always return false.
    #[cfg(unix)]
    pub fn has_foreground_process(&self) -> bool {
        let Some(shell) = self.shell_pid else {
            return false;
        };
        let Some(fd) = self.master.as_raw_fd() else {
            return false;
        };
        let fg = unsafe { libc::tcgetpgrp(fd) };
        if fg < 0 {
            return false;
        }
        (fg as u32) != shell
    }

    #[cfg(not(unix))]
    pub fn has_foreground_process(&self) -> bool {
        false
    }

    /// Restore a terminal with a plain-text scrollback snapshot. The
    /// snapshot (produced by `snapshot_text()` on save) is pure text +
    /// CRLF — no escapes — so replaying it into a fresh grid produces
    /// predictable line-by-line layout regardless of the new terminal
    /// width. Happens BEFORE the reader thread starts, so there's no
    /// race with the new shell's startup output.
    pub fn spawn_with_text_history(
        ctx: egui::Context,
        cols: usize,
        rows: usize,
        cwd: Option<&Path>,
        history_text: &str,
    ) -> std::io::Result<Self> {
        Self::spawn_inner(ctx, cols, rows, cwd, Some(history_text.to_string()))
    }

    /// Raw PTY byte log. Retained on `Terminal` for future features
    /// (e.g. exporting a session transcript); no longer written to
    /// session state since the grid-text snapshot is more useful there.
    #[allow(dead_code)]
    pub fn history_snapshot(&self) -> Vec<u8> {
        self.history.lock().clone()
    }

    /// Plain-text snapshot of the terminal's scrollback + visible grid,
    /// joined with CRLF. This is what the session should persist —
    /// replaying the raw PTY byte log doesn't work because shell prompts
    /// (especially ZSH RPROMPT) use absolute cursor-positioning escapes
    /// that were baked against the original terminal width and emit no
    /// LFs; replaying them into a fresh terminal stacks everything on
    /// one row.
    ///
    /// Trailing trailing empty rows are trimmed. Returns an empty
    /// string when there's nothing meaningful to capture.
    pub fn snapshot_text(&self) -> String {
        use alacritty_terminal::index::{Column, Line, Point};
        let guard = self.term.lock();
        let grid = guard.grid();
        let cols = grid.columns();
        let screen_lines = grid.screen_lines() as i32;
        let history = grid.history_size() as i32;
        let mut rows: Vec<String> = Vec::with_capacity((history + screen_lines) as usize);
        for line in -history..screen_lines {
            let mut row = String::with_capacity(cols);
            for c in 0..cols {
                let cell = &grid[Point::new(Line(line), Column(c))];
                let ch = cell.c;
                row.push(if ch == '\0' { ' ' } else { ch });
            }
            rows.push(row.trim_end().to_string());
        }
        while rows.last().is_some_and(|r| r.is_empty()) {
            rows.pop();
        }
        rows.join("\r\n")
    }

    pub fn write_input(&mut self, data: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(data);
        let _ = w.flush();
        // Set a flag instead of driving scroll_display here. Most
        // write_input callers execute inside a `ui.input(…)` closure
        // (which holds a read lock on egui's Context), and
        // `scroll_display` can wake the alacritty listener, which
        // calls `ctx.request_repaint()` → write lock → deadlock. The
        // render loop drains this flag after ui.input releases.
        self.pending_scroll_to_bottom
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Called by the render loop after `ui.input` closes, to snap the
    /// viewport back to the live screen if the user typed this frame.
    pub fn flush_scroll_to_bottom(&mut self) {
        if self
            .pending_scroll_to_bottom
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.term.lock().scroll_display(Scroll::Bottom);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.term.lock().resize(TermSize {
            columns: cols,
            screen_lines: rows,
        });
    }
}

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
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
    /// Bytes alacritty wants us to write back to the PTY (cursor
    /// position / device-status replies, title ack, etc.). Drained
    /// by the Terminal each frame and forwarded to the PTY writer.
    /// Without this, ZSH's RPROMPT uses `ESC[6n` to measure cursor
    /// position and, getting no reply, guesses a garbage offset —
    /// that was the "prompts stacked diagonally" bug.
    pty_replies: Arc<Mutex<Vec<u8>>>,
}

impl EventListener for WakeListener {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(s) = event {
            self.pty_replies.lock().extend_from_slice(s.as_bytes());
        }
        self.ctx.request_repaint();
    }
}

const HISTORY_MAX: usize = 256 * 1024;

pub struct Terminal {
    pub term: Arc<Mutex<Term<WakeListener>>>,
    writer: Box<dyn Write + Send>,
    pub cols: usize,
    pub rows: usize,
    pub cwd: std::path::PathBuf,
    pub history: Arc<Mutex<Vec<u8>>>,
    pub last_click: Option<(std::time::Instant, i32, usize)>,
    pub click_count: u8,
    master: Box<dyn MasterPty + Send>,
    shell_pid: Option<u32>,
    /// Bytes alacritty's VT parser has queued for the shell (replies
    /// to CSI 6n / DA / DSR queries). Drained + forwarded by
    /// `flush_pty_replies` each frame.
    pty_replies: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    /// Forward any queued VT replies to the shell. Must be called every
    /// frame (cheap when the queue is empty). Without this, ZSH's
    /// RPROMPT + any shell feature relying on cursor-position-query
    /// misbehaves.
    pub fn flush_pty_replies(&mut self) {
        let mut q = self.pty_replies.lock();
        if q.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut *q);
        drop(q);
        let _ = self.writer.write_all(&bytes);
        let _ = self.writer.flush();
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
        pre_reader_text: Option<&str>,
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

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let history = Arc::new(Mutex::new(Vec::<u8>::with_capacity(HISTORY_MAX / 2)));

        // Pre-reader history injection: write the saved text snapshot
        // into the grid BEFORE the reader thread starts, so there is
        // no race with the shell's startup output. The text is pure
        // printable chars + CRLF (no escapes), so it flows into the
        // grid predictably — filling the visible rows first, scrolling
        // earlier rows into alacritty's scrollback buffer. When the
        // reader thread starts and the shell prints its first prompt,
        // it lands below our replayed history. If the shell clears
        // the visible screen, our history still lives in scrollback
        // (Ctrl+Wheel / scrollbar to view).
        if let Some(text) = pre_reader_text
            && !text.is_empty()
        {
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut guard = term.lock();
            processor.advance(&mut *guard, text.as_bytes());
            // Append a final CRLF so the shell's own first prompt
            // starts on its own line rather than concatenating.
            processor.advance(&mut *guard, b"\r\n");
            drop(guard);
            // Any PtyWrite events that accumulated during replay are
            // replies to stale queries from the prior session — they
            // mustn't reach the new shell.
            pty_replies.lock().clear();
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
        Self::spawn_inner(ctx, cols, rows, cwd, Some(history_text))
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
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
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

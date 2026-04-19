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

    /// Restore a terminal from saved scrollback bytes. Spawns a live shell in
    /// the given cwd; then replays the saved bytes through the VT processor so
    /// the grid shows the prior visual state. New input/output starts fresh.
    #[allow(dead_code)] // staged for session scrollback replay
    pub fn spawn_with_history(
        ctx: egui::Context,
        cols: usize,
        rows: usize,
        cwd: Option<&Path>,
        saved_history: &[u8],
    ) -> std::io::Result<Self> {
        let term = Self::spawn(ctx, cols, rows, cwd)?;
        if !saved_history.is_empty() {
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut guard = term.term.lock();
            processor.advance(&mut *guard, saved_history);
            drop(guard);
            let mut h = term.history.lock();
            h.extend_from_slice(saved_history);
            if h.len() > HISTORY_MAX {
                let drop_n = h.len() - HISTORY_MAX;
                h.drain(0..drop_n);
            }
        }
        Ok(term)
    }

    pub fn history_snapshot(&self) -> Vec<u8> {
        self.history.lock().clone()
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

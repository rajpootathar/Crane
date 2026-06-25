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

use crane_term::{Processor, Term};

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
        let parser = Arc::new(Mutex::new(Processor::new()));
        let alive = Arc::new(AtomicBool::new(true));

        // Reader thread: PTY -> crane_term, write back replies, wake the UI.
        // Lock order is ALWAYS parser-then-term (deadlock-critical).
        let reader_handle = {
            let term = term.clone();
            let parser = parser.clone();
            let writer = writer.clone();
            let alive = alive.clone();
            Some(thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 8192];
                let mut last_epoch = 0u64;
                let mut last_cursor = (usize::MAX, usize::MAX);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let (replies, epoch, cursor);
                            {
                                let mut p = parser.lock();
                                let mut t = term.lock();
                                p.parse_bytes(&mut *t, &buf[..n]);
                                replies = t.take_pty_replies();
                                epoch = t.dirty_epoch;
                                cursor = (t.grid.cursor.row, t.grid.cursor.col);
                            }
                            // Write replies BEFORE the next read (P10k workaround).
                            if !replies.is_empty() {
                                let mut w = writer.lock();
                                let _ = w.write_all(&replies);
                                let _ = w.flush();
                            }
                            if epoch != last_epoch || cursor != last_cursor {
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
        })
    }

    /// Write input bytes to the PTY. `&self` (interior Arc<Mutex>) so the
    /// render closure can call it without `&mut`.
    pub fn write_input(&self, data: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(data);
        let _ = w.flush();
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

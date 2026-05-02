use crane_term::{Processor as CtProcessor, Term as CtTerm};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

const HISTORY_MAX: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Terminfo: extend xterm-256color with the Sync extended capability
// (Synchronized Output, DEC mode 2026). Ink-based TUIs (Claude Code,
// etc.) check terminfo for `Sync` and, when present, wrap their
// redraws in \e[?2026h .. \e[?2026l. Our Processor / Term then
// buffers the redraw and replays it with `set_sync_frame(true)` so
// the LFs at scroll-region bottom don't push intermediate redraw
// rows into scrollback.
// ---------------------------------------------------------------------------

#[cfg(unix)]
const CRANE_TERMINFO_SRC: &[u8] = b"xterm-crane|Crane terminal emulator,\n\
    \tuse=xterm-256color,\n\
    \tSync=\\E[?2026h\\E[?2026l,\n";

#[cfg(unix)]
static CRANE_TERMINFO_OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(unix)]
fn use_crane_terminfo() -> bool {
    *CRANE_TERMINFO_OK.get_or_init(|| {
        let home = match crate::util::home_dir() {
            Some(h) => h,
            None => return false,
        };
        let terminfo_dir = home.join(".terminfo");
        if !std::path::Path::new(&format!("{}/x", terminfo_dir.display())).exists() {
            let _ = std::fs::create_dir_all(&terminfo_dir);
        }
        let probe = terminfo_dir.join("78").join("xterm-crane");
        if probe.exists() {
            return true;
        }
        let tmp = std::env::temp_dir().join("crane-terminfo.src");
        if std::fs::write(&tmp, CRANE_TERMINFO_SRC).is_err() {
            return false;
        }
        let out = std::process::Command::new("tic")
            .args(["-x", "-o"])
            .arg(&terminfo_dir)
            .arg(&tmp)
            .output();
        let _ = std::fs::remove_file(&tmp);
        match out {
            Ok(o) if o.status.success() => true,
            _ => false,
        }
    })
}

#[cfg(not(unix))]
fn use_crane_terminfo() -> bool {
    false
}

pub struct Terminal {
    /// Crane's in-house terminal core. Holds the grid, scrollback,
    /// cursor, mode bag, and scroll region. The Processor below
    /// drives mutations through its Handler impl.
    pub term: Arc<Mutex<CtTerm>>,
    /// VT parser + `?2026` sync buffer. Owns the byte → Handler
    /// dispatch loop. Held next to the term so the reader thread
    /// and any pre-boot transcript replay both go through the same
    /// parser state.
    pub parser: Arc<Mutex<CtProcessor>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub cols: usize,
    pub rows: usize,
    pub cwd: std::path::PathBuf,
    pub history: Arc<Mutex<Vec<u8>>>,
    pub last_click: Option<(std::time::Instant, i32, usize)>,
    pub click_count: u8,
    master: Box<dyn MasterPty + Send>,
    shell_pid: Option<u32>,
    /// Shell child handle. Kept so `Drop` can `kill()` + `wait()` it
    /// when the Pane closes — relying on SIGHUP-on-master-close is
    /// unreliable for shells / subprocesses that ignore the signal.
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    /// Set by `write_input` when the user types; drained by
    /// `flush_scroll_to_bottom` after ui.input releases.
    pending_scroll_to_bottom: std::sync::atomic::AtomicBool,
    /// Sub-line wheel-delta carry. egui's `smooth_scroll_delta` is
    /// pixels; cell height ~16 px means rounding-per-frame drops
    /// most events. Carry the remainder across frames.
    pub scroll_carry: parking_lot::Mutex<f32>,
    /// False once the PTY reader has hit EOF / error. UI polls
    /// each frame and closes the owning Pane.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Terminal {
    /// Close-down order: kill child first so the PTY's slave side
    /// has no writers, then master drops (closes its fd) which
    /// EOFs the reader thread. Without this, shells that ignore
    /// SIGHUP can keep the master fd busy.
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        #[cfg(target_os = "macos")]
        macos_release_freed_pages();
    }
}

#[cfg(target_os = "macos")]
fn macos_release_freed_pages() {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(
            zone: *mut libc::c_void,
            goal: libc::size_t,
        ) -> libc::size_t;
    }
    unsafe {
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

impl Terminal {
    pub fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Terminal {
    /// Drain VT replies the parser queued (DSR / DA / title-ack) and
    /// forward them to the PTY. Now a thin wrapper since
    /// `crane_term::Term` exposes `take_pty_replies()` directly.
    pub fn flush_pty_replies(&mut self) {
        let bytes = self.term.lock().take_pty_replies();
        if bytes.is_empty() {
            return;
        }
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
        // Ink-based TUIs (Claude Code, etc.) check terminfo for the
        // `Sync` capability and wrap their redraws in
        // \e[?2026h .. \e[?2026l when present. Our Processor handles
        // those blocks correctly via the in-house sync-frame replay
        // path, so we want TUIs to use them.
        cmd.env(
            "TERM",
            if use_crane_terminfo() {
                "xterm-crane"
            } else {
                "xterm-256color"
            },
        );
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Crane");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        // Inherited from gnome-terminal / VTE-based parents. Leaks
        // confuse TUIs that pick feature flags meant for VTE's grid
        // semantics. Cleared, matching Ghostty / Wezterm / kitty.
        cmd.env_remove("VTE_VERSION");
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        } else if let Some(home) = crate::util::home_dir() {
            cmd.cwd(home);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let shell_pid = child.process_id();
        drop(pair.slave);
        let child_handle: Option<Box<dyn portable_pty::Child + Send + Sync>> = Some(child);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| std::io::Error::other(e.to_string()))?,
        ));

        let term = Arc::new(Mutex::new(CtTerm::new(rows, cols)));
        let parser = Arc::new(Mutex::new(CtProcessor::new()));

        let history = Arc::new(Mutex::new(Vec::<u8>::with_capacity(HISTORY_MAX / 2)));

        // Replay any prior session transcript BEFORE starting the
        // reader thread. The transcript is plain text + CRLF, so
        // feeding it through the parser produces predictable
        // line-by-line layout. Pad with `rows` blank rows so the
        // whole transcript ends up in scrollback (none in visible
        // grid), then home the cursor for the shell's first prompt.
        if let Some(text) = transcript.as_deref()
            && !text.is_empty()
        {
            let mut p = parser.lock();
            let mut t = term.lock();
            p.parse_bytes(&mut *t, text.as_bytes());
            if !text.ends_with('\n') {
                p.parse_bytes(&mut *t, b"\r\n");
            }
            let padding = "\r\n".repeat(rows);
            p.parse_bytes(&mut *t, padding.as_bytes());
            p.parse_bytes(&mut *t, b"\x1b[H");
            t.scroll_to_bottom();
        }

        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let term_clone = term.clone();
        let parser_clone = parser.clone();
        let writer_clone = writer.clone();
        let history_clone = history.clone();
        let ctx_clone = ctx.clone();
        let alive_clone = alive.clone();
        // Opt-in raw VT byte trace. CRANE_VT_TRACE=1 dumps every
        // byte the PTY produces, exactly as the parser sees it,
        // to ~/.crane/vt-trace-<pid>.log. Cheap branch when unset.
        let trace_file: Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>> = {
            if std::env::var("CRANE_VT_TRACE").ok().as_deref() == Some("1") {
                let dir = crate::util::home_dir()
                    .map(|h| h.join(".crane"))
                    .unwrap_or_default();
                let _ = std::fs::create_dir_all(&dir);
                let pid = std::process::id();
                let path = dir.join(format!("vt-trace-{pid}.log"));
                eprintln!("[crane] VT trace enabled → {}", path.display());
                std::fs::File::create(&path)
                    .ok()
                    .map(|f| std::sync::Arc::new(std::sync::Mutex::new(f)))
            } else {
                None
            }
        };
        let trace_file_clone = trace_file.clone();
        thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(f) = trace_file_clone.as_ref()
                            && let Ok(mut g) = f.lock()
                        {
                            let _ = g.write_all(&buf[..n]);
                        }
                        {
                            let mut p = parser_clone.lock();
                            let mut t = term_clone.lock();
                            p.parse_bytes(&mut *t, &buf[..n]);
                            // Drain DSR / DA / title-ack replies the
                            // parser produced into the writer BEFORE
                            // releasing the term lock — the
                            // Powerlevel10k width-miscount workaround
                            // depends on these arriving promptly.
                            let replies = t.take_pty_replies();
                            drop(t);
                            drop(p);
                            if !replies.is_empty() {
                                let mut w = writer_clone.lock();
                                let _ = w.write_all(&replies);
                                let _ = w.flush();
                            }
                        }
                        let mut h = history_clone.lock();
                        h.extend_from_slice(&buf[..n]);
                        if h.len() > HISTORY_MAX {
                            let drop_n = h.len() - HISTORY_MAX;
                            h.drain(0..drop_n);
                        }
                        drop(h);
                        // Wake egui per PTY batch unconditionally.
                        // The earlier dirty_epoch gate skipped
                        // repaints for cursor-only moves and on parse
                        // calls whose net effect was no cell change,
                        // which left the cursor visibly stuck. egui
                        // coalesces multiple repaint requests within
                        // a frame, so per-batch wakes are still cheap.
                        ctx_clone.request_repaint();
                    }
                    Err(_) => break,
                }
            }
            alive_clone.store(false, std::sync::atomic::Ordering::Relaxed);
            ctx_clone.request_repaint();
        });

        Ok(Self {
            term,
            parser,
            writer,
            cols,
            rows,
            cwd: cwd.map(|p| p.to_path_buf()).unwrap_or_default(),
            history,
            last_click: None,
            click_count: 0,
            master: pair.master,
            shell_pid,
            child: child_handle,
            pending_scroll_to_bottom: std::sync::atomic::AtomicBool::new(false),
            scroll_carry: parking_lot::Mutex::new(0.0),
            alive,
        })
    }

    /// True if the PTY's foreground process group is not the shell —
    /// i.e. something is actively running (vim, a build, etc.).
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

    /// Restore a terminal with a plain-text scrollback snapshot.
    pub fn spawn_with_text_history(
        ctx: egui::Context,
        cols: usize,
        rows: usize,
        cwd: Option<&Path>,
        history_text: &str,
    ) -> std::io::Result<Self> {
        Self::spawn_inner(ctx, cols, rows, cwd, Some(history_text.to_string()))
    }

    /// Raw PTY byte log. Retained for future export use.
    #[allow(dead_code)]
    pub fn history_snapshot(&self) -> Vec<u8> {
        self.history.lock().clone()
    }

    /// ANSI snapshot of the terminal's scrollback + visible grid.
    /// Preserves every cell's color and SGR flag (bold, italic,
    /// underline, inverse, dim, strikethrough, hidden, double-
    /// underline) so a restored session looks visually identical
    /// to what was saved. Used for session save in preference to
    /// `snapshot_text` whenever style preservation matters.
    pub fn snapshot_ansi(&self) -> String {
        self.term.lock().snapshot_ansi()
    }

    /// Plain-text snapshot of the terminal's scrollback + visible
    /// grid. Retained as a fallback / debugging tool — session save
    /// uses [`Terminal::snapshot_ansi`] so colors and decorations
    /// survive a restore.
    #[allow(dead_code)]
    pub fn snapshot_text(&self) -> String {
        self.term.lock().snapshot_text()
    }

    pub fn write_input(&mut self, data: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(data);
        let _ = w.flush();
        // Don't drive scroll_display from here — most callers run
        // inside `ui.input(…)` which holds an egui Context read
        // lock; scroll_display can wake the parser, which calls
        // `ctx.request_repaint()` → write lock → deadlock. Stash a
        // flag instead; the render loop flushes it after ui.input
        // releases.
        self.pending_scroll_to_bottom
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snap viewport to the live screen if the user typed this frame.
    pub fn flush_scroll_to_bottom(&mut self) {
        if self
            .pending_scroll_to_bottom
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.term.lock().scroll_to_bottom();
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
        self.term.lock().resize(rows, cols);
    }
}

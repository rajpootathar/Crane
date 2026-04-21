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

// ---------------------------------------------------------------------------
// Terminfo: extend xterm-256color with the Sync extended capability
// (Synchronized Output, DEC mode 2026). Ink-based TUIs (Claude Code,
// etc.) check terminfo for `Sync` and, when present, wrap their
// redraws in \e[?2026h .. \e[?2026l. Our SyncAwareHandler then
// converts the LFs inside the redraw region to non-scrolling
// move_down(1), preventing ghost-frame accumulation in scrollback.
// Compiled on first launch via `tic -x`; installed to ~/.terminfo/.
// Falls back to plain xterm-256color if tic is unavailable.
// ---------------------------------------------------------------------------

/// Terminfo source: xterm-256color + Sync capability.
const CRANE_TERMINFO_SRC: &[u8] = b"xterm-crane|Crane terminal emulator,\n\
    \tuse=xterm-256color,\n\
    \tSync=\\E[?2026h\\E[?2026l,\n";

/// One-time gate: true once xterm-crane has been installed (or was
/// already present). Cached for the process lifetime so `tic` only
/// runs once, even across many terminal panes.
static CRANE_TERMINFO_OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Ensure `~/.terminfo/<hash>/xterm-crane` exists. Compiles via
/// `tic -x` on first call. Returns true when the custom entry is
/// usable (either just compiled or already present).
fn use_crane_terminfo() -> bool {
    *CRANE_TERMINFO_OK.get_or_init(|| {
        let home = match std::env::var("HOME") {
            Ok(h) => h,
            Err(_) => return false,
        };
        let dir = format!("{home}/.terminfo");
        // ncurses uses either a hex subdirectory (macOS: 78/) or a
        // character subdirectory (Linux: x/). Check both.
        let hex = format!("{dir}/78/xterm-crane");
        let chr = format!("{dir}/x/xterm-crane");
        if std::path::Path::new(&hex).exists() || std::path::Path::new(&chr).exists() {
            return true;
        }
        let _ = std::fs::create_dir_all(&dir);
        let tmp = std::env::temp_dir().join(format!(
            "xterm-crane-{}.ti",
            std::process::id()
        ));
        if std::fs::write(&tmp, CRANE_TERMINFO_SRC).is_err() {
            return false;
        }
        let out = std::process::Command::new("tic")
            .args(["-x", "-o", &dir])
            .arg(&tmp)
            .output();
        let _ = std::fs::remove_file(&tmp);
        match out {
            Ok(o) if o.status.success() => true,
            Ok(o) => {
                eprintln!(
                    "[crane] tic failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                false
            }
            Err(e) => {
                eprintln!("[crane] tic unavailable: {e}");
                false
            }
        }
    })
}

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
    /// Shell child handle. Kept so `Drop` can `kill()` + `wait()` it
    /// when the Pane closes, instead of relying on SIGHUP-on-master-
    /// close (which some shells / subprocesses ignore, leaving the
    /// reader thread pinned and the alacritty grid resident in RAM
    /// long after the user closed the terminal).
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    /// Shared with WakeListener; flushed once per render frame via
    /// `flush_pty_replies`.
    pty_replies: Arc<Mutex<Vec<u8>>>,
    /// Set by `write_input` when the user types; drained by
    /// `flush_scroll_to_bottom` after ui.input releases to avoid a
    /// deadlock against egui's Context lock.
    pending_scroll_to_bottom: std::sync::atomic::AtomicBool,
    /// Sub-line wheel-delta carry. egui's `smooth_scroll_delta` arrives
    /// in pixels; a trackpad flick commonly emits ~3–6 px per frame
    /// and cell height is ~16 px, so rounding-per-frame silently
    /// drops most events and scrolling feels laggy. We accumulate the
    /// remainder here and extract whole cells once it crosses ±1.
    pub scroll_carry: parking_lot::Mutex<f32>,
    /// False once the PTY reader has hit EOF / error — i.e. the shell
    /// process exited (user typed `exit`, Ctrl-D, was killed, etc.).
    /// UI polls this each frame and closes the owning Pane.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Terminal {
    /// Close-down order matters: kill the child first so the PTY's
    /// slave side has no writers, then the master drops (closes its
    /// fd), which EOFs the reader thread. Without this, shells that
    /// ignore SIGHUP (or nested subprocesses that inherited the pty)
    /// can keep the master fd busy, stranding the reader thread —
    /// which holds Arc<Mutex<Term>> + Arc<Mutex<history>> and pins
    /// the whole grid (tens of MB per terminal) in RAM long after
    /// the Pane was closed.
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            // Reap the zombie — otherwise the child sits as defunct
            // until the Crane process itself exits.
            let _ = c.wait();
        }
        // `master` drops after this fn returns, which closes the pty
        // fd and EOFs the detached reader thread. We don't join it;
        // once the Arc drops the thread's closure finishes within a
        // few ms and exits on its own.
        #[cfg(target_os = "macos")]
        macos_release_freed_pages();
    }
}

/// Ask macOS's malloc to return freed pages to the kernel. Without
/// this hint, RSS doesn't drop when a terminal closes — the grid is
/// freed to the process's malloc arena but the arena holds the pages
/// as "dirty-but-free" until memory pressure forces a release, so
/// Activity Monitor keeps showing the same usage.
///
/// `malloc_zone_pressure_relief(NULL, 0)` walks every registered zone
/// and madvise(MADV_FREE)s unused pages. Microseconds-cheap.
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
        // TUIs like Claude Code / Ink inspect these env vars to pick
        // their redraw strategy. We install a custom terminfo entry
        // (xterm-crane) that extends xterm-256color with the `Sync`
        // extended capability, advertising Synchronized Output (DEC
        // 2026). Ink's `log-update` package checks terminfo for `Sync`;
        // when found it wraps each redraw in \e[?2026h .. \e[?2026l,
        // which our SyncAwareHandler converts to non-scrolling LFs —
        // no ghost frames in scrollback. Falls back to xterm-256color
        // if tic is unavailable (e.g. stripped minimal Linux).
        cmd.env("TERM", if use_crane_terminfo() { "xterm-crane" } else { "xterm-256color" });
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Crane");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        // Inherited from gnome-terminal / other VTE-based parents. If
        // it leaks through, TUIs like neovim / Claude Code misdetect
        // the renderer and pick feature flags meant for VTE's grid
        // semantics (which differ subtly around scroll regions and
        // cursor-save). Clearing it matches what Ghostty / WezTerm /
        // kitty do — keeps detection unambiguous.
        cmd.env_remove("VTE_VERSION");
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

        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let term_clone = term.clone();
        let history_clone = history.clone();
        let ctx_clone = ctx.clone();
        let alive_clone = alive.clone();
        // Opt-in raw VT byte trace. Set CRANE_VT_TRACE=1 in the parent
        // env to dump everything the PTY produces, exactly as the
        // parser sees it, to ~/.crane/vt-trace-<pid>.log. Lets us diff
        // our byte stream against what iTerm / Alacritty see for the
        // same TUI and pin down which escape sequence our parser is
        // mis-handling. Cheap branch when the flag is unset.
        let trace_file: Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>> = {
            if std::env::var("CRANE_VT_TRACE").ok().as_deref() == Some("1") {
                let home = std::env::var("HOME").unwrap_or_default();
                let dir = std::path::PathBuf::from(format!("{home}/.crane"));
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
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(f) = trace_file_clone.as_ref()
                            && let Ok(mut g) = f.lock()
                        {
                            use std::io::Write;
                            let _ = g.write_all(&buf[..n]);
                        }
                        // Feed bytes straight to the parser; alacritty's
                        // built-in StdSyncHandler handles `?2026` sync
                        // stashing internally. We tried a shadow-grid
                        // snapshot/restore wrapper here to eliminate
                        // ghost frames from Ink redraws; it rendered
                        // correctly on small sync blocks but overlaid
                        // shifted content onto the restored grid when
                        // sync blocks scrolled. Leaving it as a v0.5
                        // task; the ancillary code remains in
                        // `sync_handler.rs` for reference.
                        {
                            let mut t = term_clone.lock();
                            processor.advance(&mut *t, &buf[..n]);
                        }
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
            alive_clone.store(false, std::sync::atomic::Ordering::Relaxed);
            ctx_clone.request_repaint();
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
            child: child_handle,
            pty_replies,
            pending_scroll_to_bottom: std::sync::atomic::AtomicBool::new(false),
            scroll_carry: parking_lot::Mutex::new(0.0),
            alive,
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


use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::git_log::data::{self, CommitRecord, Sha};
use crate::git_log::graph::{self, LaneFrame};
use crate::git_log::refresh;
use crate::git_log::refs::{self, RefSet};

/// Filter state applied at render time over the cached GraphFrame —
/// none of these touch the underlying git query, so toggling them is
/// always cheap.
#[derive(Default, Clone)]
pub struct FilterState {
    pub text: String,
    pub branch: Option<String>,
    pub user: Option<String>,
}

/// Operation requested from the commit-row right-click menu. Bubbled
/// up via `ViewEffect` and applied by main's render path against the
/// active workspace's repo.
#[derive(Clone, Debug)]
pub enum GitLogOp {
    Checkout(Sha),
    BranchFrom(Sha),
    WorktreeFrom(Sha),
    CherryPick(Sha),
    Revert(Sha),
    CopyHash(Sha),
}

/// Snapshot produced by the worker thread on each refresh — commits
/// + refs + lane geometry, all in one consistent generation. UI
/// thread renders the cached frame; worker swaps in a new one when a
/// fresh load completes.
pub struct GraphFrame {
    pub commits: Vec<CommitRecord>,
    pub refs: RefSet,
    pub lanes: LaneFrame,
    pub generation: u64,
}

pub struct GitLogState {
    pub height: f32,
    pub col_refs_width: f32,
    pub col_details_width: f32,
    pub maximized: bool,
    pub selected_commit: Option<Sha>,
    pub selected_file: Option<PathBuf>,
    pub last_poll: Instant,

    pub frame: Option<GraphFrame>,
    pub generation: u64,
    pub worker_rx: Option<mpsc::Receiver<GraphFrame>>,
    pub filter: FilterState,
    /// Filesystem watcher on `.git/HEAD` + `.git/refs/` + packed-refs.
    /// Lazy-init on first `maybe_reload` call so we don't pay the cost
    /// for tabs that never open the pane.
    pub watcher: Option<refresh::Watcher>,
    /// True while a `git fetch --all` is running on a background
    /// thread. The header strip swaps the Fetch button for a spinner.
    pub fetch_in_flight: Arc<AtomicBool>,
    /// Path of the repo this state was last reloaded against. Used to
    /// detect Workspace switches so the watcher gets re-created on a
    /// fresh repo.
    pub watched_repo: Option<PathBuf>,
    /// One-shot op picked from the right-click context menu — drained
    /// by the caller after `view::render` returns and applied against
    /// the active workspace's repo.
    pub pending_op: Option<GitLogOp>,
    /// User-typed branch name when GitLogOp::BranchFrom is in flight.
    /// `Some((sha, name))` while the inline prompt is open.
    pub pending_branch_prompt: Option<(Sha, String)>,
    /// Refs (left) column collapsed flag. The chevron button in its
    /// header bar toggles this. When collapsed the column shrinks to
    /// a thin strip showing only the expand handle.
    pub col_refs_collapsed: bool,
    /// Details (right) column collapsed flag.
    pub col_details_collapsed: bool,
    /// Log row author/date metadata column width inside the middle
    /// column — drag handle on the right edge of the metadata band
    /// resizes this. Subject takes the remaining space.
    pub col_log_meta_width: f32,
    /// Number of commits visible after filters were applied last
    /// frame. The header strip reads this to render "N of M commits"
    /// when any filter is active.
    pub last_visible_count: usize,
    /// Set when the user picks a branch in the refs panel — the log
    /// painter scrolls the corresponding tip into view next frame.
    pub pending_scroll_to_selected: bool,
    /// Set by the Cmd+F shortcut handler to request keyboard focus
    /// on the filter TextEdit on the next frame.
    pub pending_focus_filter: bool,
    /// True when the user has interacted with the Git Log pane
    /// (clicked inside its body region) more recently than any other
    /// pane. Used to route Cmd+F: when this pane has focus, the
    /// shortcut focuses the filter; otherwise it falls through to
    /// the Files Pane's find-in-file handler.
    pub has_focus: bool,
}

impl GitLogState {
    pub fn new() -> Self {
        Self {
            height: 320.0,
            col_refs_width: 220.0,
            col_details_width: 360.0,
            maximized: false,
            selected_commit: None,
            selected_file: None,
            last_poll: Instant::now(),
            frame: None,
            generation: 0,
            worker_rx: None,
            filter: FilterState::default(),
            watcher: None,
            fetch_in_flight: Arc::new(AtomicBool::new(false)),
            watched_repo: None,
            pending_op: None,
            pending_branch_prompt: None,
            col_refs_collapsed: false,
            col_details_collapsed: false,
            col_log_meta_width: 220.0,
            last_visible_count: 0,
            pending_scroll_to_selected: false,
            pending_focus_filter: false,
            has_focus: false,
        }
    }

    /// Auto-reload trigger called once per render frame. Reloads when
    /// any of the following fire:
    /// - filesystem watcher reports a `.git/HEAD` / `refs/` /
    ///   `packed-refs` write (debounced 250 ms)
    /// - 5 s poll fallback (covers FS event drops on NFS, etc.)
    ///
    /// Reload on first call too — that's how the pane backfills its
    /// initial frame after toggle. If the worker is already in flight
    /// this is a no-op.
    pub fn maybe_reload(&mut self, repo: PathBuf, ctx: &egui::Context) {
        // Re-create the watcher if the workspace switched.
        if self.watched_repo.as_deref() != Some(repo.as_path()) {
            self.watcher = refresh::Watcher::new(&repo);
            self.watched_repo = Some(repo.clone());
        }
        let mut should_reload = false;
        if self.frame.is_none() && self.worker_rx.is_none() {
            should_reload = true;
        }
        if let Some(w) = self.watcher.as_mut() {
            if w.poll(Duration::from_millis(250)) {
                should_reload = true;
            }
        }
        if self.last_poll.elapsed() >= Duration::from_secs(5) {
            self.last_poll = Instant::now();
            should_reload = true;
        }
        if should_reload {
            self.reload(repo, ctx);
        }
    }

    pub fn fetch_all(&self, repo: PathBuf, ctx: &egui::Context) {
        refresh::fetch_all_async(repo, self.fetch_in_flight.clone(), ctx.clone());
    }

    pub fn is_fetching(&self) -> bool {
        self.fetch_in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Kick off a fresh worker if none is in-flight. The worker
    /// shells out `git log` + `git for-each-ref` + `git worktree list`,
    /// computes the lane assignment, and sends the resulting
    /// GraphFrame back via mpsc. UI thread polls via `poll_worker`.
    pub fn reload(&mut self, repo: PathBuf, ctx: &egui::Context) {
        if self.worker_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let next_gen = self.generation + 1;
        std::thread::spawn(move || {
            let commits = data::load_commits(&repo, 10_000);
            let refs = refs::load_refs(&repo);
            let lanes = graph::layout(&commits);
            let frame = GraphFrame {
                commits,
                refs,
                lanes,
                generation: next_gen,
            };
            let _ = tx.send(frame);
            ctx.request_repaint();
        });
        self.worker_rx = Some(rx);
    }

    /// Poll the worker for completion. Call once per render frame.
    pub fn poll_worker(&mut self) {
        let Some(rx) = self.worker_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(frame) => {
                self.generation = frame.generation;
                self.frame = Some(frame);
                self.worker_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker_rx = None;
            }
        }
    }

    pub fn is_loading(&self) -> bool {
        self.worker_rx.is_some()
    }
}

impl Default for GitLogState {
    fn default() -> Self {
        Self::new()
    }
}

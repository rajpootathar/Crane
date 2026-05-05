use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use crate::git_log::data::{self, CommitRecord, Sha};
use crate::git_log::graph::{self, LaneFrame};
use crate::git_log::refs::{self, RefSet};

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
        }
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

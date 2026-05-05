//! Filesystem watcher + manual `git fetch` runner. The Git Log Pane
//! auto-reloads when refs change (commits, fetches, branch switches),
//! and a Fetch all button shells out `git fetch --all --prune --tags`
//! on a worker thread so the UI stays responsive.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// Wrapper around `notify::RecommendedWatcher`. Watches the three
/// paths inside `.git` that surface ref changes from any source —
/// CLI commits, `git fetch`, branch switches, etc. Coalesces bursts
/// of events with a debounce window so we don't reload on every
/// individual write to `.git/refs/`.
pub struct Watcher {
    _inner: RecommendedWatcher,
    rx: mpsc::Receiver<()>,
    last_event: Instant,
}

impl Watcher {
    pub fn new(repo: &Path) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |_res| {
            let _ = tx.send(());
        })
        .ok()?;
        let git_dir = repo.join(".git");
        // Best-effort. Missing files are silently skipped; the poll
        // fallback in GitLogState catches anything the watcher misses.
        let _ = watcher.watch(&git_dir.join("HEAD"), RecursiveMode::NonRecursive);
        let _ = watcher.watch(&git_dir.join("refs"), RecursiveMode::Recursive);
        let _ = watcher.watch(&git_dir.join("packed-refs"), RecursiveMode::NonRecursive);
        Some(Self {
            _inner: watcher,
            rx,
            last_event: Instant::now() - Duration::from_secs(60),
        })
    }

    /// Drain the channel and return true if at least one event arrived
    /// AND `min_gap` has elapsed since the last fired event (debounce).
    pub fn poll(&mut self, min_gap: Duration) -> bool {
        let mut got = false;
        while self.rx.try_recv().is_ok() {
            got = true;
        }
        if got && self.last_event.elapsed() >= min_gap {
            self.last_event = Instant::now();
            true
        } else {
            false
        }
    }
}

/// Run `git fetch --all --prune --tags` against `repo` on a worker
/// thread. `flag` is a shared in-flight bit the UI uses to render a
/// spinner; cleared when the child exits regardless of success. The
/// caller's `notify::Watcher` will pick up the resulting `.git/refs/`
/// writes and trigger a fresh reload.
pub fn fetch_all_async(repo: PathBuf, flag: Arc<AtomicBool>, ctx: egui::Context) {
    flag.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let _ = std::process::Command::new("git")
            .args(["fetch", "--all", "--prune", "--tags"])
            .current_dir(&repo)
            .status();
        flag.store(false, Ordering::Relaxed);
        ctx.request_repaint();
    });
}

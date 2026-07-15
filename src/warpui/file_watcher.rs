//! Cross-platform filesystem watcher. One `notify::RecommendedWatcher`
//! services every watched Project root; raw events feed a single
//! debouncer thread that coalesces bursts (~50 ms quiet period),
//! filters out git-internal churn and editor backup noise, and emits
//! one [`ChangeEvent`] per affected root.
//!
//! Two threads total regardless of how many roots are watched:
//! `notify`'s internal backend thread + our debouncer. The shell holds
//! the [`Receiver`] returned from [`FileWatcher::new`] and drains it
//! every frame to mark the owning repo dirty-now.
//!
//! Framework-agnostic: no warpui / egui types leak across this
//! boundary. Roots are identified purely by their canonical on-disk
//! path, so the shell can map an event's `root` straight back to the
//! Project / Workspace it registered.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;

/// Coalesced batch of changes for a single watched root. The debouncer
/// emits one of these after a ~50 ms quiet period — typical "save in
/// editor" sequences (write + attr-change + temp rename) coalesce into
/// one event. The shell drains these each frame and marks the repo at
/// `root` dirty-now.
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    /// Canonical root the changed paths belong to — the same path the
    /// caller passed to [`FileWatcher::watch_project`] /
    /// [`FileWatcher::watch_path`]. Route back to the owning repo with
    /// a straight equality check.
    pub root: PathBuf,
    /// Coalesced changed paths under `root` (deduped, absolute).
    pub paths: Vec<PathBuf>,
    pub created: bool,
    pub modified: bool,
    pub removed: bool,
    /// When this batch was flushed. Useful for staleness checks.
    pub arrived_at: Instant,
}

const DEBOUNCE: Duration = Duration::from_millis(50);

/// Owns the OS watcher + debouncer thread. The shell holds one of
/// these for the session lifetime and drains the paired [`Receiver`].
pub struct FileWatcher {
    inner: Arc<Mutex<Inner>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    debouncer: Mutex<Option<JoinHandle<()>>>,
    raw_tx: Sender<RawEvent>,
}

/// Shared state between the public API and the debouncer thread.
struct Inner {
    /// Canonical watched roots. Routing walks this by longest matching
    /// prefix. Length is bounded by user-configured Projects /
    /// Workspaces (single-digit to low-double-digit in practice), so a
    /// linear scan beats a trie's allocation overhead.
    roots: Vec<PathBuf>,
}

enum RawEvent {
    /// A raw filesystem event from the notify backend.
    Fs(Event),
    /// Shutdown signal.
    Stop,
}

impl FileWatcher {
    /// Construct + start. Returns the watcher and the receiver the
    /// shell drains each frame. The OS backend and the debouncer
    /// thread both start immediately but stay idle until the first
    /// `watch_*` call registers a root.
    ///
    /// If the OS backend fails to initialize (rare — permissions /
    /// resource exhaustion), the watcher is still returned in a
    /// no-op state and the failure is logged; the receiver simply
    /// never yields. This keeps the constructor infallible so the
    /// shell's startup path stays branch-free.
    pub fn new() -> (Self, Receiver<ChangeEvent>) {
        let (raw_tx, raw_rx) = mpsc::channel::<RawEvent>();
        let (out_tx, out_rx) = mpsc::channel::<ChangeEvent>();
        let inner = Arc::new(Mutex::new(Inner { roots: Vec::new() }));

        let watcher_tx = raw_tx.clone();
        let watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                let _ = watcher_tx.send(RawEvent::Fs(ev));
            }
        }) {
            Ok(w) => Some(w),
            Err(e) => {
                log::error!("FileWatcher: failed to init OS backend: {e}; filesystem change detection disabled");
                None
            }
        };

        let debouncer_inner = Arc::clone(&inner);
        let debouncer = thread::Builder::new()
            .name("crane-fs-debounce".into())
            .spawn(move || run_debouncer(raw_rx, out_tx, debouncer_inner))
            .map_err(|e| log::error!("FileWatcher: failed to spawn debouncer thread: {e}"))
            .ok();

        (
            Self {
                inner,
                watcher: Mutex::new(watcher),
                debouncer: Mutex::new(debouncer),
                raw_tx,
            },
            out_rx,
        )
    }

    /// Start watching a Project's root recursively. Idempotent — a
    /// root already watched is a no-op. Canonicalizes the path so
    /// prefix routing matches the realpath-form notify reports (macOS
    /// FSEvents reports `/private/var/...` for `/var/...` symlinks;
    /// same issue on Linux with `/tmp`).
    pub fn watch_project(&self, root: &Path) -> std::io::Result<()> {
        self.add_root(root)
    }

    /// Watch an additional path recursively (e.g. a Workspace worktree
    /// living outside the Project root under `~/.crane-worktrees`).
    /// Registered as its own routing root, so changes under it are
    /// attributed to `path` rather than any enclosing Project.
    /// Idempotent.
    pub fn watch_path(&self, path: &Path) -> std::io::Result<()> {
        self.add_root(path)
    }

    /// Register `path` as a watched root and start a recursive OS
    /// watch on it. Shared by `watch_project` / `watch_path`.
    fn add_root(&self, path: &Path) -> std::io::Result<()> {
        let canonical = match std::fs::canonicalize(path) {
            Ok(c) => c,
            Err(e) => {
                // Falling back to the non-canonical path means macOS
                // FSEvents may report `/private/var/...` while we route
                // on `/var/...`, silently dropping every event. Loud
                // log so the silent-failure mode is visible.
                log::warn!(
                    "FileWatcher: canonicalize({}) failed: {e}; events for it may be misrouted",
                    path.display()
                );
                path.to_path_buf()
            }
        };

        {
            let mut inner = self.inner.lock();
            if inner.roots.iter().any(|r| r == &canonical) {
                // Already watched — true no-op.
                return Ok(());
            }
            inner.roots.push(canonical.clone());
        }

        if let Some(w) = self.watcher.lock().as_mut() {
            w.watch(&canonical, RecursiveMode::Recursive)
                .map_err(notify_to_io)?;
        }
        Ok(())
    }

    /// Stop watching a previously registered root. Idempotent — an
    /// unknown path is ignored. Accepts either the canonical or the
    /// caller-supplied form.
    pub fn unwatch(&self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let removed = {
            let mut inner = self.inner.lock();
            let before = inner.roots.len();
            inner.roots.retain(|r| r != &canonical);
            before != inner.roots.len()
        };
        if removed && let Some(w) = self.watcher.lock().as_mut() {
            let _ = w.unwatch(&canonical);
        }
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        // Tear the watcher down first so no new raw events flow in.
        drop(self.watcher.lock().take());
        let _ = self.raw_tx.send(RawEvent::Stop);
        if let Some(h) = self.debouncer.lock().take() {
            let _ = h.join();
        }
    }
}

/// True if the path is git-internal churn or editor noise we never
/// want to wake the shell for.
fn is_filtered(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.contains("/.git/objects/")
        || s.contains("/.git/logs/")
        || s.contains("/.git/index.lock")
        || s.contains("/.git/HEAD.lock")
    {
        return true;
    }
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    // Editor temp files + OS metadata. Cheap suffix checks; no regex,
    // no allocations beyond the string view above.
    matches!(name, ".DS_Store" | "Thumbs.db")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with('~')
        || name.starts_with("~$")
        || name.ends_with(".tmp")
}

/// Walk the longest matching root prefix. `roots` is small (≤ low
/// double digits in practice), so a linear scan is cheaper than a
/// trie with allocation overhead.
fn route_to_root(roots: &[PathBuf], path: &Path) -> Option<PathBuf> {
    let mut best: Option<&PathBuf> = None;
    for root in roots {
        if path.starts_with(root) {
            let longer = best.map(|b| root.as_os_str().len() > b.as_os_str().len());
            if longer.unwrap_or(true) {
                best = Some(root);
            }
        }
    }
    best.cloned()
}

#[derive(Default)]
struct RootBucket {
    paths: Vec<PathBuf>,
    created: bool,
    modified: bool,
    removed: bool,
    last_seen: Option<Instant>,
}

fn run_debouncer(
    raw_rx: Receiver<RawEvent>,
    out_tx: Sender<ChangeEvent>,
    inner: Arc<Mutex<Inner>>,
) {
    let mut buckets: HashMap<PathBuf, RootBucket> = HashMap::new();

    loop {
        // Wake whenever the soonest bucket's deadline elapses, or
        // sooner if a new event arrives.
        let now = Instant::now();
        let next_deadline = buckets
            .values()
            .filter_map(|b| b.last_seen.map(|t| t + DEBOUNCE))
            .min();

        let timeout = match next_deadline {
            Some(d) if d > now => d - now,
            Some(_) => Duration::ZERO,
            None => Duration::from_secs(60),
        };

        match raw_rx.recv_timeout(timeout) {
            Ok(RawEvent::Stop) => return,
            Ok(RawEvent::Fs(ev)) => {
                let kind = ev.kind;
                let roots = inner.lock();
                for path in ev.paths.iter() {
                    if is_filtered(path) {
                        continue;
                    }
                    let Some(root) = route_to_root(&roots.roots, path) else {
                        continue;
                    };
                    let bucket = buckets.entry(root).or_default();
                    bucket.paths.push(path.clone());
                    bucket.last_seen = Some(Instant::now());
                    match kind {
                        EventKind::Create(_) => bucket.created = true,
                        EventKind::Modify(_) => bucket.modified = true,
                        EventKind::Remove(_) => bucket.removed = true,
                        _ => bucket.modified = true,
                    }
                }
                drop(roots);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // Flush any bucket whose quiet period has elapsed. A bucket
        // that just got a new event resets its deadline.
        let now = Instant::now();
        let mut to_flush: Vec<PathBuf> = Vec::new();
        for (root, bucket) in buckets.iter() {
            if let Some(t) = bucket.last_seen
                && now.duration_since(t) >= DEBOUNCE
            {
                to_flush.push(root.clone());
            }
        }
        for root in to_flush {
            if let Some(mut bucket) = buckets.remove(&root)
                && !bucket.paths.is_empty()
            {
                bucket.paths.sort();
                bucket.paths.dedup();
                let _ = out_tx.send(ChangeEvent {
                    root,
                    paths: bucket.paths,
                    created: bucket.created,
                    modified: bucket.modified,
                    removed: bucket.removed,
                    arrived_at: now,
                });
            }
        }
    }
}

fn notify_to_io(e: notify::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn drain(rx: &Receiver<ChangeEvent>, timeout: Duration) -> Option<ChangeEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(ev) = rx.try_recv() {
                return Some(ev);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn filter_drops_git_objects_and_editor_noise() {
        assert!(is_filtered(Path::new("/repo/.git/objects/ab/cdef")));
        assert!(is_filtered(Path::new("/repo/.git/index.lock")));
        assert!(is_filtered(Path::new("/repo/src/.foo.swp")));
        assert!(is_filtered(Path::new("/repo/.DS_Store")));
        assert!(is_filtered(Path::new("/repo/~$Document.docx")));
        assert!(is_filtered(Path::new("/repo/build.tmp")));

        assert!(!is_filtered(Path::new("/repo/src/main.rs")));
        assert!(!is_filtered(Path::new("/repo/.git/HEAD")));
        assert!(!is_filtered(Path::new("/repo/.git/refs/heads/main")));
    }

    #[test]
    fn route_picks_longest_prefix() {
        let roots = vec![PathBuf::from("/parent"), PathBuf::from("/parent/child")];
        assert_eq!(
            route_to_root(&roots, Path::new("/parent/child/foo.rs")),
            Some(PathBuf::from("/parent/child"))
        );
        assert_eq!(
            route_to_root(&roots, Path::new("/parent/sibling/bar.rs")),
            Some(PathBuf::from("/parent"))
        );
        assert_eq!(route_to_root(&roots, Path::new("/elsewhere/baz")), None);
    }

    #[test]
    fn end_to_end_create_modify_emits_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (watcher, rx) = FileWatcher::new();
        watcher.watch_project(dir.path()).expect("watch");
        let root = std::fs::canonicalize(dir.path()).expect("canon");

        // Give the OS backend a moment to register.
        thread::sleep(Duration::from_millis(50));

        let target = dir.path().join("hello.txt");
        fs::write(&target, "hi").expect("write");
        // Touch again to make sure modify also fires.
        fs::write(&target, "hello").expect("write2");

        let ev = drain(&rx, Duration::from_secs(2)).expect("change event");
        assert_eq!(ev.root, root);
        assert!(
            ev.paths.iter().any(|p| p.ends_with("hello.txt")),
            "paths missing target: {:?}",
            ev.paths
        );
        assert!(ev.created || ev.modified);
    }

    /// Records where filesystem events land for a *linked* git worktree.
    /// This is the crux of the sidebar-badge live-refresh bug: a linked
    /// worktree's `.git` is a FILE pointing at `<main>/.git/worktrees/<name>`,
    /// so a `git commit` inside the worktree writes refs/index/logs under the
    /// MAIN repo's `.git` — the worktree *checkout* dir sees ZERO events. A
    /// plain working-tree edit, by contrast, does land under the checkout.
    #[test]
    fn linked_worktree_commit_lands_under_main_not_checkout() {
        fn git(dir: &Path, args: &[&str]) {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {:?} failed in {}", args, dir.display());
        }

        let base = tempfile::tempdir().expect("tempdir");
        let main = base.path().join("main");
        let wt = base.path().join("wt");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["config", "user.email", "t@t.co"]);
        git(&main, &["config", "user.name", "t"]);
        fs::write(main.join("f.txt"), "a\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "init"]);
        git(
            &main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feature"],
        );

        let (watcher, rx) = FileWatcher::new();
        let main_root = std::fs::canonicalize(&main).unwrap();
        let wt_root = std::fs::canonicalize(&wt).unwrap();
        watcher.watch_project(&main).expect("watch main");
        watcher.watch_path(&wt).expect("watch wt");
        thread::sleep(Duration::from_millis(80));

        // (1) A working-tree edit in the linked worktree DOES reach the
        //     checkout root — the badge-refresh path can see this.
        fs::write(wt.join("f.txt"), "a\nb\n").unwrap();
        let mut saw_edit_on_wt = false;
        while let Some(ev) = drain(&rx, Duration::from_millis(400)) {
            if ev.root == wt_root {
                saw_edit_on_wt = true;
            }
        }
        assert!(
            saw_edit_on_wt,
            "working-tree edit should emit an event on the worktree checkout root"
        );

        // (2) A COMMIT in the linked worktree emits NOTHING on the checkout
        //     root — every ref/index/log write lands under the MAIN repo.
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-qm", "second"]);
        let mut commit_hit_wt = false;
        let mut commit_hit_main = false;
        while let Some(ev) = drain(&rx, Duration::from_millis(500)) {
            if ev.root == wt_root {
                commit_hit_wt = true;
            }
            if ev.root == main_root {
                commit_hit_main = true;
            }
        }
        assert!(
            !commit_hit_wt,
            "commit in a linked worktree must NOT emit on its checkout root \
             (proves why the badge can't clear from a checkout-scoped watch)"
        );
        assert!(
            commit_hit_main,
            "commit in a linked worktree writes refs under the MAIN repo, so \
             the main root is where the event surfaces"
        );
    }

    #[test]
    fn unwatch_stops_emission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (watcher, rx) = FileWatcher::new();
        watcher.watch_project(dir.path()).expect("watch");
        thread::sleep(Duration::from_millis(50));
        watcher.unwatch(dir.path());
        thread::sleep(Duration::from_millis(50));

        let target = dir.path().join("after_unwatch.txt");
        fs::write(&target, "x").expect("write");

        // 200 ms is well past the 50 ms debounce — if events were
        // still flowing we'd see one.
        assert!(drain(&rx, Duration::from_millis(200)).is_none());
    }
}

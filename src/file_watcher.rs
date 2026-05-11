//! Cross-platform filesystem watcher. One `notify::RecommendedWatcher`
//! services every Project; raw events feed a single debouncer thread
//! that coalesces bursts (50 ms quiet period), filters out git-internal
//! churn and editor backup noise, and emits one [`ChangeEvent`] per
//! affected Project.
//!
//! Two threads total regardless of how many Projects are open:
//! `notify`'s internal backend thread + our debouncer. The App holds
//! the receiver and drains it every frame.
//!
//! See `docs/specs/2026-05-09-job-system-and-file-watcher.md`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;

pub type ProjectId = u64;

/// Coalesced batch of changes for a single Project. The debouncer
/// emits one of these after a 50 ms quiet period — typical "save in
/// editor" sequences (write + attr-change + temp rename) coalesce into
/// one event.
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    pub project: ProjectId,
    pub paths: Vec<PathBuf>,
    pub created: bool,
    pub modified: bool,
    pub removed: bool,
    pub arrived_at: Instant,
}

const DEBOUNCE: Duration = Duration::from_millis(50);

/// Owns the OS watcher + debouncer thread. App holds one of these for
/// the session lifetime.
pub struct FileWatcher {
    inner: Arc<Mutex<Inner>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    debouncer: Mutex<Option<JoinHandle<()>>>,
    raw_tx: Sender<RawEvent>,
    out_rx: Mutex<Option<Receiver<ChangeEvent>>>,
}

/// Shared state between public API and the debouncer thread.
struct Inner {
    /// Project root → project id. Lookups walk this map by prefix to
    /// route raw paths back to their owning Project. Length bounded
    /// by user-configured projects (single-digit to low-double-digit
    /// in practice).
    roots: HashMap<PathBuf, ProjectId>,
}

enum RawEvent {
    /// Path → kind, from the notify backend.
    Fs(Event),
    /// Shutdown signal.
    Stop,
}

impl FileWatcher {
    /// Construct + start. The debouncer thread spawns lazily on the
    /// first `watch_project` call so an empty session pays nothing.
    pub fn new() -> std::io::Result<Self> {
        let (raw_tx, raw_rx) = mpsc::channel::<RawEvent>();
        let (out_tx, out_rx) = mpsc::channel::<ChangeEvent>();
        let inner = Arc::new(Mutex::new(Inner {
            roots: HashMap::new(),
        }));

        let watcher_tx = raw_tx.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                let _ = watcher_tx.send(RawEvent::Fs(ev));
            }
        })
        .map_err(notify_to_io)?;

        let debouncer_inner = Arc::clone(&inner);
        let debouncer = thread::Builder::new()
            .name("crane-fs-debounce".into())
            .spawn(move || run_debouncer(raw_rx, out_tx, debouncer_inner))?;

        Ok(Self {
            inner,
            watcher: Mutex::new(Some(watcher)),
            debouncer: Mutex::new(Some(debouncer)),
            raw_tx,
            out_rx: Mutex::new(Some(out_rx)),
        })
    }

    /// Take ownership of the receiver. The App keeps this and
    /// `try_recv()`s every frame. Only one receiver per FileWatcher.
    pub fn take_receiver(&self) -> Option<Receiver<ChangeEvent>> {
        self.out_rx.lock().take()
    }

    /// Start watching a Project's root recursively. If the project ID
    /// is already watched at a different path, the old root is
    /// replaced — handles "project moved on disk and re-added with
    /// the same ID" without leaking the stale watcher.
    /// Canonicalizes the path so prefix routing matches the
    /// realpath-form notify reports (macOS reports /private/var/...
    /// for /var/... symlinks; identical issue on Linux with /tmp).
    pub fn watch_project(&self, project: ProjectId, root: PathBuf) -> std::io::Result<()> {
        let canonical = match std::fs::canonicalize(&root) {
            Ok(c) => c,
            Err(e) => {
                // Falling back to the non-canonical path means macOS
                // FSEvents may report /private/var/... while we route
                // on /var/..., silently dropping every event. Loud
                // log so the silent-failure mode is visible.
                log::warn!(
                    "FileWatcher: canonicalize({}) failed: {e}; events for project \
                     {project} may be misrouted",
                    root.display()
                );
                root.clone()
            }
        };
        let prev_root: Option<PathBuf> = {
            let mut inner = self.inner.lock();
            let existing = inner
                .roots
                .iter()
                .find(|(_, p)| **p == project)
                .map(|(k, _)| k.clone());
            if let Some(ref old) = existing
                && old == &canonical
            {
                // Same path, same project — true no-op.
                return Ok(());
            }
            if let Some(ref old) = existing {
                inner.roots.remove(old);
            }
            inner.roots.insert(canonical.clone(), project);
            existing
        };
        if let Some(w) = self.watcher.lock().as_mut() {
            if let Some(old) = prev_root.as_ref() {
                let _ = w.unwatch(old);
            }
            w.watch(&canonical, RecursiveMode::Recursive)
                .map_err(notify_to_io)?;
        }
        Ok(())
    }

    /// Stop watching a Project. Idempotent.
    pub fn unwatch_project(&self, project: ProjectId) {
        let root_opt = {
            let mut inner = self.inner.lock();
            let root = inner
                .roots
                .iter()
                .find(|(_, p)| **p == project)
                .map(|(k, _)| k.clone());
            if let Some(r) = root.as_ref() {
                inner.roots.remove(r);
            }
            root
        };
        if let (Some(root), Some(w)) = (root_opt, self.watcher.lock().as_mut()) {
            let _ = w.unwatch(&root);
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
/// want to wake the App for.
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
    // Editor temp files + macOS metadata. Cheap suffix checks; no
    // regex, no allocations beyond the string view above.
    matches!(name, ".DS_Store" | "Thumbs.db")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with("~")
        || name.starts_with("~$")
        || name.ends_with(".tmp")
}

/// Walk the longest matching root prefix. Length of `roots` is small
/// (≤ low double digits in practice), so a linear scan is cheaper than
/// a fancy trie with allocation overhead.
fn route_to_project(roots: &HashMap<PathBuf, ProjectId>, path: &Path) -> Option<ProjectId> {
    let mut best: Option<(usize, ProjectId)> = None;
    for (root, pid) in roots {
        if path.starts_with(root) {
            let n = root.as_os_str().len();
            match best {
                Some((cur, _)) if n <= cur => {}
                _ => best = Some((n, *pid)),
            }
        }
    }
    best.map(|(_, p)| p)
}

#[derive(Default)]
struct ProjectBucket {
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
    let mut buckets: HashMap<ProjectId, ProjectBucket> = HashMap::new();

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
                    let Some(pid) = route_to_project(&roots.roots, path) else {
                        continue;
                    };
                    let bucket = buckets.entry(pid).or_default();
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
        let mut to_flush: Vec<ProjectId> = Vec::new();
        for (pid, bucket) in buckets.iter() {
            if let Some(t) = bucket.last_seen
                && now.duration_since(t) >= DEBOUNCE
            {
                to_flush.push(*pid);
            }
        }
        for pid in to_flush {
            if let Some(bucket) = buckets.remove(&pid)
                && !bucket.paths.is_empty()
            {
                let _ = out_tx.send(ChangeEvent {
                    project: pid,
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
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

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
        let mut roots: HashMap<PathBuf, ProjectId> = HashMap::new();
        roots.insert(PathBuf::from("/parent"), 1);
        roots.insert(PathBuf::from("/parent/child"), 2);
        assert_eq!(
            route_to_project(&roots, Path::new("/parent/child/foo.rs")),
            Some(2)
        );
        assert_eq!(
            route_to_project(&roots, Path::new("/parent/sibling/bar.rs")),
            Some(1)
        );
        assert_eq!(route_to_project(&roots, Path::new("/elsewhere/baz")), None);
    }

    #[test]
    fn end_to_end_create_modify_emits_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let watcher = FileWatcher::new().expect("watcher");
        let rx = watcher.take_receiver().expect("rx");
        watcher
            .watch_project(7, dir.path().to_path_buf())
            .expect("watch");

        // Give the OS backend a moment to register.
        thread::sleep(Duration::from_millis(50));

        let target = dir.path().join("hello.txt");
        fs::write(&target, "hi").expect("write");
        // Touch again to make sure modify also fires.
        fs::write(&target, "hello").expect("write2");

        let ev = drain(&rx, Duration::from_secs(2)).expect("change event");
        assert_eq!(ev.project, 7);
        assert!(
            ev.paths.iter().any(|p| p.ends_with("hello.txt")),
            "paths missing target: {:?}",
            ev.paths
        );
        assert!(ev.created || ev.modified);
    }

    #[test]
    fn unwatch_stops_emission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let watcher = FileWatcher::new().expect("watcher");
        let rx = watcher.take_receiver().expect("rx");
        watcher
            .watch_project(11, dir.path().to_path_buf())
            .expect("watch");
        thread::sleep(Duration::from_millis(50));
        watcher.unwatch_project(11);
        thread::sleep(Duration::from_millis(50));

        let target = dir.path().join("after_unwatch.txt");
        fs::write(&target, "x").expect("write");

        // 200 ms is well past the 50 ms debounce — if events were
        // still flowing we'd see one.
        assert!(drain(&rx, Duration::from_millis(200)).is_none());
    }
}

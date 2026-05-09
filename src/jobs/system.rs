use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use parking_lot::{Condvar, Mutex};

/// Identity of the entity owning a job. Used for cancellation by scope
/// (close a tab → cancel everything keyed under that tab).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Scope {
    Global,
    Project(u64),
    Workspace(u64),
    Tab(u64),
    Pane(u64),
}

/// Derived from user focus by the App. Workers process higher-priority
/// jobs first; ties break by submission order (FIFO across keys, LIFO
/// per key via dedup).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Idle = 0,
    Background = 1,
    Visible = 2,
    Foreground = 3,
}

/// Three pools, all fixed-size, all long-lived. See design doc §Surface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Pool {
    /// CPU-bound: highlight, diff, parse. Sized to physical cores.
    Cpu,
    /// Blocking I/O: git shell-out, file reads, FS walks. Fixed at 4.
    Io,
}

/// Stable identity for dedup. Submitting a new job under an existing
/// key cancels the previous one — newer inputs always win.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct JobKey {
    pub scope: Scope,
    pub kind: &'static str,
}

impl JobKey {
    pub fn new(scope: Scope, kind: &'static str) -> Self {
        Self { scope, kind }
    }
}

/// Cooperative cancellation. Workers check this at safe boundaries
/// (between hunks, between files), never mid-write.
#[derive(Clone, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

pub enum JobOutput<T> {
    Done(T),
    Cancelled,
}

/// Returned to the caller; held in pane state. `try_recv()` each frame.
pub struct JobHandle<T> {
    rx: Receiver<JobOutput<T>>,
    cancel: CancelToken,
}

impl<T> JobHandle<T> {
    pub fn try_recv(&self) -> Option<JobOutput<T>> {
        self.rx.try_recv().ok()
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }
}

type WorkFn = Box<dyn FnOnce(&CancelToken) + Send + 'static>;

/// Internal queued unit. Pools order by `(priority, seq)` — higher
/// priority first; within the same priority, lower seq (older) first.
struct QueuedJob {
    seq: u64,
    priority: Priority,
    key: JobKey,
    cancel: CancelToken,
    work: WorkFn,
}

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq && self.priority == other.priority
    }
}
impl Eq for QueuedJob {}
impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct PoolQueue {
    heap: BinaryHeap<QueuedJob>,
    shutting_down: bool,
}

struct PoolState {
    queue: Mutex<PoolQueue>,
    cv: Condvar,
}

impl PoolState {
    fn new() -> Self {
        Self {
            queue: Mutex::new(PoolQueue {
                heap: BinaryHeap::new(),
                shutting_down: false,
            }),
            cv: Condvar::new(),
        }
    }

    fn push(&self, job: QueuedJob) {
        let mut q = self.queue.lock();
        q.heap.push(job);
        self.cv.notify_one();
    }

    fn pop_blocking(&self) -> Option<QueuedJob> {
        let mut q = self.queue.lock();
        loop {
            if let Some(j) = q.heap.pop() {
                return Some(j);
            }
            if q.shutting_down {
                return None;
            }
            self.cv.wait(&mut q);
        }
    }

    fn shutdown(&self) {
        let mut q = self.queue.lock();
        q.shutting_down = true;
        self.cv.notify_all();
    }
}

/// Single source of truth for "what's in flight." Maps key → live
/// cancel token so submit-with-existing-key can supersede.
struct Registry {
    live: HashMap<JobKey, CancelToken>,
}

impl Registry {
    fn new() -> Self {
        Self {
            live: HashMap::new(),
        }
    }

    fn replace(&mut self, key: JobKey, token: CancelToken) {
        if let Some(prev) = self.live.insert(key, token) {
            prev.cancel();
        }
    }

    fn cancel_scope(&mut self, scope: Scope) {
        self.live.retain(|k, tok| {
            if k.scope == scope {
                tok.cancel();
                false
            } else {
                true
            }
        });
    }

    fn finish(&mut self, key: &JobKey, token: &CancelToken) {
        if let Some(current) = self.live.get(key) {
            if Arc::ptr_eq(&current.0, &token.0) {
                self.live.remove(key);
            }
        }
    }
}

/// The trunk. One per app. Holds worker threads, the queue, the
/// registry of in-flight jobs, and the egui context for repaint.
pub struct JobSystem {
    cpu: Arc<PoolState>,
    io: Arc<PoolState>,
    registry: Arc<Mutex<Registry>>,
    seq: AtomicU64,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl JobSystem {
    /// Construct with a repaint callback (typically `ctx.request_repaint()`).
    /// Pass `None` in tests.
    pub fn new(repaint: Option<Arc<dyn Fn() + Send + Sync>>) -> Arc<Self> {
        let cpu_size = thread::available_parallelism()
            .map(|n| n.get().clamp(2, 8))
            .unwrap_or(4);
        Self::with_sizes(cpu_size, 4, repaint)
    }

    pub fn with_sizes(
        cpu: usize,
        io: usize,
        repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Arc<Self> {
        let sys = Arc::new(Self {
            cpu: Arc::new(PoolState::new()),
            io: Arc::new(PoolState::new()),
            registry: Arc::new(Mutex::new(Registry::new())),
            seq: AtomicU64::new(0),
            repaint,
            workers: Mutex::new(Vec::new()),
        });

        let mut workers = sys.workers.lock();
        for i in 0..cpu {
            let pool = Arc::clone(&sys.cpu);
            let registry = Arc::clone(&sys.registry);
            workers.push(
                thread::Builder::new()
                    .name(format!("crane-cpu-{i}"))
                    .spawn(move || run_worker(pool, registry))
                    .expect("spawn cpu worker"),
            );
        }
        for i in 0..io {
            let pool = Arc::clone(&sys.io);
            let registry = Arc::clone(&sys.registry);
            workers.push(
                thread::Builder::new()
                    .name(format!("crane-io-{i}"))
                    .spawn(move || run_worker(pool, registry))
                    .expect("spawn io worker"),
            );
        }
        drop(workers);
        sys
    }

    /// Submit a job. Replaces any in-flight job with the same key
    /// (the previous one's cancel token flips; its result is dropped).
    pub fn submit<T, F>(
        self: &Arc<Self>,
        key: JobKey,
        priority: Priority,
        pool: Pool,
        work: F,
    ) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&CancelToken) -> T + Send + 'static,
    {
        let cancel = CancelToken::new();
        self.registry.lock().replace(key.clone(), cancel.clone());

        let (tx, rx) = mpsc::channel();
        let cancel_for_worker = cancel.clone();
        let registry = Arc::clone(&self.registry);
        let key_for_worker = key.clone();
        let repaint = self.repaint.as_ref().map(Arc::clone);

        let work_fn: WorkFn = Box::new(move |tok: &CancelToken| {
            let out = if tok.is_cancelled() {
                JobOutput::Cancelled
            } else {
                let value = work(tok);
                if tok.is_cancelled() {
                    JobOutput::Cancelled
                } else {
                    JobOutput::Done(value)
                }
            };
            let _ = tx.send(out);
            registry.lock().finish(&key_for_worker, tok);
            if let Some(r) = repaint {
                r();
            }
        });

        let queued = QueuedJob {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            priority,
            key,
            cancel: cancel_for_worker,
            work: work_fn,
        };

        match pool {
            Pool::Cpu => self.cpu.push(queued),
            Pool::Io => self.io.push(queued),
        }

        JobHandle { rx, cancel }
    }

    /// Cancel a single job by key.
    pub fn cancel(&self, key: &JobKey) {
        let mut reg = self.registry.lock();
        if let Some(tok) = reg.live.remove(key) {
            tok.cancel();
        }
    }

    /// Cancel every job in a scope (e.g. tab close).
    pub fn cancel_scope(&self, scope: Scope) {
        self.registry.lock().cancel_scope(scope);
    }

    /// Number of in-flight jobs (pending + running). Test-only visibility.
    #[cfg(test)]
    pub fn live_count(&self) -> usize {
        self.registry.lock().live.len()
    }

    /// Stop accepting new work, signal workers, join. Workers finish
    /// the job they're running (with cancel token already flipped on
    /// any superseded job) and exit.
    pub fn shutdown(&self) {
        for k in self
            .registry
            .lock()
            .live
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
            k.cancel();
        }
        self.cpu.shutdown();
        self.io.shutdown();
        let mut workers = self.workers.lock();
        for w in workers.drain(..) {
            let _ = w.join();
        }
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        self.cpu.shutdown();
        self.io.shutdown();
        let mut workers = self.workers.lock();
        for w in workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn run_worker(pool: Arc<PoolState>, _registry: Arc<Mutex<Registry>>) {
    while let Some(job) = pool.pop_blocking() {
        (job.work)(&job.cancel);
    }
}

# JobSystem + FileWatcher — Design

Date: 2026-05-09
Branch: `feat/job-system-design`
Status: design only — no code yet

## Why this exists

Crane today reacts to "things might have changed" with **threads on timers**. The git status loop in `src/state/state.rs:1842` spawns a fresh `std::thread` every tick (1s active / 5s inactive) per workspace. With 8 workspaces open that is 8 process spawns per second running `git status` whose answer is almost always "nothing changed." Diff and syntect re-run on every render frame. Markdown re-parses on every render frame. There is no central notion of "what work needs doing, at what priority, owned by whom."

Two primitives fix the whole class of problem:

- **JobSystem** — one bounded set of workers, keyed jobs with dedup + cancellation, priority derived from user focus. Answers *how* background work runs.
- **FileWatcher** — OS-level change notifications (FSEvents / inotify / ReadDirectoryChangesW) per Project, debounced, feeding invalidations into JobSystem. Answers *when* work needs to run.

Together: the app does work only when reality changes, at the priority the user cares about, on a bounded pool. No timer-driven polling on the hot path; timers exist only as a staleness backstop.

## Non-goals

- No async runtime. No Tokio. The poller, watchers, and workers are plain `std::thread`s. (When LSP lands, a scoped `tokio::runtime::Builder::new_current_thread()` gets added inside `crates/crane_lsp` — out of scope here.)
- No global rewrite. Every consumer migrates one at a time. JobSystem ships with one consumer (git status), then each subsequent migration is independent.
- No new dependencies beyond `notify` (cross-platform file watching) and `parking_lot` (already used).
- No removal of the per-Terminal PTY reader thread. Long-lived blocking readers stay as-is — they don't fit the job model and don't need it.

## Surface

### JobSystem

```text
JobSystem
├── submit(key, priority, work) -> JobHandle
├── cancel(key)
├── invalidate(key)              // recompute even if a result is cached
├── set_priority(key, priority)  // focus changed
└── shutdown()                   // app close

JobKey   = (Scope, &'static str)
Scope    = Global | Project(ProjectId) | Workspace(WorkspaceId)
         | Tab(TabId) | Pane(PaneId)
Priority = Foreground | Visible | Background | Idle
JobHandle = { receiver: mpsc::Receiver<JobOutput>, cancel_token: Arc<AtomicBool> }
```

**Invariants** (the system is only useful if these hold):

1. **Key dedup**: `submit` with a key that already has a pending or running job replaces the previous one. The replaced job's `cancel_token` flips to `true`; its result, if it lands, is dropped. Render never sees stale data for a key whose inputs changed.
2. **Result delivery is one-shot**: each `JobHandle` receives exactly one message — either `Done(T)` or `Cancelled`. Consumers store the receiver in their pane state and `try_recv()` on render.
3. **Repaint is the worker's responsibility**: every worker calls `ctx.request_repaint()` after `tx.send(...)`. No consumer ever has to remember.
4. **Cancellation is cooperative but checked at boundaries**: workers check `cancel_token` before expensive subwork (per file, per hunk, per line range). They don't have to be perfectly preemptive — just responsive within ~50ms.
5. **No work runs after `shutdown()` returns**: workers join, queue drains, no orphan threads.

**Workers** — three pools, all fixed-size, all long-lived:

| Pool | Size | What runs there | Why |
|---|---|---|---|
| CPU | `available_parallelism()`, clamped 2..=8 | syntect highlight, similar diff, pulldown_cmark parse | CPU-bound — more workers than cores thrashes |
| I/O | 4 fixed | `git` shell-out, `fs::read_to_string`, FS walk | Blocking on syscalls — needs concurrency, not cores |
| Poller | 1 | the staleness-backstop loop (see below) | Single owner of the priority queue; no contention |

That is **8–13 threads total for the whole app** regardless of how many panes, tabs, or workspaces are open. Compared to today's "fresh thread every git tick + thread per tab on demand," this is a hard ceiling rather than a function of UI state.

**Priority ordering**:

```
Foreground = the focused Pane / Workspace / Tab
Visible    = on-screen but not focused (other panes, Right Panel changes)
Background = collapsed in Left Panel, off-screen tabs
Idle       = window unfocused; nothing visible
```

Priority is **derived**, not hand-assigned by callers. Callers pass a `Scope`; the JobSystem looks up the current focus state of that scope and computes the priority. When focus changes, `App::on_focus_change` calls `JobSystem::reprioritize_all()` once — the queue resorts in O(n log n) where n is at most a few hundred pending jobs.

**Queue discipline** — within the same priority bucket: **LIFO per key, FIFO across keys**. If the user types in a file (same `(Pane, "highlight")` key), the newer job preempts the older. Across different keys (highlight for tab A vs tab B), oldest wins so nothing starves.

### FileWatcher

```text
FileWatcher
├── watch_project(project_id, root_path) -> WatcherHandle
├── unwatch_project(project_id)
└── shutdown()

ChangeEvent
├── project: ProjectId
├── paths: Vec<PathBuf>     // coalesced batch
├── kinds: BitSet<Kind>     // Created | Modified | Removed | Renamed
└── arrived_at: Instant
```

**One watcher per Project** (not per Workspace, not per file). The `notify` crate gives recursive watching natively on macOS and Windows; on Linux it walks the tree and registers inotify on each directory. Each watcher runs its own thread for the OS callback, pushes raw events into a per-project debouncer, and emits a coalesced `ChangeEvent` after a quiet period of **50 ms**.

**Filtering** — events are dropped before they reach JobSystem if the path matches:

- `<repo>/.git/objects/**`, `.git/index.lock`, `.git/HEAD.lock` — git's own internal churn
- gitignored paths (`target/`, `node_modules/`, `.next/`, `dist/`) — checked once via `git check-ignore` per new path, cached
- macOS junk: `.DS_Store`, `~$*`

The filter list is hand-curated, not user-configurable in v1. Add a `crane.yaml` knob later if needed.

**Routing** — the watcher does not know about workspaces, panes, or jobs. It emits events to a single `mpsc::Sender<ChangeEvent>` owned by the App. The App's event loop translates events into JobSystem invalidations:

```text
ChangeEvent → for each affected workspace under this project:
                JobSystem::invalidate((Workspace(wid), "git_status"))
                JobSystem::invalidate((Workspace(wid), "branch_list"))
              for each open Files Pane rooted at the changed dir:
                JobSystem::invalidate((Pane(pid), "fs_walk"))
              for each Diff tab whose file changed:
                JobSystem::invalidate((Tab(tid), "diff"))
              for each Markdown / File tab whose file changed:
                JobSystem::invalidate((Tab(tid), "reload"))
```

**Network drives** — FSEvents and inotify don't reliably fire for NFS / SMB. The watcher detects this at `watch_project` time (statfs / fs type check) and falls back to **polling-via-JobSystem** for that project: a single Idle-priority job re-walks the tree every 30s. No code path outside the watcher knows the difference.

**Resource limits** — Linux inotify has a per-user watch limit (default 8192). For monorepos that exceed this, the watcher logs once and falls back to polling for the over-limit subtree. No silent degradation.

### Poller (the staleness backstop)

This is the single thread that replaces `state.rs:1842`'s thread-per-tick design. Owns a priority queue of `(JobKey, deadline)` pairs. Each tick:

1. Pop the entry whose `deadline` is earliest.
2. If `deadline > now`, sleep until `deadline` (or until the wake `Condvar` fires from a focus change / file event).
3. Otherwise, submit the corresponding job to JobSystem, recompute next deadline based on current priority, push back.

Deadlines per priority:

| Priority | Staleness budget |
|---|---|
| Foreground | 1 s |
| Visible | 5 s |
| Background | 30 s |
| Idle | never (skip until something invalidates it) |

The poller exists for things the FileWatcher cannot observe: **git remote state, system clock changes, things that may have changed for reasons outside the filesystem**. For everything filesystem-derived, the FileWatcher fires first and the poller is just a safety net.

## Lifecycle

**App startup**:

1. Construct `JobSystem` with worker pools and an `egui::Context` clone for repaints.
2. Construct `FileWatcher` with a channel to App.
3. Wire watcher → App event loop → JobSystem invalidations.
4. For each persisted Project, call `FileWatcher::watch_project`.

**Project added**: `App::add_project` calls `FileWatcher::watch_project`. Initial git status / branch list is invalidated immediately so the new Project lights up.

**Project removed**: `FileWatcher::unwatch_project` is called before the Project is dropped. JobSystem cancels all jobs scoped under that ProjectId.

**Pane / Tab closed**: callers invoke `JobSystem::cancel(Scope::Pane(pid))` (or `Tab(tid)`). All pending and running jobs for that scope flip their cancel token. Workers exit early at the next checkpoint.

**Window focus lost**: App calls `JobSystem::demote_all_to_idle()`. The poller stops waking; running jobs continue (mid-flight cancellation hurts more than it helps for short jobs); the watcher continues delivering events but most invalidations land at Idle priority.

**Window focus regained**: priorities recompute from current pane focus. Anything stale beyond Foreground budget runs immediately.

**Shutdown**: App calls `JobSystem::shutdown()` and `FileWatcher::shutdown()` in that order. Both join their threads. `cancel_token` is set on every in-flight job; workers check it within 50 ms and bail.

## Migration plan (incremental, ships one PR at a time)

Each step is independently mergeable and improves things on its own.

1. **Land the JobSystem skeleton** — types, three pools, key dedup, cancellation, no consumers yet. Unit tests for: dedup replaces older job, cancel flips token, shutdown joins cleanly, priority re-sorts. ~400 lines.
2. **Migrate git status** — replace `state.rs:1842`'s thread-per-tick with `JobSystem::submit((Workspace(wid), "git_status"), priority, ...)`. The existing `git_rx: Option<Receiver>` field is replaced with `git_job: Option<JobHandle>`. Behavior identical from the user's POV; thread spawns drop from N/sec to ~0/sec at idle.
3. **Land the FileWatcher** — new module `src/file_watcher.rs`. One watcher per Project, debouncer, filter list, fallback-to-polling path. Wire into App. With (2) already in place, git status now updates instantly on real change instead of within 1 s.
4. **Migrate diff computation** — `DiffTabData` grows a `computed: Option<JobHandle<Vec<Row>>>` field. `render_diff_body` reads from cache. Inputs change → invalidate. Tab close → cancel. Biggest user-visible latency win.
5. **Migrate syntect highlight** — same shape as diff. `FileTab` grows a `highlight: Option<JobHandle<...>>`. Per-content-version key so rapid edits supersede.
6. **Migrate markdown parse** — same shape. Cached parsed event stream keyed by content hash.
7. **Migrate Files Pane FS walk** — invalidated by FileWatcher events for the rooted directory.
8. **Migrate branch list enumeration** — invalidated by `.git/refs/**` watcher events.
9. **Document the pattern in `CLAUDE.md`** — new Pane types use JobSystem by default; raw `std::thread::spawn` for transient work becomes a code smell.

Steps 4–8 are independent of each other. Any subset can ship in any order after step 3.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| FileWatcher misses an event (rare on FSEvents/inotify; common on network mounts) | Poller's staleness backstop catches it within the priority bucket budget. Network mounts auto-fall-back to polling. |
| Worker pool starvation by one mega-job (50k-line diff hogs CPU pool) | CPU pool sized ≥ 2; one stuck worker leaves at least one free. Long jobs check cancel token between hunks, so a focus change can preempt. |
| Mutex contention on the JobSystem queue | Single `parking_lot::Mutex` around the priority heap; lock held only for push/pop, never across `work()`. Profile if it shows up. |
| Stale results landing after a key was invalidated | Workers compare `cancel_token` before `tx.send`; if cancelled, drop result. Receivers also tag jobs with a generation number for double-safety. |
| `notify` crate cross-platform quirks (rename detection on Linux is two events) | The 50 ms debouncer coalesces both halves; consumers see one `Renamed` after the quiet period. |
| Shutdown deadlock if a worker is mid-`Command::wait()` on a hung git | I/O pool workers wrap subprocesses with a 30 s hard timeout. Cancel token kills the child process. |
| Priority inversion (a Background job blocks the I/O pool while a Foreground job waits) | Acceptable in v1 — the I/O pool is small but the longest job is bounded (git timeout). If it bites, add a small Foreground-only reserve worker. |
| `egui::Context` cloning cost | Cheap — `Arc` internally. Pass into every job. |

## Open questions for v2

- Should the CPU pool steal work from the I/O pool when idle, or stay segregated? (Start segregated; revisit if profiling shows imbalance.)
- Should JobSystem expose a synchronous `await_now(key)` for tests / one-shot UI flows? (Probably yes, gated on `cfg(test)`.)
- Where does LSP live? Likely its own scoped `tokio::runtime::Builder::new_current_thread()` inside `crates/crane_lsp`, talking to JobSystem via channels. Out of scope for this design.
- Telemetry: should we record per-key job counts and durations for a `crane debug jobs` command? Useful for catching regressions; defer to a follow-up.

## Mapping to async-Rust concepts (and why we still don't need a runtime-wide Tokio)

The async-Rust model has four ideas worth borrowing **as semantics**, even though our runtime is plain threads:

| Async concept | What it means | How JobSystem expresses it |
|---|---|---|
| **Future (lazy)** | A description of work that does nothing until polled | `submit(...)` returns a `JobHandle`. The work doesn't start until a worker picks it up. The handle is the "description"; the worker is the "executor." |
| **Task (spawned future)** | A Future handed to a runtime, scheduled, executed | Once a job is dequeued by a worker, it is the task. The cancel token + `JobHandle` give the same observability a `JoinHandle` does. |
| **`await`** | Sequential composition; pause until done | Consumer-side: `try_recv()` on the `JobHandle` receiver each frame. Render never blocks; it asks "are you done yet?" — the immediate-mode equivalent of `await`. |
| **`spawn`** | Fire-and-forget | `submit` with no consumer holding the handle. Cancellation still works via key. |
| **`join!`** | Concurrent run-until-all-done | Consumer holds N `JobHandle`s, drains all of them before rendering "done." Composable without runtime support. |
| **`select!`** | Race; cancel the losers | Submitting a new job under the same key cancels the old. That **is** select-on-supersede. |
| **Cancellation safety** | Dropping a future mid-write must not corrupt state | Workers check `cancel_token` only at **boundaries** — between hunks, between files, never mid-`fs::write`. Same rule as Tokio: cancel safe points are deliberate, not "anywhere a `.await` could be." |
| **`spawn_blocking`** | Don't run CPU-heavy work on the async reactor | We **only** have spawn_blocking. The CPU pool is exactly that: a dedicated thread pool for blocking/CPU work. There is no reactor to starve, so the design is one-sided by construction. |

**The macro angle.** Async Rust's ergonomic win is that `async fn` + `.await` lets you write linear code that's actually a state machine. Crane can get the same ergonomic with **declarative macros** without a runtime — e.g. a `job!{ key: ..., priority: ..., body: { ... } }` macro that wraps `submit` with cancel-token plumbing, repaint calls, and result delivery. Same readability, no executor. If a future migration wants real `async fn`, the macro can be re-pointed at a scoped `tokio::runtime::Builder::new_current_thread()` without consumers changing.

**Where a small Tokio runtime still earns its slot, later.** Two concrete cases — both scoped, both opt-in, neither part of v1:

1. **LSP** (in `crates/crane_lsp`): each language server has bidirectional stdio with concurrent in-flight requests + notifications. A single `current_thread` runtime per server multiplexes this far better than two threads-with-channels. The runtime is invisible outside the crate; it talks to JobSystem via plain `mpsc`.
2. **Network panes** (remote SSH, model streaming, collab presence): same shape — many idle sockets, occasional bursts. One `current_thread` runtime, scoped to that subsystem.

In both cases the Tokio runtime is a **leaf** in the architecture, not the trunk. JobSystem stays the trunk because the rest of Crane (git shell-out, file I/O, CPU jobs) does not benefit from `select!` / `join!` semantics — the work is one-shot, not a pipeline of awaits.

**Cancellation safety, applied to Crane's worst case.** The dangerous spot is `file_save.rs` — write-then-conflict-check is two syscalls with shared semantics. Today it's synchronous on the UI thread (bad latency, but cancellation-safe by accident). Migrated to JobSystem, the worker must:

- Acquire a per-tab `Mutex<SaveLock>` before writing.
- Check `cancel_token` only **before** the write begins, never between write and conflict-check.
- If cancelled mid-write, complete the current syscall, release the lock, then exit. The result is dropped but the file is not corrupted.

This is the exact rule `select!` enforces in Tokio: cancellation is safe at await points, dangerous mid-state-mutation. We adopt the rule explicitly rather than relying on the absence of cancellation to save us.

## Success criteria

After all migration steps land, on an idle 12-workspace project on a 2-core machine:

- `git status` subprocess spawns / sec drops from ~12 to **0** at idle, ~1–2/sec under active editing.
- Frame time for a Diff Pane on a 5k-line diff drops from ~150 ms to **< 1 ms** (cached) on every frame after the first.
- Total live thread count stays at **8–13 + 1 per Terminal Pane**, regardless of how many tabs are open.
- Saving a file in any external editor reflects in Crane's Right Panel within **100 ms** (50 ms watcher debounce + git status I/O).
- Closing a tab mid-highlight does not produce a stale repaint; the cancelled job's result is dropped.

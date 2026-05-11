//! Background job execution. See `docs/specs/2026-05-09-job-system-and-file-watcher.md`.
//!
//! One bounded set of workers, keyed jobs with dedup + cancellation,
//! priority derived from user focus. The trunk of Crane's background
//! work model — git status, diff compute, syntect highlight, file
//! reads, FS walks all submit through here once migrated.

// Skeleton landed in step 1 of the migration plan. Consumers (git
// status, diff, syntect, ...) wire up in subsequent steps. The
// `allow(dead_code)` covers the gap between landing the type surface
// and the first consumer.
#[allow(dead_code)]
mod system;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use system::{
    CancelToken, JobHandle, JobKey, JobOutput, JobSystem, Pool, Priority, Scope,
};

use std::sync::{Arc, OnceLock};

static GLOBAL: OnceLock<Arc<JobSystem>> = OnceLock::new();

/// Install the process-wide JobSystem. Called once by App on first
/// init. Subsequent calls are a programming error: render code reads
/// from the singleton, so swapping it would leave callers holding
/// Arcs to a JobSystem that's about to be dropped. Logs a warning
/// to make the misuse visible.
pub fn install(jobs: Arc<JobSystem>) {
    if GLOBAL.set(jobs).is_err() {
        log::warn!(
            "jobs::install called more than once; ignoring (first install wins). \
             Render code already holds the original Arc<JobSystem>."
        );
    }
}

/// Read the process-wide JobSystem. None until `install` has been
/// called. Render code uses this to submit jobs without plumbing an
/// Arc through every function signature — lock-free read.
pub fn global() -> Option<Arc<JobSystem>> {
    GLOBAL.get().map(Arc::clone)
}

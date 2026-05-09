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

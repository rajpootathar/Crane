use super::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

fn drain<T>(handle: &JobHandle<T>, timeout: Duration) -> Option<JobOutput<T>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(out) = handle.try_recv() {
            return Some(out);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn submit_runs_work_and_returns_done() {
    let sys = JobSystem::with_sizes(2, 2, None);
    let handle = sys.submit::<i32, _>(
        JobKey::new(Scope::Global, "compute"),
        Priority::Foreground,
        Pool::Cpu,
        |_tok| 42,
    );
    match drain(&handle, Duration::from_secs(1)) {
        Some(JobOutput::Done(v)) => assert_eq!(v, 42),
        other => panic!("expected Done(42), got {:?}", other.is_some()),
    }
}

#[test]
fn dedup_supersedes_earlier_job() {
    let sys = JobSystem::with_sizes(1, 1, None);
    let key = JobKey::new(Scope::Tab(7), "highlight");

    // First job blocks until released so the second is guaranteed
    // to land while the first is still pending or running.
    let gate = Arc::new(AtomicUsize::new(0));
    let gate_a = Arc::clone(&gate);
    let h1 = sys.submit::<&'static str, _>(
        key.clone(),
        Priority::Foreground,
        Pool::Cpu,
        move |tok| {
            while gate_a.load(Ordering::Acquire) == 0 {
                if tok.is_cancelled() {
                    return "cancelled-internal";
                }
                thread::sleep(Duration::from_millis(2));
            }
            "first"
        },
    );

    // Submit replacement under the same key.
    let h2 = sys.submit::<&'static str, _>(
        key.clone(),
        Priority::Foreground,
        Pool::Cpu,
        |_tok| "second",
    );

    // First should observe its cancel token tripping.
    let deadline = Instant::now() + Duration::from_secs(1);
    while !h1.cancel_token().is_cancelled() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        h1.cancel_token().is_cancelled(),
        "older job's token must flip on supersede"
    );

    // Release the first so the worker can finish and pick up the second.
    gate.store(1, Ordering::Release);

    let out = drain(&h2, Duration::from_secs(1)).expect("second job result");
    match out {
        JobOutput::Done(s) => assert_eq!(s, "second"),
        JobOutput::Cancelled => panic!("second job should not be cancelled"),
    }
}

#[test]
fn cancel_scope_flips_all_matching_tokens() {
    let sys = JobSystem::with_sizes(1, 1, None);
    let scope = Scope::Pane(99);

    let blockers: Vec<_> = (0..3)
        .map(|i| {
            let kind: &'static str = match i {
                0 => "a",
                1 => "b",
                _ => "c",
            };
            sys.submit::<bool, _>(
                JobKey::new(scope, kind),
                Priority::Background,
                Pool::Cpu,
                |tok| {
                    while !tok.is_cancelled() {
                        thread::sleep(Duration::from_millis(2));
                    }
                    true
                },
            )
        })
        .collect();

    sys.cancel_scope(scope);

    for h in &blockers {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !h.cancel_token().is_cancelled() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            h.cancel_token().is_cancelled(),
            "scope cancellation must flip every matching token"
        );
    }
}

#[test]
fn shutdown_joins_workers_cleanly() {
    let sys = JobSystem::with_sizes(2, 2, None);
    let _h = sys.submit::<i32, _>(
        JobKey::new(Scope::Global, "tiny"),
        Priority::Visible,
        Pool::Io,
        |_| 1,
    );
    // Match Arc::try_unwrap pattern: drop the only Arc clone we hold.
    let inner = Arc::try_unwrap(sys).unwrap_or_else(|_| panic!("only one Arc"));
    drop(inner); // Drop impl shuts down + joins.
}

#[test]
fn repaint_is_invoked_after_completion() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_cb = Arc::clone(&counter);
    let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        counter_for_cb.fetch_add(1, Ordering::Relaxed);
    });
    let sys = JobSystem::with_sizes(1, 1, Some(repaint));
    let h = sys.submit::<(), _>(
        JobKey::new(Scope::Global, "ping"),
        Priority::Foreground,
        Pool::Cpu,
        |_| (),
    );
    let _ = drain(&h, Duration::from_secs(1));
    let deadline = Instant::now() + Duration::from_millis(200);
    while counter.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        counter.load(Ordering::Relaxed) >= 1,
        "repaint callback must fire after completion"
    );
}

#[test]
fn worker_panic_does_not_kill_pool_and_cleans_registry() {
    // Critical for production: a panicking job (syntect bug, git
    // output unwrap, etc.) must not take down its worker — with
    // CPU pool clamped to 1 on a 2-core machine, that's the whole
    // pool. Additionally the registry entry must be released so
    // future submits under the same key aren't immediately
    // superseded by a phantom stale token.
    let sys = JobSystem::with_sizes(1, 1, None);

    // First: a job that panics.
    let panic_handle = sys.submit::<i32, _>(
        JobKey::new(Scope::Global, "panicker"),
        Priority::Foreground,
        Pool::Cpu,
        |_tok| panic!("intentional test panic"),
    );

    // Consumer sees Disconnected (try_recv → None), not a hang.
    let deadline = Instant::now() + Duration::from_secs(1);
    while !panic_handle.is_disconnected() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        panic_handle.is_disconnected(),
        "panicked worker must close its sender so consumer doesn't poll forever"
    );

    // Registry must have released the panicked job's entry, otherwise
    // live_count would be 1 here.
    let live_after_panic = sys.live_count();
    assert_eq!(
        live_after_panic, 0,
        "panicked job must clean its registry entry (live_count={live_after_panic})"
    );

    // Pool must still be alive — a second submit completes normally.
    let ok_handle = sys.submit::<i32, _>(
        JobKey::new(Scope::Global, "post_panic"),
        Priority::Foreground,
        Pool::Cpu,
        |_tok| 99,
    );
    let out = drain(&ok_handle, Duration::from_secs(1)).expect("post-panic job result");
    match out {
        JobOutput::Done(v) => assert_eq!(v, 99),
        JobOutput::Cancelled => panic!("post-panic job should run, not cancel"),
    }
}

#[test]
fn drop_cancels_in_flight_jobs_before_join() {
    // App-shutdown shape: a long-running job is in flight when the
    // JobSystem is dropped. The Drop impl must flip cancel tokens
    // *before* joining workers, else shutdown waits for the job to
    // finish naturally.
    let sys = JobSystem::with_sizes(1, 1, None);
    let saw_cancel = Arc::new(AtomicUsize::new(0));
    let saw_cancel_inner = Arc::clone(&saw_cancel);
    let _h = sys.submit::<(), _>(
        JobKey::new(Scope::Global, "long"),
        Priority::Background,
        Pool::Cpu,
        move |tok| {
            // Spin until cancelled. Without the Drop-cancel fix this
            // would hang forever; with it, drop unblocks us within
            // a few millis.
            while !tok.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            saw_cancel_inner.fetch_add(1, Ordering::Relaxed);
        },
    );

    // Give the worker a moment to claim the job.
    thread::sleep(Duration::from_millis(20));

    // Drop the only Arc — Drop runs, cancels, joins. Bounded time.
    let start = Instant::now();
    let inner = Arc::try_unwrap(sys).unwrap_or_else(|_| panic!("only one Arc"));
    drop(inner);
    let elapsed = start.elapsed();

    assert_eq!(
        saw_cancel.load(Ordering::Relaxed),
        1,
        "worker must observe cancellation"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "Drop must not wait for natural completion (took {elapsed:?})"
    );
}

#[test]
fn rapid_submits_same_key_only_newest_wins() {
    // Reviewer scenario: a tab edits rapidly, generating a burst of
    // submits under the same key. The newest wins; earlier ones see
    // their tokens flipped. Without stable keys, dedup never fires
    // and the I/O pool burns cycles on results no one reads.
    let sys = JobSystem::with_sizes(1, 1, None);
    let key = JobKey::new(Scope::Tab(42), "compute");
    let executed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for i in 0..5 {
        let executed = Arc::clone(&executed);
        let h = sys.submit::<usize, _>(
            key.clone(),
            Priority::Foreground,
            Pool::Cpu,
            move |tok| {
                // Yield so the next submit can race.
                thread::sleep(Duration::from_millis(5));
                if tok.is_cancelled() {
                    return i;
                }
                executed.fetch_add(1, Ordering::Relaxed);
                i
            },
        );
        handles.push(h);
    }

    // Earlier handles should have their tokens flipped by the time
    // the last submit replaces the registry entry.
    let last = handles.pop().unwrap();
    for h in &handles {
        assert!(
            h.cancel_token().is_cancelled(),
            "every superseded job's token must be cancelled"
        );
    }

    let _ = drain(&last, Duration::from_secs(1));
    // At least the last one ran; earlier ones may have run briefly
    // before their token flipped (cooperative cancellation), but
    // the harness above only burns 5 ms each — total executions
    // should be small, not 5.
    let ran = executed.load(Ordering::Relaxed);
    assert!(
        ran <= 2,
        "dedup should keep executed count low under rapid resubmits (saw {ran})"
    );
}

#[test]
fn priority_higher_runs_first_when_workers_are_busy() {
    // Single CPU worker. Submit a Background job that holds the worker,
    // then enqueue a Foreground and a Background. Foreground must
    // complete before the second Background.
    let sys = JobSystem::with_sizes(1, 1, None);
    let release = Arc::new(AtomicUsize::new(0));

    let release_a = Arc::clone(&release);
    let _holding = sys.submit::<(), _>(
        JobKey::new(Scope::Global, "hold"),
        Priority::Background,
        Pool::Cpu,
        move |tok| {
            while release_a.load(Ordering::Acquire) == 0 && !tok.is_cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
        },
    );

    // Give the holding job a moment to claim the worker.
    thread::sleep(Duration::from_millis(20));

    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let order_a = Arc::clone(&order);
    let bg = sys.submit::<(), _>(
        JobKey::new(Scope::Global, "bg"),
        Priority::Background,
        Pool::Cpu,
        move |_| {
            order_a.lock().push("bg");
        },
    );
    let order_b = Arc::clone(&order);
    let fg = sys.submit::<(), _>(
        JobKey::new(Scope::Global, "fg"),
        Priority::Foreground,
        Pool::Cpu,
        move |_| {
            order_b.lock().push("fg");
        },
    );

    // Release the holder; queued jobs now run. The single worker
    // should take Foreground before Background.
    release.store(1, Ordering::Release);

    let _ = drain(&bg, Duration::from_secs(1));
    let _ = drain(&fg, Duration::from_secs(1));

    let seen = order.lock().clone();
    assert_eq!(seen, vec!["fg", "bg"], "higher priority must run first");
}

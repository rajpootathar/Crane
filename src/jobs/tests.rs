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

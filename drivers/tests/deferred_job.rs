use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use zcore_drivers::utils::deferred_job::{
    drain_deferred_jobs, drain_deferred_jobs_max, pending_deferred_jobs, push_deferred_job,
};

#[test]
fn push_and_drain() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    push_deferred_job(move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    assert!(pending_deferred_jobs() >= 1);
    drain_deferred_jobs();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn drain_max_limits_jobs() {
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..4 {
        let c = counter.clone();
        push_deferred_job(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }
    // drain_deferred_jobs_max(1) should run at most 1 job
    drain_deferred_jobs_max(1);
    let after_one = counter.load(Ordering::SeqCst);
    assert!(
        after_one <= 1,
        "expected at most 1 job drained, got {after_one}"
    );
    // drain remaining
    drain_deferred_jobs_max(8);
    drain_deferred_jobs_max(8);
}

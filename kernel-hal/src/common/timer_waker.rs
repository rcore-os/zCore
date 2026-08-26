//! Cancellable timer wake-ups for futures that block on I/O.
//!
//! [`timer_set`](crate::timer::timer_set) is fire-and-forget: it takes an owned
//! callback and hands back no handle, so a future that arms a fallback deadline
//! (poll/epoll/select backstops, TTY reads, socket waits) has no way to say
//! "never mind" when the I/O it was waiting for arrives first. Leaving those
//! callbacks to fire on their own is not harmless: each one owns a cloned
//! [`Waker`], so a task that completed and was freed still gets woken through a
//! stale waker minutes later — the use-after-free class of bug that
//! `docs/README-crash-repro.md` traces back to exactly this shape.
//!
//! A [`TimerWakerSlot`] is that missing handle. The armed callback and the slot
//! share one refcounted cell; cancelling flips a flag and **drops the waker**,
//! so when the timer eventually fires it finds nothing to do. The callback is
//! never unregistered from the timer heap — it simply becomes a no-op — which
//! keeps cancellation lock-free-ish and safe to call from `Drop`.
//!
//! Callers keep an `Option<TimerWakerSlot>` in their future and drive it with
//! [`ensure_timer_waker`] (arm, or refresh the waker if the same deadline is
//! already armed) and [`kill_timer_waker`] (cancel; idempotent). Every future
//! that arms one must also cancel it in its `Drop`.

use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Waker};
use core::time::Duration;

use lock::Mutex;

/// State shared between the armed timer callback and the owner's slot.
struct TimerWakerInner {
    /// Deadline this cell was armed for. Lets [`ensure_timer_waker`] recognise
    /// a re-poll that wants the same wake-up and refresh the waker in place
    /// instead of arming a second timer for the same instant.
    deadline: Duration,
    /// Set by whichever side gets there first: the owner cancelling, or the
    /// callback firing. One-way, so a fire and a cancel can race and exactly
    /// one of them wins.
    done: AtomicBool,
    /// The waker to fire. Taken on fire, dropped on cancel — never left behind
    /// pointing at a task that may already be gone.
    waker: Mutex<Option<Waker>>,
}

/// A live timer wake-up that its owner can cancel.
///
/// Dropping the slot alone does **not** cancel: use [`kill_timer_waker`], which
/// takes the slot and marks the shared cell done. (The distinction matters
/// because the callback holds its own reference to that cell — the slot going
/// away says nothing about the timer.)
pub struct TimerWakerSlot {
    inner: Arc<TimerWakerInner>,
}

impl TimerWakerSlot {
    /// The deadline this slot was armed for.
    pub fn deadline(&self) -> Duration {
        self.inner.deadline
    }

    /// Whether the timer has already fired or been cancelled.
    pub fn is_done(&self) -> bool {
        self.inner.done.load(Ordering::SeqCst)
    }
}

/// Ensure `slot` holds a live wake-up for `deadline`, registering `cx`'s waker.
///
/// - Slot already armed for this exact deadline and still live: the stored
///   waker is replaced with `cx`'s. A future may be polled by a different task
///   context than the one that armed the timer, so refreshing is what keeps the
///   wake-up pointed at whoever is actually waiting.
/// - Slot empty, already fired, or armed for a *different* deadline: the old
///   one is cancelled and a fresh timer is armed.
pub fn ensure_timer_waker(slot: &mut Option<TimerWakerSlot>, deadline: Duration, cx: &Context<'_>) {
    if let Some(existing) = slot.as_ref() {
        if !existing.is_done() && existing.inner.deadline == deadline {
            *existing.inner.waker.lock() = Some(cx.waker().clone());
            return;
        }
        kill_timer_waker(slot);
    }

    let inner = Arc::new(TimerWakerInner {
        deadline,
        done: AtomicBool::new(false),
        waker: Mutex::new(Some(cx.waker().clone())),
    });
    let fired = inner.clone();
    crate::timer::timer_set(
        deadline,
        Box::new(move |_now: Duration| {
            // Lost the race against a cancel (or a duplicate fire): the waker
            // is already gone and the owner is no longer interested.
            if fired.done.swap(true, Ordering::SeqCst) {
                return;
            }
            // Take the waker out from under the lock before waking: `wake`
            // runs scheduler code (and this callback runs in timer-IRQ
            // context), so holding this cell's lock across it would widen the
            // window for no reason.
            let waker = fired.waker.lock().take();
            if let Some(waker) = waker {
                waker.wake();
            }
        }),
    );
    *slot = Some(TimerWakerSlot { inner });
}

/// Cancel the wake-up in `slot`, if any, and empty the slot.
///
/// Idempotent and safe from `Drop`. After this returns, the armed callback (if
/// it has not run yet) is guaranteed to be a no-op and to hold no `Waker`, so
/// the task it referred to can be freed.
pub fn kill_timer_waker(slot: &mut Option<TimerWakerSlot>) {
    if let Some(slot) = slot.take() {
        // Mark done first: a callback firing concurrently on another CPU then
        // sees `done` already set and returns without touching the waker.
        slot.inner.done.store(true, Ordering::SeqCst);
        // Drop the waker even if the callback won the race and already took
        // it — `take` on `None` is fine, and this is what guarantees no stale
        // waker outlives the future.
        *slot.inner.waker.lock() = None;
    }
}

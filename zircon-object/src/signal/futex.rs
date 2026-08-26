use super::*;
use crate::{object::*, task::Thread};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::*;
use core::task::{Context, Poll, Waker};
use lock::Mutex;

/// A primitive for creating userspace synchronization tools.
///
/// ## SYNOPSIS
/// A **futex** is a Fast Userspace muTEX. It is a low level
/// synchronization primitive which is a building block for higher level
/// APIs such as `pthread_mutex_t` and `pthread_cond_t`.
/// Futexes are designed to not enter the kernel or allocate kernel
/// resources in the uncontested case.
pub struct Futex {
    base: KObjectBase,
    value: &'static AtomicI32,
    inner: Mutex<FutexInner>,
}

impl_kobject!(Futex);

#[derive(Default)]
struct FutexInner {
    waiter_queue: VecDeque<Arc<Waiter>>,
    /// NOTE: use `set_owner`
    owner: Option<Arc<Thread>>,
}

impl Futex {
    /// Create a new Futex.
    ///
    /// The parameter `value` is the reference to
    /// an userspace `AtomicI32`. This reference is the
    /// information used in kernel to track what futex given threads are
    /// waiting on. The kernel does not currently modify the value of
    /// `*value`. It is up to userspace code to correctly atomically modify this
    /// value across threads in order to build mutexes and so on.
    pub fn new(value: &'static AtomicI32) -> Arc<Self> {
        Arc::new(Futex {
            base: KObjectBase::default(),
            value,
            inner: Mutex::new(FutexInner::default()),
        })
    }

    /// Wait on a futex.
    ///
    /// This atomically verifies that `value_ptr` still contains the value `current_value`
    /// and sleeps until the futex is made available by a call to [`wake`].
    ///
    /// See [`wait_with_owner`] for advanced usage and more details.
    ///
    /// [`wait_with_owner`]: Futex::wait_with_owner
    /// [`wake`]: Futex::wake
    pub fn wait(self: &Arc<Self>, current_value: i32) -> impl Future<Output = ZxResult> {
        self.wait_with_owner(current_value, None, None)
    }

    /// Wake some number of threads waiting on a futex.
    ///
    /// It wakes at most `wake_count` of the waiters that are waiting on this futex.
    /// Return the number of waiters that were woken up.
    ///
    /// # Ownership
    ///
    /// The owner of the futex is set to nothing, regardless of the wake count.
    pub fn wake(&self, wake_count: usize) -> usize {
        // Drain up to `wake_count` waiters in a single critical section,
        // clear the owner in the same lock, then deliver wakeups after the
        // lock is released. `Waiter::wake` takes the waiter lock, so holding
        // futex.inner -> waiter.inner here while poll/Drop take
        // waiter.inner -> futex.inner would be a lock-order inversion
        // (deadlock under SMP). This collapses N lock acquires (one per
        // waker) into one for the common case.
        if wake_count == 0 {
            self.inner.lock().set_owner(None);
            return 0;
        }
        // Wake-one fast path (the canonical pthread_mutex_unlock case): one
        // lock acquire, zero allocations.
        if wake_count == 1 {
            let waiter = {
                let mut inner = self.inner.lock();
                let w = inner.waiter_queue.pop_front();
                inner.set_owner(None);
                w
            };
            return match waiter {
                Some(w) if w.wake() => 1,
                Some(_) => {
                    // Tombstoned (cancelled / timed out); try the next live waiter.
                    loop {
                        let w = self.inner.lock().waiter_queue.pop_front();
                        match w {
                            Some(w) if w.wake() => break 1,
                            Some(_) => continue,
                            None => break 0,
                        }
                    }
                }
                None => 0,
            };
        }
        // Wake-many (broadcast / condvar): drain the batch under one lock,
        // wake outside it. Trades N lock acquires for one Vec allocation.
        let batch: alloc::vec::Vec<Arc<Waiter>> = {
            let mut inner = self.inner.lock();
            let take = wake_count.min(inner.waiter_queue.len());
            let mut v = alloc::vec::Vec::with_capacity(take);
            for _ in 0..take {
                if let Some(w) = inner.waiter_queue.pop_front() {
                    v.push(w);
                }
            }
            inner.set_owner(None);
            v
        };
        let mut woken = 0;
        for waiter in batch {
            // Tombstoned waiters (timed-out / cancelled) return false and
            // must not consume the wake count.
            if waiter.wake() {
                woken += 1;
            }
        }
        // Tombstone top-up: rare path where some popped waiters were already
        // cancelled. Keep semantics: we must wake exactly up to `wake_count`
        // live waiters when available.
        while woken < wake_count {
            let waiter = self.inner.lock().waiter_queue.pop_front();
            match waiter {
                Some(waiter) => {
                    if waiter.wake() {
                        woken += 1;
                    }
                }
                None => break,
            }
        }
        woken
    }

    /// Fast comparison against the futex's current value, without taking the
    /// queue lock or allocating a waiter.
    ///
    /// Used by `FUTEX_WAIT` to short-circuit `EAGAIN` when userspace already
    /// lost the cmpxchg race — the canonical sysbench / pthread mutex hot
    /// path. A `false` result is authoritative; a `true` result must still
    /// be re-checked under the queue lock by the slow path.
    pub fn value_eq(&self, expected: i32) -> bool {
        // `Acquire` suffices: the producer (FUTEX_WAKE side) Release-stores
        // the new value in userspace before issuing the wake; we only need
        // happens-before with that store, not full SeqCst.
        self.value.load(Ordering::Acquire) == expected
    }

    // ------ Advanced APIs on Zircon ------

    /// Get the owner of the futex.
    pub fn owner(&self) -> Option<Arc<Thread>> {
        self.inner.lock().owner.clone()
    }

    /// Wait on a futex.
    ///
    /// This atomically verifies that `value_ptr` still contains the value `current_value`
    /// and sleeps until the futex is made available by a call to [`wake`].
    ///
    /// # SPURIOUS WAKEUPS
    ///
    /// This implementation currently does not generate spurious wakeups.
    ///
    /// # Ownership
    ///
    /// A successful call results in the owner of the futex being set to the
    /// thread referenced by the `new_owner`, or to nothing if it is `None`.
    ///
    /// # Errors
    ///
    /// - `INVALID_ARGS`: One of the following is true
    ///   - `new_owner` is currently a member of the waiters for this.
    ///   - `new_owner` has not been started yet.
    /// - `BAD_STATE`: `current_value` does not match the value at `value_ptr`.
    /// - `TIMED_OUT`: The thread was not woken before deadline passed.
    ///
    /// [`wake`]: Futex::wake
    pub fn wait_with_owner(
        self: &Arc<Self>,
        current_value: i32,
        thread: Option<Arc<Thread>>,
        new_owner: Option<Arc<Thread>>,
    ) -> impl Future<Output = ZxResult> {
        #[must_use = "wait does nothing unless polled/`await`-ed"]
        struct FutexFuture {
            waiter: Arc<Waiter>,
            current_value: i32,
            new_owner: Option<Arc<Thread>>,
        }
        impl Future for FutexFuture {
            type Output = ZxResult;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut inner = self.waiter.inner.lock();
                // check wakeup
                if inner.woken {
                    // set new owner on success
                    inner.futex.inner.lock().set_owner(self.new_owner.clone());
                    return Poll::Ready(Ok(()));
                }
                // first time?
                if inner.waker.is_none() {
                    let futex = inner.futex.clone();
                    let mut futex_inner = futex.inner.lock();
                    // Check the value while holding the futex queue lock: a
                    // concurrent FUTEX_WAKE pops waiters under the same lock,
                    // so the check-and-enqueue is atomic with respect to it.
                    // Checking the value before taking the lock allowed a wake
                    // to slip in between the check and the enqueue, leaving
                    // the waiter asleep forever (lost wakeup), which hung
                    // pthread barriers/condvars (e.g. sysbench startup).
                    let value = futex.value.load(Ordering::SeqCst);
                    if value != self.current_value {
                        return Poll::Ready(Err(ZxError::BAD_STATE));
                    }
                    // check new owner
                    if !futex_inner.is_valid_new_owner(&self.new_owner) {
                        return Poll::Ready(Err(ZxError::INVALID_ARGS));
                    }
                    inner.waker.replace(cx.waker().clone());
                    futex_inner.waiter_queue.push_back(self.waiter.clone());
                }
                Poll::Pending
            }
        }
        // The FutexFuture will be dropped when the thread is no longer waiting
        // if we wake without be woken, remove myself from the waiter_queue
        impl Drop for FutexFuture {
            fn drop(&mut self) {
                let mut inner = self.waiter.inner.lock();
                if !inner.woken {
                    // Tombstone the waiter: if a concurrent wake/requeue
                    // already popped it from the queue (so the search below
                    // misses), its wake() must report "already consumed".
                    inner.woken = true;
                    let futex = inner.futex.clone();
                    let queue = &mut futex.inner.lock().waiter_queue;
                    if let Some(pos) = queue.iter().position(|x| Arc::ptr_eq(x, &self.waiter)) {
                        // Nobody cares about the order of queue, so just remove faster
                        queue.swap_remove_back(pos);
                    }
                }
            }
        }
        FutexFuture {
            waiter: Arc::new(Waiter {
                thread,
                inner: Mutex::new(WaiterInner {
                    waker: None,
                    woken: false,
                    futex: self.clone(),
                }),
            }),
            current_value,
            new_owner,
        }
    }

    /// Wake exactly one thread from the futex wait queue.
    ///
    /// If there is at least one thread to wake, the owner of the futex will
    /// be set to the thread which was woken. Otherwise, the futex will have
    /// no owner.
    ///
    /// # Ownership
    ///
    /// If there is at least one thread to wake, the owner of the futex will be
    /// set to the thread which was woken. Otherwise, the futex will have no owner.
    pub fn wake_single_owner(&self) {
        // Pop the waiter and set the new owner under one lock acquire
        // (pre-compute the next owner from the popped waiter's thread,
        // independent of whether the wakeup itself raced with cancellation).
        // The actual wake happens after the lock is released to preserve the
        // futex.inner -> waiter.inner ordering ban.
        let waiter = {
            let mut inner = self.inner.lock();
            let w = inner.waiter_queue.pop_front();
            let new_owner = w.as_ref().and_then(|w| w.thread.clone());
            inner.set_owner(new_owner);
            w
        };
        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }

    /// Requeuing is a generalization of waking.
    ///
    /// First, verifies that the value in `current_value` matches the value of the futex,
    /// and if not reports `ZxError::BAD_STATE`. After waking `wake_count` threads,
    /// `requeue_count` threads are moved from the original futex's wait queue to the
    /// wait queue corresponding to another `requeue_futex`.
    ///
    /// This requeueing behavior may be used to avoid thundering herds on wake.
    ///
    /// # Ownership
    ///
    /// The owner of this futex is set to nothing, regardless of the wake count.
    /// The owner of the `requeue_futex` is set to the thread `new_requeue_owner`.
    pub fn requeue(
        &self,
        current_value: i32,
        wake_count: usize,
        requeue_count: usize,
        requeue_futex: &Arc<Futex>,
        new_requeue_owner: Option<Arc<Thread>>,
        check_value: bool,
    ) -> ZxResult {
        let mut to_wake = alloc::vec::Vec::new();
        let mut to_requeue = alloc::vec::Vec::new();
        {
            // Hold BOTH queue locks while moving waiters: if a waiter is
            // popped from this queue but not yet visible on the target one,
            // a concurrent FUTEX_WAKE on the target (e.g. a mutex unlock
            // racing musl's condvar unlock_requeue) finds an empty queue and
            // the wakeup is lost — threads then stall until a timeout.
            // Lock in address order so two concurrent requeues with swapped
            // futexes cannot deadlock (ABBA).
            let this = self as *const Futex;
            let that = Arc::as_ptr(requeue_futex);
            if this == that {
                // Requeueing a futex onto itself is meaningless and would
                // self-deadlock below; treat it as a plain wake.
                drop(to_wake);
                let woken = self.wake(wake_count);
                let _ = woken;
                return Ok(());
            }
            let (mut inner, mut new_inner);
            if (this as usize) <= (that as usize) {
                inner = self.inner.lock();
                new_inner = requeue_futex.inner.lock();
            } else {
                new_inner = requeue_futex.inner.lock();
                inner = self.inner.lock();
            }
            if check_value {
                // check value (under the queue lock, like FUTEX_WAIT does)
                if self.value.load(Ordering::SeqCst) != current_value {
                    return Err(ZxError::BAD_STATE);
                }
            }
            for _ in 0..wake_count {
                if let Some(waiter) = inner.waiter_queue.pop_front() {
                    to_wake.push(waiter);
                } else {
                    break;
                }
            }
            let requeue_count = requeue_count.min(inner.waiter_queue.len());
            for waiter in inner.waiter_queue.drain(..requeue_count) {
                new_inner.waiter_queue.push_back(waiter.clone());
                to_requeue.push(waiter);
            }
            inner.set_owner(None);
            new_inner.set_owner(new_requeue_owner);
        }
        // Retarget waiters after releasing the queue locks (`reset_futex`
        // takes the waiter lock; taking it under futex.inner would invert
        // the poll/Drop lock order). A waiter cancelled in this window
        // searches its old queue, misses, and stays tombstoned on the new
        // queue, where wake() skips it without consuming a count.
        for waiter in to_requeue {
            waiter.reset_futex(requeue_futex.clone());
        }
        // Deliver wakeups last, with no futex lock held.
        for waiter in to_wake {
            waiter.wake();
        }
        Ok(())
    }
}

impl FutexInner {
    fn is_valid_new_owner(&self, new_owner: &Option<Arc<Thread>>) -> bool {
        // TODO: check whether the thread has been started yet
        if let Some(new_owner) = &new_owner {
            if self
                .waiter_queue
                .iter()
                .filter_map(|waiter| waiter.thread.as_ref())
                .any(|thread| Arc::ptr_eq(thread, new_owner))
            {
                return false;
            }
        }
        true
    }

    fn set_owner(&mut self, owner: Option<Arc<Thread>>) {
        // TODO: change the priority of owner thread
        self.owner = owner;
    }
}

struct Waiter {
    /// The thread waiting on the futex.
    thread: Option<Arc<Thread>>,
    inner: Mutex<WaiterInner>,
}

struct WaiterInner {
    /// The waker of waiting future. `None` indicates first poll.
    waker: Option<Waker>,
    woken: bool,
    futex: Arc<Futex>,
}

impl Waiter {
    /// Wake up the waiting thread.
    ///
    /// Returns `false` if the waiter was already woken or cancelled (its
    /// future dropped, e.g. on timeout), in which case it must not consume
    /// a wake count. The waker is invoked after releasing the waiter lock.
    fn wake(&self) -> bool {
        let waker = {
            let mut inner = self.inner.lock();
            if inner.woken {
                return false;
            }
            inner.woken = true;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
            true
        } else {
            false
        }
    }

    /// Reset futex on requeue.
    fn reset_futex(&self, futex: Arc<Futex>) {
        self.inner.lock().futex = futex;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Job, Process};
    use core::time::Duration;

    #[async_std::test]
    async fn wait() {
        static VALUE: AtomicI32 = AtomicI32::new(1);
        let futex = Futex::new(&VALUE);

        let count = futex.wake(1);
        assert_eq!(count, 0);

        // inconsistent value should fail.
        assert_eq!(futex.wait(0).await, Err(ZxError::BAD_STATE));

        // spawn a new task to wake me up.
        {
            let futex = futex.clone();
            async_std::task::spawn(async move {
                async_std::task::sleep(Duration::from_millis(10)).await;
                VALUE.store(2, Ordering::SeqCst);
                let count = futex.wake(1);
                assert_eq!(count, 1);
            });
        }
        // wait for wake.
        futex.wait(1).await.unwrap();
        assert_eq!(VALUE.load(Ordering::SeqCst), 2);
        assert_eq!(futex.wake(1), 0);
    }

    #[async_std::test]
    async fn requeue() {
        static VALUE: AtomicI32 = AtomicI32::new(1);
        let futex = Futex::new(&VALUE);
        static REQUEUE_VALUE: AtomicI32 = AtomicI32::new(100);
        let requeue_futex = Futex::new(&REQUEUE_VALUE);

        let count = futex.wake(1);
        assert_eq!(count, 0);

        // inconsistent value should fail.
        assert_eq!(futex.wait(0).await, Err(ZxError::BAD_STATE));

        // spawn a new task to wait
        {
            let futex = futex.clone();
            async_std::task::spawn(async move {
                futex.wait(1).await.unwrap();
            });
        }
        // spawn a new task to requeue.
        {
            let futex = futex.clone();
            async_std::task::spawn(async move {
                async_std::task::sleep(Duration::from_millis(10)).await;
                VALUE.store(2, Ordering::SeqCst);

                let waiters = futex.inner.lock().waiter_queue.clone();
                assert_eq!(waiters.len(), 2);

                // inconsistent value should fail.
                assert_eq!(
                    futex.requeue(1, 1, 1, &requeue_futex, None, true),
                    Err(ZxError::BAD_STATE)
                );
                assert!(futex.requeue(2, 1, 1, &requeue_futex, None, true).is_ok());
                // 1 waiter waken, 1 waiter moved into `requeue_futex`.
                assert_eq!(futex.inner.lock().waiter_queue.len(), 0);
                assert_eq!(requeue_futex.inner.lock().waiter_queue.len(), 1);
                assert!(Arc::ptr_eq(
                    &requeue_futex.inner.lock().waiter_queue[0],
                    &waiters[1]
                ));
                // wake the requeued waiter.
                assert_eq!(requeue_futex.wake(1), 1);
            });
        }
        // wait for wake.
        futex.wait(1).await.unwrap();
        assert_eq!(VALUE.load(Ordering::SeqCst), 2);
    }

    #[async_std::test]
    async fn owner() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        static VALUE: AtomicI32 = AtomicI32::new(1);
        let futex = proc.get_futex(&VALUE);
        assert!(futex.owner().is_none());
        futex.inner.lock().set_owner(Some(thread.clone()));

        {
            let futex = futex.clone();
            let thread = thread.clone();
            async_std::task::spawn(async move {
                futex
                    .wait_with_owner(1, Some(thread.clone()), Some(thread))
                    .await
                    .unwrap();
            });
        }
        async_std::task::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            futex
                .wait_with_owner(1, Some(thread.clone()), Some(thread.clone()))
                .await
                .unwrap_err(),
            ZxError::INVALID_ARGS
        );

        futex.inner.lock().set_owner(None);
        futex.wake_single_owner();
        assert!(Arc::ptr_eq(&futex.owner().unwrap(), &thread));
        assert_eq!(futex.wake(1), 0);
    }
}

use super::*;
use crate::{object::*, task::Thread};
use alloc::collections::VecDeque;
use alloc::{boxed::Box, sync::Arc};
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::*;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use kernel_hal::timer_now;
use spin::Mutex;

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
    pub fn wait(self: &Arc<Self>, current_value: i32) -> impl Future<Output = ZxResult<()>> {
        self.wait_with_owner(current_value, None, None, None)
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
        let mut inner = self.inner.lock();
        inner.set_owner(None);
        for i in 0..wake_count {
            if let Some(waiter) = inner.waiter_queue.pop_front() {
                waiter.wake();
            } else {
                return i + 1;
            }
        }
        wake_count
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
        deadline: Option<Duration>,
    ) -> impl Future<Output = ZxResult<()>> {
        #[must_use = "wait does nothing unless polled/`await`-ed"]
        struct FutexFuture {
            waiter: Arc<Waiter>,
            current_value: i32,
            new_owner: Option<Arc<Thread>>,
            deadline: Option<Duration>,
        }
        impl Future for FutexFuture {
            type Output = ZxResult<()>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                // check timeout
                if let Some(deadline) = self.deadline {
                    if timer_now() >= deadline {
                        return Poll::Ready(Err(ZxError::TIMED_OUT));
                    }
                }
                let mut inner = self.waiter.inner.lock();
                // check wakeup
                if inner.woken {
                    // set new owner on success
                    inner.futex.inner.lock().set_owner(self.new_owner.clone());
                    return Poll::Ready(Ok(()));
                }
                // first time?
                if inner.waker.is_none() {
                    // check value
                    let value = inner.futex.value.load(Ordering::SeqCst);
                    if value != self.current_value {
                        return Poll::Ready(Err(ZxError::BAD_STATE));
                    }
                    // check new owner
                    let mut futex = inner.futex.inner.lock();
                    if !futex.is_valid_new_owner(&self.new_owner) {
                        return Poll::Ready(Err(ZxError::INVALID_ARGS));
                    }
                    futex.waiter_queue.push_back(self.waiter.clone());
                    drop(futex);
                    // set timer
                    if let Some(deadline) = self.deadline {
                        self.waiter.set_timer(deadline);
                    }
                    inner.waker.replace(cx.waker().clone());
                }
                Poll::Pending
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
            deadline,
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
        let mut inner = self.inner.lock();
        let new_owner = inner.waiter_queue.pop_front().and_then(|waiter| {
            waiter.wake();
            waiter.thread.clone()
        });
        inner.set_owner(new_owner);
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
    ) -> ZxResult<()> {
        let mut inner = self.inner.lock();
        // check value
        if self.value.load(Ordering::SeqCst) != current_value {
            return Err(ZxError::BAD_STATE);
        }
        // wake
        for _ in 0..wake_count {
            if let Some(waiter) = inner.waiter_queue.pop_front() {
                waiter.wake();
            } else {
                break;
            }
        }
        // requeue
        let mut new_inner = requeue_futex.inner.lock();
        for waiter in inner.waiter_queue.drain(..requeue_count) {
            waiter.reset_futex(requeue_futex.clone());
            new_inner.waiter_queue.push_back(waiter);
        }
        // set owner
        inner.set_owner(None);
        new_inner.set_owner(new_requeue_owner);
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
                .any(|thread| Arc::ptr_eq(&thread, new_owner))
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
    fn wake(&self) {
        let mut inner = self.inner.lock();
        inner.woken = true;
        inner.waker.as_ref().unwrap().wake_by_ref();
    }

    /// Reset futex on requeue.
    fn reset_futex(&self, futex: Arc<Futex>) {
        self.inner.lock().futex = futex;
    }

    fn set_timer(self: &Arc<Self>, deadline: Duration) {
        let me = Arc::downgrade(self);
        kernel_hal::timer_set(
            deadline,
            Box::new(move |_now| {
                if let Some(me) = me.upgrade() {
                    let inner = me.inner.lock();
                    inner.waker.as_ref().unwrap().wake_by_ref();
                }
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn wait_async() {
        static VALUE: AtomicI32 = AtomicI32::new(1);
        let futex = Futex::new(&VALUE);

        // inconsistent value should fail.
        assert_eq!(futex.wait(0).await, Err(ZxError::BAD_STATE));

        // spawn a new task to wake me up.
        {
            let futex = futex.clone();
            async_std::task::spawn(async move {
                VALUE.store(2, Ordering::SeqCst);
                let count = futex.wake(1);
                assert_eq!(count, 1);
            });
        }
        // wait for wake.
        futex.wait(1).await.unwrap();
        assert_eq!(VALUE.load(Ordering::SeqCst), 2);
    }
}

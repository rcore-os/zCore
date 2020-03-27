use super::*;
use crate::{object::*, task::Thread};
use alloc::collections::VecDeque;
use alloc::{boxed::Box, sync::Arc};
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::*;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use spin::Mutex;

struct Waiter {
    thread: Arc<Thread>,
    timer: Arc<Timer>,
    inner: Mutex<WaiterInner>,
}

struct WaiterInner {
    waker: Option<Waker>,
    woken: bool,
    futex: Arc<Futex>,
    time_out: bool,
}

impl Waiter {
    pub fn set_woken(&self) {
        self.inner.lock().woken = true;
    }

    pub fn set_futex(&self, futex: Arc<Futex>) {
        self.inner.lock().futex = futex;
    }

    pub fn set_timer(self: &Arc<Waiter>, deadline: Duration, slack: Duration) {
        self.timer.set(deadline, slack);
        let weak_self = Arc::downgrade(self);
        self.timer.add_signal_callback(Box::new(move |s| {
            if let Some(real_self) = weak_self.upgrade() {
                if (s & Signal::SIGNALED).is_empty() {
                    panic!("fault signal when timer come!");
                } else {
                    real_self.set_time_out();
                    real_self.inner.lock().waker.as_ref().unwrap().wake_by_ref();
                    true
                }
            } else {
                true
            }
        }));
    }

    pub fn set_time_out(&self) {
        self.inner.lock().time_out = true;
    }
}

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

    /// Wait on a futex asynchronously.
    ///
    /// This atomically verifies that `value_ptr` still contains the value `current_value`
    /// and sleeps until the futex is made available by a call to [`wake`].
    ///
    /// # SPURIOUS WAKEUPS
    ///
    /// This implementation currently does not generate spurious wakeups.
    ///
    /// [`wake`]: Futex::wake
    pub fn wait_async(
        self: &Arc<Self>,
        current_value: i32,
        thread: Arc<Thread>,
        deadline: u64,
    ) -> impl Future<Output = ZxResult<()>> {
        let waiter = Arc::new(Waiter {
            thread,
            timer: Timer::new(),
            inner: Mutex::new(WaiterInner {
                waker: None,
                woken: false,
                time_out: false,
                futex: self.clone(),
            }),
        });
        if deadline != 0x7fff_ffff_ffff_ffff {
            waiter.set_timer(Duration::from_nanos(deadline), Duration::from_nanos(0));
        }
        self.inner.lock().waiter_queue.push_back(waiter.clone());
        struct FutexFuture {
            waiter: Arc<Waiter>,
            current_value: i32,
        }
        impl Future for FutexFuture {
            type Output = ZxResult<()>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut inner = self.waiter.inner.lock();
                if self.waiter.timer.signal().contains(Signal::SIGNALED) {
                    Poll::Ready(Err(ZxError::TIMED_OUT))
                } else if inner.woken {
                    Poll::Ready(Ok(()))
                } else {
                    // if waiting, check wake num
                    let value = inner.futex.value.load(Ordering::SeqCst);
                    if value != self.current_value {
                        Poll::Ready(Err(ZxError::BAD_STATE))
                    } else {
                        inner.waker.replace(cx.waker().clone());
                        drop(inner);
                        Poll::Pending
                    }
                }
            }
        }

        FutexFuture {
            waiter,
            current_value,
        }
    }

    /// Wake some number of threads waiting on a futex.
    ///
    /// It wakes at most `wake_count` of the waiters that are waiting on this futex.
    /// Return the number of waiters that were woken up.
    pub fn wake(&self, wake_count: usize) -> usize {
        let mut inner = self.inner.lock();
        for i in 0..wake_count {
            if let Some(waiter) = inner.waiter_queue.pop_front() {
                waiter.set_woken();
                let waker = waiter.inner.lock().waker.as_ref().unwrap().clone();
                waker.wake();
            } else {
                return i + 1;
            }
        }
        wake_count
    }

    pub fn wake_single_owner(&self) {
        let mut inner = self.inner.lock();
        let new_owner = match inner.waiter_queue.pop_front() {
            Some(waiter) => {
                waiter.set_woken();
                let waker = waiter.inner.lock().waker.as_ref().unwrap().clone();
                waker.wake();
                Some(waiter.thread.clone())
            }
            None => None,
        };
        inner.owner = new_owner;
    }

    pub fn get_owner(&self) -> KoID {
        match self.inner.lock().owner.as_ref() {
            Some(ptr) => ptr.id(),
            None => 0,
        }
    }

    // TODO: for a thread, to be owner of one futex means change on priority
    // see fuchsia/docs/reference/kernel_objects/futex.md#Ownership and Priority Inheritance
    pub fn set_owner(&self, owner: Option<Arc<Thread>>) -> ZxResult<()> {
        self.check_thread(owner.as_ref())?;
        let mut inner = self.inner.lock();
        inner.owner = owner;
        Ok(())
    }

    /// Check if `thread` can be owner of this futex
    fn check_thread(&self, to_check: Option<&Arc<Thread>>) -> ZxResult<()> {
        // TODO: to check whether the `to_check` thread has been started yet
        let inner = self.inner.lock();
        match to_check {
            Some(to_check) => {
                if inner
                    .waiter_queue
                    .iter()
                    .any(|waiter| Arc::ptr_eq(&waiter.thread, to_check))
                {
                    Err(ZxError::INVALID_ARGS)
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }

    pub fn wake_and_requeue(
        &self,
        wake_count: usize,
        requeue_futex: Arc<Futex>,
        requeue_count: usize,
    ) -> ZxResult<()> {
        self.wake(wake_count);
        self.set_owner(None)?;
        let mut inner = self.inner.lock();
        let current_len = inner.waiter_queue.len();
        let mut waiter_list = VecDeque::new();
        for _ in 0..requeue_count.min(current_len) {
            let waiter = inner.waiter_queue.pop_front().unwrap();
            waiter.set_futex(requeue_futex.clone());
            waiter_list.push_back(waiter);
        }
        requeue_futex.push_waiters(waiter_list);
        Ok(())
    }

    fn push_waiters(&self, mut waiters: VecDeque<Arc<Waiter>>) {
        let mut inner = self.inner.lock();
        inner.waiter_queue.append(&mut waiters);
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
        assert_eq!(futex.wait_async(0).await, Err(ZxError::BAD_STATE));

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
        futex.wait_async(1).await.unwrap();
        assert_eq!(VALUE.load(Ordering::SeqCst), 2);
    }
}

use super::*;
use crate::object::*;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::*;
use core::task::{Context, Poll, Waker};
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
    waiter_queue: VecDeque<Waker>,
    wake_num: usize,
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
    pub fn wait_async(self: &Arc<Self>, current_value: i32) -> impl Future<Output = ZxResult<()>> {
        struct FutexFuture {
            futex: Arc<Futex>,
            current_value: i32,
            num: Option<usize>,
        }
        impl Future for FutexFuture {
            type Output = ZxResult<()>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut inner = self.futex.inner.lock();
                // if waiting, check wake num
                if let Some(num) = self.num {
                    return if inner.wake_num > num {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    };
                }
                // first call, check value
                let value = self.futex.value.load(Ordering::SeqCst);
                if value != self.current_value {
                    return Poll::Ready(Err(ZxError::BAD_STATE));
                }
                // push to wait queue
                let wait_num = inner.wake_num + inner.waiter_queue.len();
                inner.waiter_queue.push_back(cx.waker().clone());
                drop(inner);
                self.num = Some(wait_num);
                Poll::Pending
            }
        }

        FutexFuture {
            futex: self.clone(),
            current_value,
            num: None,
        }
    }

    /// Wake some number of threads waiting on a futex.
    ///
    /// It wakes at most `wake_count` of the waiters that are waiting on this futex.
    /// Return the number of waiters that were woken up.
    pub fn wake(&self, wake_count: usize) -> usize {
        let mut inner = self.inner.lock();
        for i in 0..wake_count {
            if let Some(waker) = inner.waiter_queue.pop_front() {
                waker.wake();
                inner.wake_num += 1;
            } else {
                return i + 1;
            }
        }
        wake_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn wait_async() {
        static VALUE: AtomicI32 = AtomicI32::new(1);
        let futex = Futex::new(&VALUE);

        for _ in 0..4 {
            let futex = futex.clone();
            async_std::task::spawn(async move {
                futex.wait_async(1).await.unwrap();
            });
        }
        let count = futex.wake(2);
        assert_eq!(count, 2);
    }
}

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
use core::time::Duration;

/// Yields execution back to the async runtime.
pub fn yield_now() -> impl Future<Output = ()> {
    YieldFuture::default()
}

#[must_use = "yield_now does nothing unless polled/`await`-ed"]
#[derive(Default)]
struct YieldFuture {
    flag: bool,
}

impl Future for YieldFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if self.flag {
            Poll::Ready(())
        } else {
            self.flag = true;
            cx.waker().clone().wake();
            Poll::Pending
        }
    }
}

/// Sleeps until the specified of time.
pub fn sleep_until(deadline: Duration) -> impl Future {
    SleepFuture {
        woken: Arc::new(Default::default()),
        deadline: Some(deadline),
    }
}

#[must_use = "sleep does nothing unless polled/`await`-ed"]
pub struct SleepFuture {
    woken: Arc<AtomicBool>,
    deadline: Option<Duration>,
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if self.woken.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if let Some(deadline) = self.deadline.take() {
            let woken = Arc::downgrade(&self.woken);
            let waker = cx.waker().clone();
            crate::timer_set(
                deadline,
                Box::new(move |_| {
                    if let Some(woken) = woken.upgrade() {
                        woken.store(true, Ordering::Release);
                        waker.wake();
                    }
                }),
            );
        }
        Poll::Pending
    }
}

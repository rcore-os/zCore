use crate::timer_now;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
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
        deadline,
        set: false,
    }
}

#[must_use = "sleep does nothing unless polled/`await`-ed"]
pub struct SleepFuture {
    deadline: Duration,
    set: bool,
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if timer_now() >= self.deadline {
            return Poll::Ready(());
        }
        if !self.set && self.deadline.as_nanos() < i64::max_value() as u128 {
            let waker = cx.waker().clone();
            crate::timer_set(self.deadline, Box::new(move |_| waker.wake()));
            self.set = true;
        }
        Poll::Pending
    }
}

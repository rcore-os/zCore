use super::timer_now;
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
    SleepFuture { deadline }
}

#[must_use = "sleep does nothing unless polled/`await`-ed"]
pub struct SleepFuture {
    deadline: Duration,
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if timer_now() >= self.deadline {
            return Poll::Ready(());
        }
        if self.deadline.as_nanos() < i64::max_value() as u128 {
            let waker = cx.waker().clone();
            super::timer_set(self.deadline, Box::new(move |_| waker.wake()));
        }
        Poll::Pending
    }
}

/// Get a char from serial.
pub fn serial_getchar() -> impl Future<Output = u8> {
    SerialFuture
}

#[must_use = "serial_getchar does nothing unless polled/`await`-ed"]
pub struct SerialFuture;

impl Future for SerialFuture {
    type Output = u8;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut buf = [0u8];
        if super::serial_read(&mut buf) != 0 {
            return Poll::Ready(buf[0]);
        }
        let waker = cx.waker().clone();
        super::serial_set_callback(Box::new(move || waker.wake()));
        Poll::Pending
    }
}

use alloc::boxed::Box;
use core::task::{Context, Poll};
use core::time::Duration;
use core::{future::Future, pin::Pin};

#[must_use = "`yield_now()` does nothing unless polled/`await`-ed"]
#[derive(Default)]
pub(super) struct YieldFuture {
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

#[must_use = "`sleep_until()` does nothing unless polled/`await`-ed"]
pub(super) struct SleepFuture {
    deadline: Duration,
}

impl SleepFuture {
    pub fn new(deadline: Duration) -> Self {
        Self { deadline }
    }
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if crate::timer::timer_now() >= self.deadline {
            return Poll::Ready(());
        }
        if self.deadline.as_nanos() < i64::max_value() as u128 {
            let waker = cx.waker().clone();
            crate::timer::timer_set(self.deadline, Box::new(move |_| waker.wake()));
        }
        Poll::Pending
    }
}

#[must_use = "`console_read()` does nothing unless polled/`await`-ed"]
pub(super) struct SerialReadFuture<'a> {
    buf: &'a mut [u8],
}

impl<'a> SerialReadFuture<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf }
    }
}

impl Future for SerialReadFuture<'_> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let uart = if let Some(uart) = crate::drivers::uart::first() {
            uart
        } else {
            return Poll::Pending;
        };
        let buf = &mut self.get_mut().buf;
        let mut n = 0;
        for i in 0..buf.len() {
            if let Some(c) = uart.try_recv().unwrap_or(None) {
                buf[i] = c;
                n += 1;
            } else {
                break;
            }
        }
        if n > 0 {
            return Poll::Ready(n);
        }
        let waker = cx.waker().clone();
        uart.subscribe(Box::new(move || waker.wake_by_ref()), true);
        Poll::Pending
    }
}

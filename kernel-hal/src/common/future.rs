use alloc::{boxed::Box, sync::Arc};
use core::task::{Context, Poll};
use core::time::Duration;
use core::{future::Future, pin::Pin};
use zcore_drivers::scheme::DisplayScheme;

use crate::timer;

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
        if timer::timer_now() >= self.deadline {
            return Poll::Ready(());
        }
        if self.deadline.as_nanos() < i64::max_value() as u128 {
            let waker = cx.waker().clone();
            timer::timer_set(self.deadline, Box::new(move |_| waker.wake()));
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
        let uart = if let Some(uart) = crate::drivers::all_uart().first() {
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
        uart.subscribe(Box::new(move |_| waker.wake_by_ref()), true);
        Poll::Pending
    }
}

pub(crate) struct DisplayFlushFuture {
    next_flush_time: Duration,
    frame_time: Duration,
    display: Arc<dyn DisplayScheme>,
}

impl DisplayFlushFuture {
    #[allow(dead_code)]
    pub fn new(display: Arc<dyn DisplayScheme>, refresh_rate: usize) -> Self {
        Self {
            next_flush_time: Duration::default(),
            frame_time: Duration::from_millis(1000 / refresh_rate as u64),
            display,
        }
    }
}

impl Future for DisplayFlushFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let now = timer::timer_now();
        if now >= self.next_flush_time {
            self.display.flush().ok();
            let frame_time = self.frame_time;
            self.next_flush_time += frame_time;
            let waker = cx.waker().clone();
            timer::timer_set(self.next_flush_time, Box::new(move |_| waker.wake()));
        }
        Poll::Pending
    }
}

use alloc::{boxed::Box, sync::Arc};
use core::task::{Context, Poll};
use core::time::Duration;
use core::{future::Future, pin::Pin};
use zcore_drivers::scheme::DisplayScheme;

use crate::timer;
use crate::timer_waker::{self, TimerWakerSlot};

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
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[must_use = "`sleep_until()` does nothing unless polled/`await`-ed"]
pub(super) struct SleepFuture {
    deadline: Duration,
    /// Waker slot shared with the armed timer callback. Armed once; re-polls
    /// refresh in place via [`timer_waker::ensure_timer_waker`].
    slot: Option<TimerWakerSlot>,
}

impl SleepFuture {
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            slot: None,
        }
    }
}

impl Drop for SleepFuture {
    fn drop(&mut self) {
        // Cancel so a late tick cannot wake a finished/reused task.
        timer_waker::kill_timer_waker(&mut self.slot);
    }
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.get_mut();
        if timer::timer_now() >= this.deadline {
            timer_waker::kill_timer_waker(&mut this.slot);
            return Poll::Ready(());
        }
        if this.deadline.as_nanos() >= i64::MAX as u128 {
            // "Never": the caller relies on some other wake source.
            return Poll::Pending;
        }
        let deadline = this.deadline;
        timer_waker::ensure_timer_waker(&mut this.slot, deadline, cx);
        Poll::Pending
    }
}

#[must_use = "`console_read()` does nothing unless polled/`await`-ed"]
pub(super) struct SerialReadFuture<'a> {
    buf: &'a mut [u8],
    sub_id: Option<u64>,
}

impl<'a> SerialReadFuture<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, sub_id: None }
    }
}

impl Drop for SerialReadFuture<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.sub_id.take() {
            if let Some(uart) = crate::drivers::all_uart().first() {
                uart.unsubscribe(id);
            }
        }
    }
}

impl Future for SerialReadFuture<'_> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.get_mut();
        let uart = if let Some(uart) = crate::drivers::all_uart().first() {
            uart
        } else {
            return Poll::Pending;
        };
        let buf = &mut this.buf;
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
            if let Some(id) = this.sub_id.take() {
                uart.unsubscribe(id);
            }
            return Poll::Ready(n);
        }
        if this.sub_id.is_none() {
            let waker = cx.waker().clone();
            this.sub_id = uart.subscribe(Box::new(move |_| waker.wake_by_ref()), true);
        }
        Poll::Pending
    }
}

pub(crate) struct DisplayFlushFuture {
    next_flush_time: Duration,
    frame_time: Duration,
    display: Arc<dyn DisplayScheme>,
    slot: Option<TimerWakerSlot>,
}

impl DisplayFlushFuture {
    #[allow(dead_code)]
    pub fn new(display: Arc<dyn DisplayScheme>, refresh_rate: usize) -> Self {
        Self {
            next_flush_time: Duration::default(),
            frame_time: Duration::from_millis(1000 / refresh_rate as u64),
            display,
            slot: None,
        }
    }
}

impl Drop for DisplayFlushFuture {
    fn drop(&mut self) {
        timer_waker::kill_timer_waker(&mut self.slot);
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
            let deadline = self.next_flush_time;
            // Re-arm each frame; kill_timer_waker first so a late previous
            // tick cannot wake after we replace the slot.
            timer_waker::kill_timer_waker(&mut self.slot);
            timer_waker::ensure_timer_waker(&mut self.slot, deadline, cx);
        }
        Poll::Pending
    }
}

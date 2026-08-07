//! Futures for I/O waits (NIC, HID, TTY).

use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use super::{register_io_wait_wakers, retain_io_wait_wakers};

/// Fallback timer when no IRQ wakes a multiplex wait (poll/epoll/select).
pub const IO_WAIT_TICK_MS: u64 = 4;

/// Resolves when Ctrl+C is pending, NET RX wakers fire, or deadline.
pub struct NetOrTtyWait {
    deadline: Duration,
    armed: bool,
}

impl NetOrTtyWait {
    pub fn new_after_ms(ms: u64) -> Self {
        Self {
            deadline: kernel_hal::timer::timer_now() + Duration::from_millis(ms),
            armed: false,
        }
    }
}

impl Future for NetOrTtyWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if crate::fs::stdio::ctrl_c_pending_peek() {
            return Poll::Ready(());
        }
        if kernel_hal::timer::timer_now() >= self.deadline {
            return Poll::Ready(());
        }
        if self.armed {
            retain_io_wait_wakers(cx.waker(), true, true);
            return Poll::Ready(());
        }
        register_io_wait_wakers(cx.waker(), true, true);
        let waker = cx.waker().clone();
        let dl = self.deadline;
        kernel_hal::timer::timer_set(dl, Box::new(move |_| waker.wake_by_ref()));
        self.armed = true;
        Poll::Pending
    }
}

/// One sleep cycle in epoll/poll: wake on NET/TTY IRQ or fallback timer.
pub struct IoMultiplexWait {
    deadline: Option<Duration>,
    watch_net: bool,
    watch_hid: bool,
    armed: bool,
}

impl IoMultiplexWait {
    pub fn new(timeout_msecs: isize, watch_net: bool, watch_hid: bool) -> Self {
        let deadline = if timeout_msecs >= 0 {
            Some(kernel_hal::timer::timer_now() + Duration::from_millis(timeout_msecs as u64))
        } else {
            None
        };
        Self {
            deadline,
            watch_net,
            watch_hid,
            armed: false,
        }
    }
}

impl Future for IoMultiplexWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(dl) = self.deadline {
            if kernel_hal::timer::timer_now() >= dl {
                return Poll::Ready(());
            }
        }
        if self.armed {
            retain_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            return Poll::Ready(());
        }
        register_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
        let waker = cx.waker().clone();
        let wake_at = if let Some(dl) = self.deadline {
            let tick = Duration::from_millis(IO_WAIT_TICK_MS);
            let now = kernel_hal::timer::timer_now();
            if now + tick < dl {
                now + tick
            } else {
                dl
            }
        } else {
            kernel_hal::timer::timer_now() + Duration::from_millis(IO_WAIT_TICK_MS)
        };
        kernel_hal::timer::timer_set(wake_at, Box::new(move |_| waker.wake_by_ref()));
        self.armed = true;
        Poll::Pending
    }
}

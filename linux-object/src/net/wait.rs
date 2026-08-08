//! Futures for I/O waits (NIC, HID, TTY).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use kernel_hal::timer_waker::{self, TimerWakerSlot};

use super::{clear_io_wait_wakers, register_io_wait_wakers, retain_io_wait_wakers};

/// Fallback timer when no IRQ wakes a multiplex wait (poll/epoll/select).
pub const IO_WAIT_TICK_MS: u64 = 4;

/// Resolves when Ctrl+C is pending, NET RX wakers fire, or deadline.
pub struct NetOrTtyWait {
    deadline: Duration,
    armed: bool,
    timer: Option<TimerWakerSlot>,
    watch_net: bool,
    watch_hid: bool,
    io_waker: Option<core::task::Waker>,
}

impl NetOrTtyWait {
    pub fn new_after_ms(ms: u64) -> Self {
        Self {
            deadline: kernel_hal::timer::timer_now() + Duration::from_millis(ms),
            armed: false,
            timer: None,
            watch_net: true,
            watch_hid: true,
            io_waker: None,
        }
    }
}

impl Drop for NetOrTtyWait {
    fn drop(&mut self) {
        timer_waker::kill_timer_waker(&mut self.timer);
        if let Some(w) = self.io_waker.take() {
            clear_io_wait_wakers(&w, self.watch_net, self.watch_hid);
        }
    }
}

impl Future for NetOrTtyWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if crate::fs::stdio::ctrl_c_pending_peek() {
            timer_waker::kill_timer_waker(&mut self.timer);
            if let Some(w) = self.io_waker.take() {
                clear_io_wait_wakers(&w, self.watch_net, self.watch_hid);
            }
            return Poll::Ready(());
        }
        if kernel_hal::timer::timer_now() >= self.deadline {
            timer_waker::kill_timer_waker(&mut self.timer);
            if let Some(w) = self.io_waker.take() {
                clear_io_wait_wakers(&w, self.watch_net, self.watch_hid);
            } else {
                clear_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            }
            return Poll::Ready(());
        }
        if self.armed {
            retain_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            timer_waker::kill_timer_waker(&mut self.timer);
            clear_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            self.io_waker = None;
            return Poll::Ready(());
        }
        register_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
        self.io_waker = Some(cx.waker().clone());
        let dl = self.deadline;
        timer_waker::ensure_timer_waker(&mut self.timer, dl, cx);
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
    timer: Option<TimerWakerSlot>,
    /// Waker parked in NET_RX/TTY lists — needed so Drop can unregister without
    /// a `Context` (Ready already clears via `cx.waker()`).
    io_waker: Option<core::task::Waker>,
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
            timer: None,
            io_waker: None,
        }
    }
}

impl Drop for IoMultiplexWait {
    fn drop(&mut self) {
        timer_waker::kill_timer_waker(&mut self.timer);
        if let Some(w) = self.io_waker.take() {
            clear_io_wait_wakers(&w, self.watch_net, self.watch_hid);
        }
    }
}

impl Future for IoMultiplexWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(dl) = self.deadline {
            if kernel_hal::timer::timer_now() >= dl {
                timer_waker::kill_timer_waker(&mut self.timer);
                clear_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
                self.io_waker = None;
                return Poll::Ready(());
            }
        }
        if self.armed {
            retain_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            timer_waker::kill_timer_waker(&mut self.timer);
            // Drop the IRQ registration: the epoll loop boxes a fresh wait each
            // cycle. Leaving the waker in NET_RX/TTY lists after Ready meant a
            // later IRQ could wake a completed wait's task slot under churn
            // (observed as delayed KERNEL PAGE FAULT / null fn-ptr after a
            // long labwc session).
            clear_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
            self.io_waker = None;
            return Poll::Ready(());
        }
        register_io_wait_wakers(cx.waker(), self.watch_net, self.watch_hid);
        self.io_waker = Some(cx.waker().clone());
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
        timer_waker::ensure_timer_waker(&mut self.timer, wake_at, cx);
        self.armed = true;
        Poll::Pending
    }
}

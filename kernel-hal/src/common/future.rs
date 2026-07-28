use alloc::{boxed::Box, sync::Arc};
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use core::{future::Future, pin::Pin};
use zcore_drivers::scheme::DisplayScheme;

use crate::timer;

/// Mutex protecting a [`SleepFuture`]'s waker slot. On bare metal it must be
/// the IRQ-disabling `lock::Mutex`: the armed timer callback runs in IRQ
/// context and takes this lock, so a poller holding it with IRQs on could be
/// interrupted on the same CPU and deadlock. libos has no IRQ context (and
/// `lock`'s IRQ plumbing is unimplemented there), so a plain spinlock is right.
#[cfg(not(feature = "libos"))]
type WakerSlotMutex = lock::Mutex<Option<Waker>>;
#[cfg(feature = "libos")]
type WakerSlotMutex = spin::Mutex<Option<Waker>>;

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
    /// Waker slot shared with the armed timer callback. The timer is armed
    /// exactly ONCE; re-polls (routine when this future is combined with I/O
    /// readiness in a select-style future, e.g. poll/select timeouts) just
    /// refresh the waker in place. Previously every re-poll pushed a fresh
    /// boxed callback into the global timer heap: a heap allocation + global
    /// mutex acquire per poll, plus a pile of stale entries whose expiry
    /// caused spurious wakes (which re-polled and pushed yet more entries).
    slot: Option<Arc<WakerSlotMutex>>,
}

impl SleepFuture {
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            slot: None,
        }
    }
}

impl Future for SleepFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.get_mut();
        if timer::timer_now() >= this.deadline {
            return Poll::Ready(());
        }
        if this.deadline.as_nanos() >= i64::MAX as u128 {
            // "Never": the caller relies on some other wake source.
            return Poll::Pending;
        }
        match &this.slot {
            Some(slot) => {
                let mut w = slot.lock();
                let refresh = match &*w {
                    Some(old) => !old.will_wake(cx.waker()),
                    None => true, // callback already fired and took the waker
                };
                if refresh {
                    *w = Some(cx.waker().clone());
                }
            }
            None => {
                let slot = Arc::new(WakerSlotMutex::new(Some(cx.waker().clone())));
                let cb_slot = slot.clone();
                this.slot = Some(slot);
                timer::timer_set(
                    this.deadline,
                    Box::new(move |_| {
                        let waker = cb_slot.lock().take();
                        if let Some(w) = waker {
                            w.wake();
                        }
                    }),
                );
            }
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
            timer::timer_set(self.next_flush_time, Box::new(move |_| waker.wake_by_ref()));
        }
        Poll::Pending
    }
}

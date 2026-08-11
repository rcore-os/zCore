//! `timerfd_create(2)` — a timer that delivers expirations through a readable
//! file descriptor. libwayland's `wl_event_loop` arms one of these for all its
//! timers, so a Wayland compositor (labwc/wlroots) needs it to run.

use super::*;
use crate::sync::{Event, EventBus};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};
use core::time::Duration;
use lock::Mutex;
use zircon_object::object::*;

/// Shared timer state, separated from the [`TimerFd`] wrapper so the kernel
/// timer callback can hold a `Weak` to *just this* and not keep the fd alive.
struct TimerInner {
    /// Number of expirations since the last successful `read`.
    count: AtomicU64,
    /// Period for a recurring timer, in nanoseconds. `0` = one-shot.
    interval_ns: AtomicU64,
    /// Absolute monotonic deadline (ns) of the next expiration, for `gettime`.
    next_deadline_ns: AtomicU64,
    /// Bumped on every `settime`; an in-flight callback whose generation no
    /// longer matches was disarmed/re-armed and must not fire or re-schedule.
    generation: AtomicU64,
    eventbus: Arc<Mutex<EventBus>>,
}

impl TimerInner {
    /// (Re)arm the timer. `value_ns == 0` disarms it. `abs` selects an absolute
    /// monotonic deadline (`TFD_TIMER_ABSTIME`) rather than a relative one.
    fn arm(self: &Arc<Self>, value_ns: u64, interval_ns: u64, abs: bool) {
        let generation = self.generation.fetch_add(1, SeqCst) + 1;
        self.interval_ns.store(interval_ns, SeqCst);
        if value_ns == 0 {
            self.next_deadline_ns.store(0, SeqCst);
            return; // disarmed
        }
        let now = kernel_hal::timer::timer_now().as_nanos() as u64;
        let deadline = if abs {
            value_ns
        } else {
            now.saturating_add(value_ns)
        };
        self.schedule(deadline, generation);
    }

    fn schedule(self: &Arc<Self>, deadline_ns: u64, generation: u64) {
        self.next_deadline_ns.store(deadline_ns, SeqCst);
        let weak = Arc::downgrade(self);
        kernel_hal::timer::timer_set(
            Duration::from_nanos(deadline_ns),
            Box::new(move |_now| {
                let Some(inner) = weak.upgrade() else { return };
                if inner.generation.load(SeqCst) != generation {
                    return; // disarmed / re-armed: this callback is stale
                }
                inner.count.fetch_add(1, SeqCst);
                inner.eventbus.lock().set(Event::READABLE);
                let interval = inner.interval_ns.load(SeqCst);
                if interval > 0 {
                    let next = kernel_hal::timer::timer_now().as_nanos() as u64 + interval;
                    inner.schedule(next, generation);
                }
            }),
        );
    }
}

/// timerfd implementation.
pub struct TimerFd {
    base: KObjectBase,
    inner: Arc<TimerInner>,
    flags: OpenFlags,
}

impl_kobject!(TimerFd);

impl TimerFd {
    /// Create a disarmed timerfd.
    pub fn new(flags: OpenFlags) -> Arc<Self> {
        Arc::new(TimerFd {
            base: KObjectBase::new(),
            inner: Arc::new(TimerInner {
                count: AtomicU64::new(0),
                interval_ns: AtomicU64::new(0),
                next_deadline_ns: AtomicU64::new(0),
                generation: AtomicU64::new(0),
                eventbus: EventBus::new(),
            }),
            flags,
        })
    }

    /// Arm/disarm (`timerfd_settime`). `abs` = `TFD_TIMER_ABSTIME`.
    pub fn set_time(&self, value_ns: u64, interval_ns: u64, abs: bool) {
        // A fresh arm starts a new expiration epoch.
        self.inner.count.store(0, SeqCst);
        self.inner.eventbus.lock().clear(Event::READABLE);
        self.inner.arm(value_ns, interval_ns, abs);
    }

    /// `(interval_ns, remaining_ns)` for `timerfd_gettime`.
    pub fn get_time(&self) -> (u64, u64) {
        let interval = self.inner.interval_ns.load(SeqCst);
        let deadline = self.inner.next_deadline_ns.load(SeqCst);
        let now = kernel_hal::timer::timer_now().as_nanos() as u64;
        let remaining = deadline.saturating_sub(now);
        (interval, remaining)
    }
}

#[async_trait]
impl FileLike for TimerFd {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
            flags: self.flags,
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        if buf.len() < 8 {
            return Err(LxError::EINVAL);
        }
        loop {
            let count = self.inner.count.swap(0, SeqCst);
            if count > 0 {
                self.inner.eventbus.lock().clear(Event::READABLE);
                buf[..8].copy_from_slice(&count.to_ne_bytes());
                return Ok(8);
            }
            if self.flags.contains(OpenFlags::NON_BLOCK) {
                return Err(LxError::EAGAIN);
            }
            self.async_poll(PollEvents::IN).await?;
        }
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    async fn read_at(&self, _offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf).await
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: self.inner.count.load(SeqCst) > 0,
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        loop {
            let status = self.poll(_events)?;
            if status.read {
                return Ok(status);
            }
            let bus = self.inner.eventbus.clone();
            crate::sync::wait_for_event(bus, Event::READABLE).await;
        }
    }

    fn subscribe_readiness(
        &self,
        events: PollEvents,
        waker: &core::task::Waker,
    ) -> Option<crate::sync::ReadinessSub> {
        // Expiry sets READABLE from the kernel timer callback (see
        // `TimerInner::schedule`), and the deadline-programmed LAPIC fires at
        // the exact deadline — so a parked poller wakes AT expiry instead of
        // on its next 4 ms re-scan tick.
        let mask = super::poll_events_to_bus_mask(events);
        Some(crate::sync::subscribe_readiness_on(
            &self.inner.eventbus,
            mask,
            waker,
        ))
    }
}

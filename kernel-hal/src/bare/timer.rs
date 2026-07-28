//! Time and clock functions.

use alloc::boxed::Box;
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;
use core::convert::TryFrom;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use lock::Mutex;

/// A pending timer event: its absolute deadline and the callback to run.
///
/// Ordered so the [`BinaryHeap`] (a max-heap) yields the *earliest* deadline
/// first — `cmp` is reversed on `deadline`.
struct TimerEvent {
    deadline: Duration,
    callback: Box<dyn FnOnce(Duration) + Send + Sync + 'static>,
}

impl PartialEq for TimerEvent {
    fn eq(&self, other: &Self) -> bool {
        self.deadline.eq(&other.deadline)
    }
}
impl Eq for TimerEvent {}
impl PartialOrd for TimerEvent {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEvent {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Reverse so the min-deadline is at the top of the max-heap.
        other.deadline.cmp(&self.deadline)
    }
}

/// A minimal timer heap.
///
/// Unlike `naive_timer::Timer`, whose `expire()` runs every due callback
/// *inline while the heap is borrowed*, this type separates draining from
/// invoking: [`Self::drain_expired`] pops the due events and returns their
/// callbacks so the caller can drop the `NAIVE_TIMER` lock BEFORE running
/// them. That is essential here — timer callbacks re-arm periodic timers by
/// calling `timer_set`, which re-locks `NAIVE_TIMER`; running them under the
/// lock is a same-CPU re-entrant self-deadlock (see `timer_tick`).
#[derive(Default)]
struct TimerHeap {
    events: BinaryHeap<TimerEvent>,
}

impl TimerHeap {
    fn add(
        &mut self,
        deadline: Duration,
        callback: Box<dyn FnOnce(Duration) + Send + Sync + 'static>,
    ) {
        self.events.push(TimerEvent { deadline, callback });
    }

    /// Deadline of the earliest pending timer, if any.
    fn next(&self) -> Option<Duration> {
        self.events.peek().map(|e| e.deadline)
    }

    /// Pop all events whose deadline is `<= now`, returning their callbacks.
    /// The heap lock can then be released before the callbacks are invoked.
    fn drain_expired(
        &mut self,
        now: Duration,
    ) -> Vec<Box<dyn FnOnce(Duration) + Send + Sync + 'static>> {
        let mut ready = Vec::new();
        while let Some(t) = self.events.peek() {
            if t.deadline > now {
                break;
            }
            ready.push(self.events.pop().unwrap().callback);
        }
        ready
    }
}

/// Timer interrupt frequency in Hz.
/// 250 Hz gives a 4 ms tick granularity — a good balance between
/// scheduler responsiveness and interrupt overhead for a desktop/interactive
/// workload. (Previous value was 100 Hz / 10 ms which caused noticeable lag.)
pub(super) const TICKS_PER_SEC: u64 = 250;

/// Master switch for tickless idle. Set to `false` to fall back to the plain
/// full-rate periodic tick everywhere. Only consumed on arches with a re-armable
/// per-CPU timer (x86_64 today).
///
/// Currently DISABLED: stretching the LAPIC count in `timer_idle_enter` was
/// observed to stop the timer from firing once a CPU actually halts — pending
/// timers (the poll/select 4 ms re-arm, socket fallbacks, scheduled wakeups)
/// then never expire. A net busy-spin had been masking this by keeping the CPUs
/// awake and the LAPIC at its full-rate count; the moment the spin was removed
/// (so the cores genuinely halted) all timers stalled, which froze the shell's
/// `poll(stdin)` re-poll loop and with it the xHCI HID polling — i.e. the
/// keyboard/mouse went dead. With tickless off the LAPIC keeps its 250 Hz
/// periodic count, so timers fire reliably; an idle CPU still halts via `hlt`
/// between ticks (≈99 % idle), it just wakes every 4 ms. Re-enabling tickless
/// needs the re-arm path fixed so a halted CPU's timer still expires.
#[allow(dead_code)]
const TICKLESS_IDLE: bool = false;

/// Upper bound on how long an idle CPU may sleep between scheduler ticks, in
/// nanoseconds (50 ms ≈ 20 Hz). Nearer pending timers are always honoured; this
/// only bounds the "nothing pending" case so USB-HID polling and the cursor
/// blink keep running, and so a timer *set* after a CPU has already halted is
/// serviced within this bound. Lowering it trades idle CPU for responsiveness.
#[allow(dead_code)]
const IDLE_TICK_CAP_NS: u64 = 50_000_000;

lazy_static::lazy_static! {
    static ref NAIVE_TIMER: Mutex<TimerHeap> = Mutex::new(TimerHeap::default());
}

/// Offset (in nanoseconds) added to monotonic boot time for
/// `CLOCK_REALTIME` / `gettimeofday`. Stored as a raw `u64` so the read path
/// (`clock_gettime` is on libc's critical path for almost every interactive
/// program) hits a single relaxed load instead of acquiring a spinlock.
/// `u64` nanoseconds covers ~584 years from the Unix epoch — more than
/// enough for any wall-clock we care about.
static WALL_CLOCK_OFFSET_NS: AtomicU64 = AtomicU64::new(0);

/// Earliest pending timer deadline (in monotonic nanoseconds), or `u64::MAX`
/// when no timer is registered. Maintained alongside the heap inside the
/// `NAIVE_TIMER` lock, but readable lock-free. Lets every CPU's per-tick
/// `timer_tick` skip the spinlock when there is nothing to expire — the
/// common case under multi-CPU where all CPUs would otherwise contend on
/// the timer mutex 250 times a second.
static NEXT_DEADLINE_NS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Most recent monotonic time (ns) at which the shared xHCI controller was
/// polled from a timer tick. Used to rate-limit the background HID poll across
/// all CPUs (see `timer_tick`). Only meaningful on x86_64 with PCI/USB.
#[cfg(all(target_arch = "x86_64", not(feature = "no-pci")))]
static XHCI_LAST_POLL_NS: AtomicU64 = AtomicU64::new(0);

/// Minimum spacing between background xHCI HID polls (~125 Hz). `timer_tick`
/// runs on every CPU at up to 250 Hz, but the single shared controller only
/// needs HID-rate sampling, so this bounds the aggregate poll rate regardless
/// of CPU count. Active stdin reads still poll at full rate via the io_wait
/// path, so this does not add key latency.
#[cfg(all(target_arch = "x86_64", not(feature = "no-pci")))]
const XHCI_POLL_INTERVAL_NS: u64 = 8_000_000;

#[inline]
fn duration_to_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// Wall-clock time (Unix epoch): monotonic since boot + adjustable offset.
pub fn wall_clock_now() -> Duration {
    let offset = Duration::from_nanos(WALL_CLOCK_OFFSET_NS.load(Ordering::Relaxed));
    timer_now() + offset
}

/// Set wall-clock instant (`CLOCK_REALTIME` / `settimeofday`).
pub fn wall_clock_set(target: Duration) {
    let mono = timer_now();
    let offset = target.saturating_sub(mono);
    // `Duration::as_nanos` is u128; truncate to u64. Anything beyond ~584
    // years would already be a nonsensical wall-clock value here, so
    // clamping is fine.
    let ns = u64::try_from(offset.as_nanos()).unwrap_or(u64::MAX);
    WALL_CLOCK_OFFSET_NS.store(ns, Ordering::Relaxed);
}

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
        fn timer_enable() {
            super::arch::timer_init();
        }

        fn timer_now() -> Duration {
            super::arch::timer::timer_now()
        }

        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
            debug!("Set timer at: {:?}", deadline);
            // Mutex::lock() uses push_off/pop_off which already handles interrupt
            // disabling. Manual intr_off/on here would bypass the noff accounting
            // and cause "RefCell already borrowed" panics under SMP.
            let mut t = NAIVE_TIMER.lock();
            t.add(deadline, callback);
            // Republish the new earliest deadline so other CPUs' fast-path
            // ticks observe it. Done under the lock so concurrent updates
            // can't race with `timer_tick`'s post-expire publish.
            let next = t.next().map(duration_to_ns).unwrap_or(u64::MAX);
            NEXT_DEADLINE_NS.store(next, Ordering::Release);
        }

        fn timer_tick() {
            crate::kstats::note_timer_tick();
            // Blink the framebuffer text cursor. Cheap (one atomic load) on most
            // ticks; must run before the lock-free deadline fast-path below so it
            // keeps blinking while the system is idle with no pending timers.
            crate::console::cursor_blink_tick();

            let now = timer_now();

            // Maintain the cross-CPU monotonic floor (and watch for TSC skew)
            // at tick rate, so `timer_now()`'s invariant-TSC fast path can skip
            // the globally-contended RMW on every clock read.
            #[cfg(target_arch = "x86_64")]
            super::arch::timer::mono_floor_tick(duration_to_ns(now));

            // Background USB-HID poll. `timer_tick` fires on *every* CPU at up to
            // 250 Hz, but the single shared xHCI controller only needs HID-rate
            // sampling, so rate-limit the poll to ~125 Hz across all CPUs: the
            // first CPU to see the interval elapse claims the slot via CAS and
            // polls; the others skip it. This collapses ~250 Hz × N_CPU MMIO
            // polls — plus contention on the two global xHCI spinlocks — into
            // ~125 Hz total. Active stdin reads still poll input at full rate
            // through the io_wait path, so key latency is unaffected.
            #[cfg(all(target_arch = "x86_64", not(feature = "no-pci")))]
            {
                let now_ns = duration_to_ns(now);
                let last = XHCI_LAST_POLL_NS.load(Ordering::Relaxed);
                if now_ns.saturating_sub(last) >= XHCI_POLL_INTERVAL_NS
                    && XHCI_LAST_POLL_NS
                        .compare_exchange(last, now_ns, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                {
                    crate::kstats::note_hid_poll_timer(); // [diag]
                    zcore_drivers::usb::xhci_hid::poll();
                }
            }
            // Adaptive thermal P-state governor: each CPU samples its own
            // temperature and nudges its HWP/CPPC ceiling to stay cool. Self
            // rate-limited to ~1 Hz per core, so cheap to call every tick; runs
            // before the deadline fast-path so it ticks under load too (when the
            // package is hot but there are no expiring timers).
            #[cfg(target_arch = "x86_64")]
            super::arch::power::thermal_governor_tick();

            // Lock-free fast path: if the earliest pending deadline hasn't
            // arrived yet, skip the mutex entirely. Saves a spinlock acquire
            // per CPU per tick (250 Hz × N CPUs), which is the dominant
            // contention on the timer mutex under SMP.
            if duration_to_ns(now) < NEXT_DEADLINE_NS.load(Ordering::Acquire) {
                return;
            }
            // Drain the due callbacks and republish the next deadline while
            // holding the lock, then RELEASE it before invoking them. Running a
            // callback under the lock would deadlock: periodic timers (POSIX
            // timers, timerfd) re-arm themselves by calling `timer_set`, which
            // re-locks `NAIVE_TIMER` on this same CPU. Dropping the guard first
            // makes that re-entrancy safe.
            let expired = {
                let mut t = NAIVE_TIMER.lock();
                let expired = t.drain_expired(now);
                let next = t.next().map(duration_to_ns).unwrap_or(u64::MAX);
                NEXT_DEADLINE_NS.store(next, Ordering::Release);
                expired
            };
            for callback in expired {
                callback(now);
            }
        }

        fn timer_idle_enter() {
            #[cfg(target_arch = "x86_64")]
            if TICKLESS_IDLE {
                // Stretch this CPU's tick to the next pending timer deadline,
                // capped, so a fully idle CPU stops taking the 250 Hz tick. The
                // periodic timer keeps firing at the stretched period; on the
                // next wake `timer_idle_exit` restores the fast tick.
                let now = duration_to_ns(timer_now());
                let next = NEXT_DEADLINE_NS.load(Ordering::Acquire);
                let span = next.saturating_sub(now).min(IDLE_TICK_CAP_NS);
                super::arch::timer::set_tick_count(super::arch::timer::ns_to_tick_count(span));
                super::percpu::set_timer_idle_armed(true);
            }
        }

        fn timer_idle_exit() {
            #[cfg(target_arch = "x86_64")]
            if TICKLESS_IDLE && super::percpu::timer_idle_armed() {
                // Resuming real work: restore the full-rate scheduler tick so
                // preemption and HID polling run at their normal cadence.
                super::arch::timer::set_tick_count(super::arch::timer::fast_tick_count());
                super::percpu::set_timer_idle_armed(false);
            }
        }
    }
}

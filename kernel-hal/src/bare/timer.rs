//! Time and clock functions.

use alloc::boxed::Box;
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;
use core::convert::TryFrom;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// How many corrupt timer callbacks were skipped after a null-range EXECUTE
/// #PF during dispatch (see `try_skip_timer_callback_fault` in the x86_64
/// trap path). Diagnostic only.
static TIMER_CALLBACKS_SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// Whether this CPU is inside `timer_tick` (housekeeping or Box callbacks).
#[inline]
pub fn in_timer_callback() -> bool {
    super::percpu::in_timer_callback()
}

/// Note that a timer-path indirect call was skipped after a contained
/// null-range #PF. Does **not** adjust nesting depth: recovery resumes at the
/// return site of the bad `call`, so the normal `end_timer_callback` in
/// `timer_tick` still runs.
#[inline]
pub fn note_timer_callback_skipped() {
    TIMER_CALLBACKS_SKIPPED.fetch_add(1, Ordering::Relaxed);
}

/// Number of timer callbacks skipped due to contained null-range #PF.
#[inline]
pub fn timer_callbacks_skipped() -> usize {
    TIMER_CALLBACKS_SKIPPED.load(Ordering::Relaxed)
}

// ── Deadline-programmed timer ───────────────────────────────────────────────
//
// Timers used to expire *only* on the 250 Hz periodic tick: `timer_set` pushed
// onto the heap and `timer_tick` drained whatever was due. That made 4 ms the
// floor on every sleep, poll/select timeout, socket retransmit and scheduled
// wakeup in the system — measured as a mean 2.8 ms overshoot on
// `nanosleep(1 ms)` against Linux's 0.6 ms in the same emulator, because Linux
// programs its LAPIC for the actual deadline instead of rounding up to the next
// tick.
//
// So do we now. The direction is deliberately one-way: this only ever makes a
// CPU's timer fire *sooner* than its scheduler tick would have, never later.
// That matters — the previous attempt at re-arming (`TICKLESS_IDLE`, still
// disabled below) stretched the period to skip ticks on an idle CPU and left
// halted CPUs with timers that never expired, killing input. Shortening cannot
// reproduce that failure: in the worst case a CPU simply takes more ticks than
// it needs, and `timer_tick` re-establishes the 4 ms bound on every fire.

/// Master switch for deadline programming, settable from the kernel command
/// line (`TIMERDEADLINE=0`). Off, timers expire only on the 250 Hz periodic
/// tick exactly as before — so the two behaviours can be compared on one build,
/// on one machine, by rebooting.
#[cfg(target_arch = "x86_64")]
static DEADLINE_TIMER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Enable/disable deadline programming of the LAPIC timer.
pub fn set_deadline_timer(enabled: bool) {
    #[cfg(target_arch = "x86_64")]
    DEADLINE_TIMER.store(enabled, Ordering::Relaxed);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = enabled;
}

/// Whether deadline programming is enabled.
pub fn deadline_timer_enabled() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        DEADLINE_TIMER.load(Ordering::Relaxed)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Floor on how short the LAPIC period may be set, bounding the worst-case
/// timer interrupt rate to ~5 kHz per CPU. A deadline nearer than this is
/// served on the next fire instead of chasing it, which is also what Linux's
/// `min_delta_ns` does for the same reason.
#[cfg(target_arch = "x86_64")]
const MIN_ARM_NS: u64 = 200_000;

/// Absolute monotonic time (ns) each CPU's LAPIC timer is currently expected to
/// fire at. `0` means "not yet armed by this mechanism" and is treated as
/// infinitely far away so the first arm always takes effect.
#[cfg(target_arch = "x86_64")]
static ARMED_NS: [AtomicU64; crate::config::MAX_CORE_NUM] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CORE_NUM];

/// Absolute monotonic time (ns) by which each CPU's *scheduler* tick is due.
/// Arming never pushes a CPU's next fire past this, so no amount of timer
/// churn can starve preemption or the per-tick housekeeping.
#[cfg(target_arch = "x86_64")]
static TICK_DUE_NS: [AtomicU64; crate::config::MAX_CORE_NUM] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CORE_NUM];

/// Program this CPU's LAPIC timer to fire at `target_ns` (absolute monotonic),
/// but no later than its scheduler tick is already due and no sooner than
/// [`MIN_ARM_NS`] from now. No-op if it is already set to fire at least that
/// soon.
///
/// Callers must not be preemptible across this: it reads `cpu_id` and then
/// writes *that* CPU's local APIC. Both call sites hold an IRQ-disabling lock
/// or run in interrupt context.
#[cfg(target_arch = "x86_64")]
fn arm_deadline(now_ns: u64, target_ns: u64) {
    if !DEADLINE_TIMER.load(Ordering::Relaxed) {
        return;
    }
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu >= crate::config::MAX_CORE_NUM {
        return;
    }
    let tick_due = TICK_DUE_NS[cpu].load(Ordering::Relaxed);
    // Before the first tick on this CPU there is no recorded due time; fall
    // back to a full period from now so the clamp below still bounds us.
    let tick_due = if tick_due == 0 {
        now_ns + super::arch::timer::fast_tick_ns()
    } else {
        tick_due
    };
    let target = target_ns.min(tick_due);
    let armed = ARMED_NS[cpu].load(Ordering::Relaxed);
    // Hysteresis: only reprogram when it buys at least a full `MIN_ARM_NS`.
    // Without it a stream of timers with slightly-decreasing deadlines (a busy
    // poll/select loop, socket retransmit timers) reprograms the LAPIC on every
    // `timer_set`, and each reprogram restarts the countdown — which both costs
    // an MMIO write per call and can push the interrupt rate far above what the
    // deadlines actually require.
    if armed != 0 && target.saturating_add(MIN_ARM_NS) >= armed {
        return; // already firing at least this soon
    }
    let span = target
        .saturating_sub(now_ns)
        .clamp(MIN_ARM_NS, super::arch::timer::fast_tick_ns());
    super::arch::timer::set_tick_count(super::arch::timer::ns_to_tick_count(span));
    ARMED_NS[cpu].store(now_ns + span, Ordering::Relaxed);
    crate::kstats::note_timer_rearm();
}

/// Called from `timer_tick` after the due callbacks have been drained: record
/// when this CPU's next scheduler tick is due and re-arm for the earliest
/// pending deadline within that window.
#[cfg(target_arch = "x86_64")]
fn rearm_after_tick(now_ns: u64) {
    if !DEADLINE_TIMER.load(Ordering::Relaxed) {
        return;
    }
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu >= crate::config::MAX_CORE_NUM {
        return;
    }
    let tick_due = now_ns + super::arch::timer::fast_tick_ns();
    TICK_DUE_NS[cpu].store(tick_due, Ordering::Relaxed);
    // Reset the armed marker first so `arm_deadline` is free to shorten again.
    ARMED_NS[cpu].store(tick_due, Ordering::Relaxed);
    super::arch::timer::set_tick_count(super::arch::timer::fast_tick_count());
    let next = NEXT_DEADLINE_NS.load(Ordering::Acquire);
    if next != u64::MAX {
        arm_deadline(now_ns, next);
    }
}

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
    notify_clock_changed();
}

/// The offset `wall_clock_now` adds to monotonic time, in nanoseconds.
///
/// Exposed so the Linux personality can hand the same number to userspace
/// through the vDSO instead of having it recomputed from a `Duration` — the two
/// clocks must agree exactly, and the cheapest way to guarantee that is for
/// there to be one number.
pub fn wall_clock_offset_ns() -> u64 {
    WALL_CLOCK_OFFSET_NS.load(Ordering::Relaxed)
}

/// The TSC→ns multiplier when the TSC is fit for userspace to read directly.
///
/// `None` on every architecture but x86_64, and on x86_64 whenever the counter
/// is not invariant — see the x86_64 implementation for what that costs.
pub fn vdso_tsc_mult() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        super::arch::timer::vdso_tsc_mult()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}

/// Treat the TSC as usable by userspace regardless of what CPUID reports.
///
/// Set from the kernel command line (`VDSOFORCE=1`); a no-op off x86_64. See
/// the x86_64 implementation for when this is sound.
pub fn set_force_tsc_invariant(force: bool) {
    #[cfg(target_arch = "x86_64")]
    super::arch::timer::set_force_tsc_invariant(force);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = force;
}

/// Notified whenever the parameters userspace reads the clock through may have
/// changed: the wall-clock offset, or the TSC's fitness to be read at all.
///
/// A callback rather than a direct call because the clock lives here and the
/// vDSO — a Linux ABI object — lives in the personality above, which this crate
/// must not depend on. Registered once, when the vDSO image is first built.
static CLOCK_OBSERVER: AtomicUsize = AtomicUsize::new(0);

/// Register the clock-parameter observer. Later registrations replace earlier
/// ones; in practice there is exactly one, installed before any process runs.
pub fn set_clock_observer(observer: fn()) {
    CLOCK_OBSERVER.store(observer as usize, Ordering::Release);
    // Publish the current values immediately: the observer exists to keep a
    // copy in step, and it starts out with no copy at all.
    observer();
}

/// Invoke the observer, if one is registered.
pub(crate) fn notify_clock_changed() {
    let observer = CLOCK_OBSERVER.load(Ordering::Acquire);
    if observer == 0 {
        return;
    }
    // Soft-smash can leave a truncated .text low32 in this AtomicUsize.
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        if (observer as u64 >> 32) != 0xffff_ff00 {
            zcore_drivers::utils::note_heap_smash_suspected();
            return;
        }
    }
    // Safe: the only value ever stored is a `fn()` cast from a live
    // function pointer, and it is never unregistered (unless smashed).
    let observer: fn() = unsafe { core::mem::transmute(observer) };
    observer();
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
            // Bring this CPU's timer forward to the new deadline if it is
            // nearer than the pending fire. Without this the timer would only
            // be noticed on the next 4 ms tick, which is the whole of the
            // sleep/poll/select latency gap against Linux. Done while still
            // holding the (IRQ-disabling) heap lock so we cannot migrate
            // between reading `cpu_id` and writing that CPU's local APIC.
            #[cfg(target_arch = "x86_64")]
            if next != u64::MAX {
                arm_deadline(duration_to_ns(timer_now()), next);
            }
            drop(t);
        }

        fn timer_tick() {
            crate::kstats::note_timer_tick();
            // Mark the *entire* tick — not only Box callbacks. Pre-loop work
            // (cursor blink → DisplayScheme vtable, xHCI poll → EventListener
            // handlers, mono_floor → CLOCK_OBSERVER) also does indirect calls
            // from IRQ context with no current thread. A null-range EXECUTE
            // #PF there used to report `in_timer_callback=false` and halt;
            // with the flag raised the x86_64 trap path can skip a bad call
            // when `[rsp]` still holds a valid `.text` return.
            super::percpu::begin_timer_callback();

            // Program this CPU's debug registers from the globally-published
            // watchpoint request, if one changed since the last tick. A pure
            // generation compare (one relaxed load) when nothing is armed —
            // WP_GEN stays 0 on a normal boot, so this never touches DR. It goes
            // live only after the null-execute path auto-arms a write-watch on
            // the zeroed stack run (see `dump_null_execute_stack_once`), which is
            // what finally lets the #DB trap name the zero-writer's rip. Without
            // this call the whole watchpoint facility was dead code: the request
            // was published but no CPU ever loaded it into DR0.
            //
            // (Generation 2 of the null-exec hunt: the same sync now also keeps
            // DR0-DR3 loaded with the live executors' write-once spine slots,
            // so the corruptor traps with its rip — see `watchpoint.rs`.)
            crate::watchpoint::sync_this_cpu();

            // Spine-slot sweep (null-exec hunt): verify every live executor's
            // write-once `[stack_top-0x508]` return slot each tick. This names
            // the smash within ~one tick of the write — usually while the
            // victim coroutine is still deep, BEFORE its fatal `ret`-to-0 —
            // even if the debug registers missed the store. ≤16 relaxed loads
            // when healthy. Printing uses the IRQ-safe spin serial writer.
            #[cfg(target_arch = "x86_64")]
            if let Some(s) = ::executor::spine_verify() {
                ::executor::note_heap_smash_suspected();
                // Detection self-heals per slot (see `spine_verify`), so each
                // report is a distinct write; cap the total anyway so a
                // pathological repeat writer cannot own the console.
                if s.ordinal < 12 {
                crate::console::serial_write_fmt_spin(format_args!(
                    "\n[spine-smash] #{}: executor id={} spine slot {:#x} \
                     (stack {:#x}..{:#x}) expected {:#x} found {:#x}\n",
                    s.ordinal + 1,
                    s.exec_id,
                    s.slot,
                    s.stack_base,
                    s.stack_top,
                    s.expected,
                    s.found,
                ));
                if s.ordinal == 0 {
                    crate::console::serial_write_fmt_spin(format_args!(
                        "[spine-smash] blob [{:#x}..{:#x}) = {} bytes \
                         (top-relative [-{:#x}..-{:#x})); edge fingerprint:\n",
                        s.blob_lo,
                        s.blob_hi,
                        s.blob_hi - s.blob_lo,
                        s.stack_top.saturating_sub(s.blob_lo),
                        s.stack_top.saturating_sub(s.blob_hi),
                    ));
                    // The qwords bracketing each blob edge fingerprint the
                    // foreign object (its nonzero header/tail fields).
                    for k in 0..6usize {
                        let a = s.blob_lo.saturating_sub(8 * (6 - k));
                        if a >= s.stack_base {
                            let v = unsafe { core::ptr::read_volatile(a as *const u64) };
                            crate::console::serial_write_fmt_spin(format_args!(
                                "[spine-smash]   below @{a:#x} = {v:#018x}\n"
                            ));
                        }
                    }
                    for k in 0..6usize {
                        let a = s.blob_hi + 8 * k;
                        if a + 8 <= s.stack_top {
                            let v = unsafe { core::ptr::read_volatile(a as *const u64) };
                            crate::console::serial_write_fmt_spin(format_args!(
                                "[spine-smash]   above @{a:#x} = {v:#018x}\n"
                            ));
                        }
                    }
                    // Cross-CPU context: what every core was doing at its last
                    // tick — the writer is at most one tick away on one of
                    // them.
                    crate::kstats::dump_last_tick_rips();
                }
                }
            }

            // Soft-smash sticky AND hard guards installed: the heap/stack is
            // corrupt but growth-overflow is ruled out. This USED to halt the
            // CPU here. Isolation-first: do NOT halt — force-skip every dangerous
            // dyn dispatch (below) so the timer path cannot call through a
            // smashed fat-ptr, while the CPU keeps running so `oops` can isolate
            // the guilty task and the rest of the system carries on. Warn once,
            // not at 250 Hz.
            let smash_force_skip = {
                let (hard, _) = ::executor::hard_guard_executor_counts();
                hard > 0
                    && (::executor::heap_smash_suspected()
                        || zcore_drivers::utils::heap_smash_suspected())
            };
            if smash_force_skip {
                note_timer_callback_skipped();
                static WARNED: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    crate::console::serial_write_str(
                        "\n[soft-smash] timer_tick: smash sticky + hard guards — skipping \
                         ALL timer dyn dispatch from here on (kernel stays up; \
                         isolation-first, no halt)\n",
                    );
                }
            }

            // Idle IRQ + smash sticky / null stack top / near-overflow: skip
            // ALL dyn dispatch this tick (cursor, HID poll, Box callbacks).
            // Calling through a half-smashed fat-ptr with no current thread is
            // the `rip=0` / `[rsp0]=0` desktop bring-up halt.
            let skip_dyn = smash_force_skip
                || ::executor::irq_should_skip_dyn_dispatch()
                || (::executor::irq_on_idle_executor()
                    && zcore_drivers::utils::heap_smash_suspected());

            // Blink the framebuffer text cursor. Cheap (one atomic load) on most
            // ticks; must run before the lock-free deadline fast-path below so it
            // keeps blinking while the system is idle with no pending timers.
            if !skip_dyn {
                crate::console::cursor_blink_tick();
            }

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
                // IRQs nest on the coroutine stack. Near overflow / smash
                // suspect: skip HID poll (`EventListener::trigger` walking
                // `Box<dyn Fn>`). MSI + io_wait still deliver input.
                if !skip_dyn {
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
            }
            // Adaptive thermal P-state governor: each CPU samples its own
            // temperature and nudges its HWP/CPPC ceiling to stay cool. Self
            // rate-limited to ~1 Hz per core, so cheap to call every tick; runs
            // before the deadline fast-path so it ticks under load too (when the
            // package is hot but there are no expiring timers).
            #[cfg(target_arch = "x86_64")]
            super::arch::power::thermal_governor_tick();

            // Re-establish this CPU's 4 ms scheduler-tick bound and re-arm for
            // the earliest pending deadline inside it. Done before the fast
            // path below so a tick that expires nothing still restores the
            // period after a short deadline arm shortened it.
            #[cfg(target_arch = "x86_64")]
            rearm_after_tick(duration_to_ns(now));

            // Lock-free fast path: if the earliest pending deadline hasn't
            // arrived yet, skip the mutex entirely. Saves a spinlock acquire
            // per CPU per tick (250 Hz × N CPUs), which is the dominant
            // contention on the timer mutex under SMP.
            if skip_dyn || duration_to_ns(now) < NEXT_DEADLINE_NS.load(Ordering::Acquire) {
                // On skip_dyn leave the heap untouched so live one-shots
                // (DRM flip, poll wakers) still fire on a later safer tick.
                super::percpu::end_timer_callback();
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
            // Fat-ptr gate (null / non-kernel vtable e.g. `0x13446`): same as
            // EventListener / x86_apic. High-bits rejects smash residue without
            // the PR #759 false-positive null-only sniff. Also abort the rest
            // of this tick if soft-smash was sticky-noted mid-pass.
            let mut smash_abort = false;
            for callback in expired {
                if smash_abort
                    || ::executor::heap_smash_suspected()
                    || zcore_drivers::utils::heap_smash_suspected()
                {
                    if !smash_abort {
                        note_timer_callback_skipped();
                        smash_abort = true;
                    }
                    core::mem::forget(callback);
                    continue;
                }
                if !zcore_drivers::utils::dyn_fat_ptr_live(&callback) {
                    core::mem::forget(callback);
                    continue;
                }
                // [diag] Publish which callback is about to run, so a fault
                // inside it can name the exact closure instead of "somewhere
                // in the tick". Read back by the trap path's zero-chain dump;
                // symbolize the vtable with llvm-addr2line to identify the
                // arming call site's closure type. Two relaxed stores per
                // callback — noise-level next to the dyn dispatch itself.
                {
                    let words = &callback as *const _ as *const usize;
                    // SAFETY: a Box<dyn FnOnce> is exactly {data, vtable}.
                    let (data, vtable) = unsafe { (*words, *words.add(1)) };
                    crate::kstats::note_timer_cb(data as u64, vtable as u64);
                }
                callback(now);
                crate::kstats::note_timer_cb(0, 0);
            }
            // Callbacks routinely re-arm periodic timers (POSIX timers,
            // timerfd, socket retransmits) via `timer_set`, which arms against
            // the clock as it was *before* they ran. Re-arm once more from the
            // republished deadline so a re-armed short timer is honoured on
            // this tick rather than waiting for the next one.
            #[cfg(target_arch = "x86_64")]
            {
                let next = NEXT_DEADLINE_NS.load(Ordering::Acquire);
                if next != u64::MAX {
                    arm_deadline(duration_to_ns(timer_now()), next);
                }
            }
            super::percpu::end_timer_callback();
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

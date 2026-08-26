//! Spinlock deadlock self-report, shared by every mutex flavor in this crate.
//!
//! A spinlock that spins "forever" (billions of PAUSEs, i.e. many seconds) is
//! a deadlock on an IRQ-off kernel: the machine freezes with no panic and no
//! console output — indistinguishable from a hard hang on a monitor-only box.
//! When a waiter crosses the threshold it calls the installed hook ONCE with
//! its `#[track_caller]` location, so the kernel can paint the stuck call site
//! somewhere lock-free (e.g. straight onto the framebuffer). The hook MUST NOT
//! take locks or allocate.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static DEADLOCK_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Default threshold: ~8s of PAUSE iterations on current hardware. Normal
/// contention is orders of magnitude below this; only a genuine
/// deadlock/livelock crosses it.
pub const DEADLOCK_SPINS_DEFAULT: u64 = 1_000_000_000;

/// Live threshold, overridable at boot via [`set_deadlock_spins`].
///
/// A fixed count calibrated for real hardware is unreachable under emulation: a
/// billion spin iterations that take eight seconds natively take many minutes
/// under QEMU/TCG, so on an emulated run the detector effectively never fires
/// and a genuine deadlock is indistinguishable from a slow one. That cost a
/// real debugging cycle — a hang was read as "not a deadlock, no banner
/// appeared" when in truth the threshold had simply not been reached (and the
/// banner was framebuffer-only besides). Lower it with `DEADLOCKSPINS=<n>` on
/// the kernel command line when running emulated.
static DEADLOCK_SPINS_LIVE: AtomicU64 = AtomicU64::new(DEADLOCK_SPINS_DEFAULT);

/// Override the deadlock spin threshold. `0` restores the default.
pub fn set_deadlock_spins(spins: u64) {
    DEADLOCK_SPINS_LIVE.store(
        if spins == 0 {
            DEADLOCK_SPINS_DEFAULT
        } else {
            spins
        },
        Ordering::Relaxed,
    );
}

/// The current threshold. Read on the spin path, so kept to one relaxed load.
#[inline(always)]
pub(crate) fn deadlock_spins() -> u64 {
    DEADLOCK_SPINS_LIVE.load(Ordering::Relaxed)
}

/// Install the deadlock self-report hook (`file`, `line` of the stuck caller).
pub fn set_deadlock_hook(f: fn(&'static str, u32)) {
    DEADLOCK_HOOK.store(f as usize, Ordering::SeqCst);
}

#[inline(never)]
pub(crate) fn report_deadlock(file: &'static str, line: u32) {
    let h = DEADLOCK_HOOK.load(Ordering::Relaxed);
    if h != 0 {
        let f: fn(&'static str, u32) = unsafe { core::mem::transmute(h) };
        f(file, line);
    }
}

/// Public variant for OTHER crates' spinlocks (e.g. the scheduler's
/// `spin::Mutex`-based runtime locks) to self-report through the same hook.
pub fn report_stuck(file: &'static str, line: u32) {
    report_deadlock(file, line);
}

static DEADLOCK_HOLDER_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Hook run periodically from inside spin loops, IRQs still disabled.
///
/// A CPU spinning on a ticket lock has interrupts off (`push_off` precedes the
/// spin), so it cannot take the TLB-shootdown IPI — and a peer performing a
/// shootdown spin-waits for its ack. Under parallel VM work that is a convoy:
/// the flusher holds the lock the spinners want, the spinners' silence burns
/// the flusher's whole ack budget, and 4 CPUs faulting in parallel measured
/// 80x SLOWER in absolute terms than 1. The hook lets a spinning waiter drain
/// its own shootdown queue (queue-only work: lock-free SPSC ring + `invlpg`,
/// no locks taken), turning the ack latency from "whenever the lock is
/// released" into "the next pump interval".
static SPIN_PUMP: AtomicUsize = AtomicUsize::new(0);

/// Install the spin-pump hook. MUST take no locks and never allocate.
pub fn set_spin_pump(f: fn()) {
    SPIN_PUMP.store(f as usize, Ordering::SeqCst);
}

#[inline]
pub(crate) fn spin_pump() {
    let h = SPIN_PUMP.load(Ordering::Relaxed);
    if h != 0 {
        let f: fn() = unsafe { core::mem::transmute(h) };
        f();
    }
}

/// Public variant for OTHER crates' IRQs-off spin loops (the scheduler's
/// `spin::Mutex`-based runtime locks in `diag_lock`, the RM's `os_*_spinlock`
/// glue) to drain their OWN pending TLB-shootdown queue while spinning, exactly
/// as this crate's ticket lock does at `set_spin_pump`.
///
/// A CPU spinning with interrupts disabled is deaf to the shootdown IPI, so a
/// peer that spin-waits for its ack (while holding, say, the VMAR lock) wedges —
/// and every CPU queued behind that lock wedges with it. Only this crate's own
/// ticket lock pumped; a CPU parked in any other IRQs-off spinner was an ack
/// black hole. Callers should invoke it at a coarse cadence (e.g. every 512
/// spins); it is a single relaxed load when no pump is installed and one
/// queue-pointer compare when the queue is empty. Same contract as the hook:
/// takes no locks, never allocates.
#[inline]
pub fn pump() {
    spin_pump();
}

/// Install the holder-report hook: `(file_ptr, file_len, line, cpu)` of the
/// CURRENT HOLDER of a lock some CPU has been spinning on for ~8s. The
/// spinners a deadlock banner lists are usually innocent readers; this is the
/// line that names the culprit. The file travels as raw parts because it is
/// snapshotted from the lock's atomics, not a live `&'static str` — it still
/// points into an immortal `#[track_caller]` string. Same rules as the main
/// hook: MUST NOT take locks or allocate.
pub fn set_deadlock_holder_hook(f: fn(usize, usize, u32, u32)) {
    DEADLOCK_HOLDER_HOOK.store(f as usize, Ordering::SeqCst);
}

#[inline(never)]
pub(crate) fn report_deadlock_holder(file_ptr: usize, file_len: usize, line: u32, cpu: u32) {
    let h = DEADLOCK_HOLDER_HOOK.load(Ordering::Relaxed);
    if h != 0 {
        let f: fn(usize, usize, u32, u32) = unsafe { core::mem::transmute(h) };
        f(file_ptr, file_len, line, cpu);
    }
}

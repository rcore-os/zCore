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

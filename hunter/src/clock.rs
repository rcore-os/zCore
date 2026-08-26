//! Pluggable, tamper-resistant monotonic clock for timestamping security events.
//!
//! `hunter` is a `no_std` crate that must stay free of a hard dependency on
//! `kernel-hal` (it sits *below* the syscall layer in the build graph). The
//! kernel registers a time source at boot via [`set_time_source`]; until then
//! timestamps read as `0`, which the renderer prints harmlessly.
//!
//! Hardening (P12): the time source gates both the forensic log and the IDS
//! sliding windows, so it is a security-sensitive input. Registration is
//! therefore **sealed** — only the first call wins and a null pointer is
//! refused — preventing later code (or an attacker who reaches a mutator) from
//! swapping in a frozen / lying clock to silence detection. The heuristics
//! additionally apply a count-based window backstop so a stuck clock cannot
//! disable rate detection outright.

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// The kernel's monotonic clock (nanoseconds since boot), stored as an erased
/// pointer. `fn` pointers are guaranteed to fit in a data pointer here.
static TIME_SOURCE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
/// Set once the time source has been registered; further registrations are
/// rejected so the clock cannot be replaced at runtime.
static SEALED: AtomicBool = AtomicBool::new(false);

/// Registers the monotonic clock used to timestamp events, in nanoseconds
/// since boot. Only the first non-null registration takes effect.
pub fn set_time_source(f: fn() -> u64) {
    // Seal on first use: later calls are no-ops, so the clock cannot be
    // swapped out at runtime.
    if SEALED.swap(true, Ordering::SeqCst) {
        return;
    }
    TIME_SOURCE.store(f as *mut (), Ordering::SeqCst);
}

/// Returns `true` once a time source has been sealed in.
pub fn is_sealed() -> bool {
    SEALED.load(Ordering::Relaxed)
}

/// Current monotonic time in nanoseconds, or `0` if no source is registered.
pub fn now_ns() -> u64 {
    let p = TIME_SOURCE.load(Ordering::Acquire);
    if p.is_null() {
        return 0;
    }
    // SAFETY: `p` is non-null only after `set_time_source` sealed a valid
    // `fn() -> u64` pointer, which is never unloaded or replaced.
    let f: fn() -> u64 = unsafe { core::mem::transmute(p) };
    f()
}

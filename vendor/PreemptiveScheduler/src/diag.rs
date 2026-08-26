//! Deadlock-visible locking for the scheduler's `spin::Mutex`es.
//!
//! The runtime/task locks here are taken from timer-IRQ context on every tick,
//! so a deadlock involving them freezes every CPU with no panic and no console
//! output. The kernel's own `lock::Mutex` self-reports long spins through a
//! lock-free hook (painted straight onto the framebuffer); this helper gives
//! the scheduler's external `spin::Mutex`es the same behavior: spin with
//! `try_lock`, and after ~8s of continuous spinning report the stuck call site
//! (once) through `lock::report_stuck`, then keep spinning.

use spin::{Mutex, MutexGuard};

/// ~8s of PAUSE iterations — orders of magnitude beyond legitimate contention.
const DEADLOCK_SPINS: u64 = 1_000_000_000;

#[track_caller]
pub(crate) fn diag_lock<'a, T>(m: &'a Mutex<T>) -> MutexGuard<'a, T> {
    let caller = core::panic::Location::caller();
    let mut spins: u64 = 0;
    loop {
        if let Some(g) = m.try_lock() {
            return g;
        }
        core::hint::spin_loop();
        spins += 1;
        // These runtime/task locks are taken from timer-IRQ context (IRQs off),
        // so a CPU parked here is deaf to TLB-shootdown IPIs. A peer unmapping
        // memory spin-waits for our ack while holding the VMAR lock; without
        // this drain that peer wedges and every CPU behind the VMAR lock wedges
        // with it (observed as a multi-core vmar.rs DEADLOCK banner). Pump our
        // own shootdown queue at the same cadence the kernel's ticket lock does.
        if spins & 511 == 0 {
            lock::pump();
        }
        if spins == DEADLOCK_SPINS {
            lock::report_stuck(caller.file(), caller.line());
        }
    }
}

#![no_std]

/// Single source of truth for the size of every per-CPU array indexed by the
/// dense logical cpu id (this crate's `CPUS`, the scheduler's `GLOBAL_RUNTIME`,
/// kernel-hal's percpu storage).
pub const MAX_CORE_NUM: usize = 64;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "none", feature = "ticket"))] {
        extern crate alloc;
        mod interrupt;
        pub use interrupt::{current_cpu_id, lock_depth};
        #[cfg(any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "riscv32",
            target_arch = "riscv64"
        ))]
        pub use interrupt::set_logical_cpu_id;
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        pub use interrupt::{hardware_apic_id, set_phys_virt_offset, with_ap_boot_logical};
        pub mod mcslock;
        pub mod rwlock;
        pub use {rwlock::*, mcslock::*};
        mod deadlock;
        pub use deadlock::{
            report_stuck, set_deadlock_holder_hook, set_deadlock_hook, set_deadlock_spins,
            set_spin_pump,
        };
        pub mod ticket;
        pub use ticket::{TicketMutex as Mutex, TicketMutexGuard as MutexGuard};
    } else if #[cfg(target_os = "none")] {
        extern crate alloc;
        mod interrupt;
        pub use interrupt::{current_cpu_id, lock_depth};
        #[cfg(any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "riscv32",
            target_arch = "riscv64"
        ))]
        pub use interrupt::set_logical_cpu_id;
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        pub use interrupt::{hardware_apic_id, set_phys_virt_offset, with_ap_boot_logical};
        pub mod mcslock;
        pub mod rwlock;
        pub use {rwlock::*, mcslock::*};
        mod deadlock;
        pub use deadlock::{
            report_stuck, set_deadlock_holder_hook, set_deadlock_hook, set_deadlock_spins,
            set_spin_pump,
        };
        pub mod spin;
        pub use spin::{SpinMutex as Mutex, SpinMutexGuard as MutexGuard};
    } else {
        pub use spin::*;

        /// Hosted (libos) no-op twin of the bare-metal stuck-lock reporter.
        /// The preemptive executor's diagnostics call it unconditionally, and
        /// without it the whole libos build fails to compile; on a hosted
        /// target there is no interrupts-off spin to diagnose, so it does
        /// nothing.
        pub fn report_stuck(_file: &'static str, _line: u32) {}

        /// Hosted no-op: deadlock hooks only exist on bare-metal builds.
        pub fn set_deadlock_hook(_f: fn(&'static str, u32)) {}

        /// Hosted no-op twin of the holder-report hook installer.
        pub fn set_deadlock_holder_hook(_f: fn(usize, usize, u32, u32)) {}

        /// Hosted no-op: there is no interrupts-off spin to threshold.
        pub fn set_deadlock_spins(_spins: u64) {}

        /// Hosted no-op: no IRQs-off spins, so nothing to pump.
        pub fn set_spin_pump(_f: fn()) {}

        /// Hosted twin of the bare-metal lock-nesting depth. There is no
        /// `push_off` bookkeeping on a hosted target, so report 0 ("no kernel
        /// lock held") — the callers that gate on it are bare-metal only.
        pub fn lock_depth() -> i32 {
            0
        }

        /// Hosted twin of the dense logical cpu id. Host test builds (e.g.
        /// `cargo test -p linux-object`, which links zcore-drivers) run on one
        /// thread of a hosted OS; per-CPU diagnostics all collapse to slot 0.
        pub fn current_cpu_id() -> u8 {
            0
        }
    }
}

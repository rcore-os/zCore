//! CPU information.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use cortex_a::registers::*;
use tock_registers::interfaces::Readable;

use crate::config::MAX_CORE_NUM;

// ─── CPU topology: dense logical id  <->  MPIDR affinity ─────────────────────────
//
// MPIDR_EL1 affinity fields (Aff0..Aff3) are sparse: Aff0 repeats across clusters,
// so it cannot index per-CPU arrays. Each online CPU gets a dense logical id
// (0..NCPU, boot CPU = 0) which is stored in TPIDR_EL1 (read by `lock`/`cpu_id`).
// We keep the reverse map (logical -> affinity) for targeting GIC SGIs.

/// Number of logical ids assigned so far.
static LOGICAL_COUNT: AtomicU8 = AtomicU8::new(0);

/// logical id -> packed MPIDR affinity (Aff3:Aff2:Aff1:Aff0). Index 0 = boot CPU.
static LOGICAL_TO_AFFINITY: [AtomicU32; MAX_CORE_NUM] = [const { AtomicU32::new(0) }; MAX_CORE_NUM];

/// Packed MPIDR affinity (Aff3<<24 | Aff2<<16 | Aff1<<8 | Aff0) of the current CPU.
///
/// Use this — not [`cpu_id`](crate::cpu::cpu_id) — whenever a *hardware* CPU
/// identifier is required (PSCI `CPU_ON` target, GIC affinity routing).
pub fn raw_affinity() -> u32 {
    let mpidr = MPIDR_EL1.get();
    let aff0 = (mpidr & 0xff) as u32;
    let aff1 = ((mpidr >> 8) & 0xff) as u32;
    let aff2 = ((mpidr >> 16) & 0xff) as u32;
    let aff3 = ((mpidr >> 32) & 0xff) as u32;
    (aff3 << 24) | (aff2 << 16) | (aff1 << 8) | aff0
}

/// Assign this CPU its dense logical id, publish it in TPIDR_EL1, and record the
/// reverse (logical -> affinity) map. Called once per CPU from `percpu::register`.
pub fn register_logical_id() -> u8 {
    let logical = LOGICAL_COUNT.fetch_add(1, Ordering::AcqRel);
    if let Some(slot) = LOGICAL_TO_AFFINITY.get(logical as usize) {
        slot.store(raw_affinity(), Ordering::Release);
    }
    unsafe { core::arch::asm!("msr tpidr_el1, {0}", in(reg) logical as u64) };
    logical
}

/// Translate a dense logical CPU id back to its packed MPIDR affinity.
pub fn logical_to_affinity(logical: usize) -> u32 {
    LOGICAL_TO_AFFINITY
        .get(logical)
        .map(|a| a.load(Ordering::Acquire))
        .unwrap_or(0)
}

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            // Dense logical id (from TPIDR_EL1 via `lock`); see module docs.
            lock::current_cpu_id()
        }

        fn cpu_frequency() -> u16 {
            0
        }

        fn cpu_brand() -> alloc::string::String {
            alloc::string::String::from("AArch64 CPU")
        }

        fn cpu_count() -> u8 {
            LOGICAL_COUNT.load(Ordering::Acquire)
        }

        fn reset() -> ! {
            info!("shutdown...");
            let psci_system_off = 0x8400_0008_usize;
            unsafe {
                core::arch::asm!(
                    "hvc #0",
                    in("x0") psci_system_off
                );
            }
            unreachable!()
        }
    }
}

//! CPU information.
use core::sync::atomic::{AtomicU8, Ordering};

use crate::config::MAX_CORE_NUM;
use crate::utils::init_once::InitOnce;

pub(super) static CPU_FREQ_MHZ: InitOnce<u16> = InitOnce::new_with_default(1000); // 1GHz

// ─── CPU topology: dense logical id  <->  hart id ───────────────────────────────
//
// Hart ids (in `tp`) may be sparse (some boards reserve hart 0), so they cannot be
// used directly to index per-CPU arrays. Each online hart gets a dense logical id
// (0..NCPU, boot hart = 0). The forward map (hart -> logical) lives in `lock` so
// the lock crate and the kernel share one id space; here we keep the reverse map
// (logical -> hart) needed to target SBI IPIs.

/// Number of logical ids assigned so far.
static LOGICAL_COUNT: AtomicU8 = AtomicU8::new(0);

/// logical id -> hart id. Index 0 is the boot hart.
static LOGICAL_TO_HART: [AtomicU8; MAX_CORE_NUM] = [const { AtomicU8::new(0) }; MAX_CORE_NUM];

/// Raw hart id of the current CPU (kernel convention: stored in `tp`).
///
/// Use this — not [`cpu_id`](crate::cpu::cpu_id) — whenever a *hardware* hart id
/// is required (device-tree `riscv-intc-cpuN` nodes, PLIC contexts, SBI hart masks).
pub fn raw_hart_id() -> usize {
    let hart_id: usize;
    unsafe { core::arch::asm!("mv {0}, tp", out(reg) hart_id) };
    hart_id
}

/// Assign this hart its dense logical id and register the hart<->logical maps.
/// Called once per hart from `percpu::register`, before any lock-taking code.
pub fn register_logical_id() -> u8 {
    let hart_id = raw_hart_id() as u8;
    let logical = LOGICAL_COUNT.fetch_add(1, Ordering::AcqRel);
    if let Some(slot) = LOGICAL_TO_HART.get(logical as usize) {
        slot.store(hart_id, Ordering::Release);
    }
    lock::set_logical_cpu_id(hart_id as u32, logical);
    logical
}

/// Translate a dense logical CPU id back to its hart id (for SBI IPI delivery).
pub fn logical_to_hart(logical: usize) -> usize {
    LOGICAL_TO_HART
        .get(logical)
        .map(|h| h.load(Ordering::Acquire) as usize)
        .unwrap_or(logical)
}

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            // Dense logical id (0..NCPU), resolved from the sparse hart id via the
            // table in `lock`. Hart ids are not contiguous on all boards, so they
            // must not be used directly to index per-CPU arrays.
            lock::current_cpu_id()
        }

        fn cpu_frequency() -> u16 {
            *CPU_FREQ_MHZ
        }

        fn cpu_brand() -> alloc::string::String {
            alloc::string::String::from("RISC-V CPU")
        }

        fn cpu_count() -> u8 {
            LOGICAL_COUNT.load(Ordering::Acquire)
        }

        fn reset() -> ! {
            info!("shutdown...");
            sbi_rt::system_reset(sbi_rt::Shutdown, sbi_rt::NoReason);
            unreachable!()
        }
    }
}

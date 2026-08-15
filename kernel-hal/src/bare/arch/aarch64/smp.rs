//! aarch64 Symmetric Multi-Processing startup (PSCI `CPU_ON`).
//!
//! Brings up secondary cores via the PSCI `CPU_ON` SMC/HVC call. Secondaries
//! enter [`secondary_trampoline`] **with the MMU off**, at the trampoline's
//! *physical* address, with `x0` = physical pointer to a [`SecondaryContext`].
//! The trampoline restores the BSP's translation registers (so it shares the
//! kernel page table), enables the MMU, and jumps to the kernel's per-CPU entry
//! (`ap_fn`, i.e. `secondary_main`) on its own stack.
//!
//! MMU hand-off detail: while the MMU is being enabled, the PC still holds the
//! trampoline's *physical* address, so that address must be mapped. We therefore
//! build a TTBR0 table that identity-maps the trampoline page; after the `br` to
//! the high-half entry, execution runs purely out of the kernel (TTBR1) mapping.
//!
//! NOTE: this path has been validated to *compile* but not yet boot-tested on
//! QEMU; the MMU/PSCI hand-off is the likely place to debug if secondaries hang.

use core::sync::atomic::{AtomicUsize, Ordering};

use cortex_a::registers::*;
use tock_registers::interfaces::Readable;

use super::vm::PageTable;
use crate::{vm::GenericPageTable, MMUFlags, KCONFIG};

const PAGE_SIZE: usize = 4096;
const STACK_SIZE: usize = 256 * 1024;
// AP stacks are allocated via `Layout::from_size_align(STACK_SIZE, PAGE_SIZE)`,
// which requires the size to be a multiple of the (page) alignment.
const _: () = assert!(STACK_SIZE.is_multiple_of(PAGE_SIZE));

/// PSCI `CPU_ON` (SMC64) function id.
const PSCI_CPU_ON: u64 = 0xC400_0003;
/// PSCI return code for a non-existent target CPU.
const PSCI_INVALID_PARAMETERS: i64 = -2;
const PSCI_ALREADY_ON: i64 = -4;

/// Number of secondary CPUs that have signalled they are running.
pub static AP_ONLINE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// State handed to a secondary core (read by [`secondary_trampoline`] with the
/// MMU off, so it lives in identity-readable physical memory). Field order is
/// load-bearing — the trampoline reads it by fixed byte offsets.
#[repr(C)]
struct SecondaryContext {
    ttbr0: u64, // +0
    ttbr1: u64, // +8
    tcr: u64,   // +16
    mair: u64,  // +24
    sctlr: u64, // +32
    sp: u64,    // +40  (virtual stack top)
    entry: u64, // +48  (virtual kernel entry, ap_fn)
}

/// Physical address of a kernel virtual address.
fn virt_to_phys(va: usize) -> usize {
    va - KCONFIG.phys_to_virt_offset
}

/// Translation registers captured from the BSP, shared by every secondary.
struct TransRegs {
    ttbr0: u64,
    ttbr1: u64,
    tcr: u64,
    mair: u64,
    sctlr: u64,
}

/// Build a TTBR0 page table that identity-maps the trampoline code page so the PC
/// remains valid across the MMU-enable step. Leaked: it must outlive the bring-up.
fn build_identity_ttbr0(tramp_phys: usize) -> u64 {
    let mut pt = PageTable::new();
    let base = tramp_phys & !(PAGE_SIZE - 1);
    // Two pages, in case the trampoline straddles a page boundary.
    pt.map_cont(
        base,
        PAGE_SIZE * 2,
        base,
        MMUFlags::READ | MMUFlags::WRITE | MMUFlags::EXECUTE,
    )
    .expect("[smp] identity-map trampoline failed");
    let token = pt.table_phys() as u64;
    core::mem::forget(pt); // keep the table alive for the lifetime of the APs
    token
}

/// Start all secondary cores. Called once from the BSP `primary_init`, after the
/// kernel page table and GIC are initialised.
pub fn start_secondary_cores() {
    let max_aps = crate::config::MAX_CORE_NUM - 1;

    let tramp_phys = virt_to_phys(secondary_trampoline as *const () as usize);
    let regs = TransRegs {
        ttbr0: build_identity_ttbr0(tramp_phys),
        ttbr1: TTBR1_EL1.get(),
        tcr: TCR_EL1.get(),
        mair: MAIR_EL1.get(),
        sctlr: SCTLR_EL1.get(),
    };

    crate::klog_info!("[smp] starting secondary cores (PSCI CPU_ON)");

    let mut started = 0usize;
    // QEMU `virt` (single cluster) numbers cores by Aff0 = 0,1,2,...; the BSP is
    // affinity 0. Probe upward until PSCI reports the target does not exist.
    for aff in 1..=(max_aps as u64) {
        let stack_top = match alloc_stack() {
            Some(top) => top,
            None => {
                crate::klog_warn!("[smp] out of memory allocating AP stack");
                break;
            }
        };

        let ctx = alloc::boxed::Box::new(SecondaryContext {
            ttbr0: regs.ttbr0,
            ttbr1: regs.ttbr1,
            tcr: regs.tcr,
            mair: regs.mair,
            sctlr: regs.sctlr,
            sp: stack_top as u64,
            entry: KCONFIG.ap_fn as usize as u64,
        });
        // Kept as a Box until CPU_ON is known to have succeeded: the loop always
        // ends on a probe that fails (that is how the core count is discovered),
        // and leaking the stack + context of every failed probe threw away a
        // 256 KiB stack on every boot.
        let ctx_phys = virt_to_phys(ctx.as_ref() as *const _ as usize) as u64;

        // Fire CPU_ON and move on: the AP only proceeds past its `STARTED` gate
        // once the BSP finishes init, so waiting for it to come online here would
        // just stall boot. `ap_signal_online` tracks the real online count.
        let ret = unsafe { psci_cpu_on(aff, tramp_phys as u64, ctx_phys) };
        match ret {
            0 => {
                started += 1;
                // The secondary reads this context with the MMU off, long after
                // we return: it must outlive us. Same for its stack.
                let _ = alloc::boxed::Box::leak(ctx);
                crate::klog_info!("[smp] CPU_ON affinity {} -> ok", aff);
                continue;
            }
            PSCI_ALREADY_ON => crate::klog_warn!("[smp] affinity {} already on", aff),
            PSCI_INVALID_PARAMETERS => {
                free_stack(stack_top);
                break; // no more cores
            }
            other => crate::klog_warn!("[smp] CPU_ON affinity {} failed: {}", aff, other),
        }
        // Any non-success path: this core never started, so reclaim its stack
        // (the context Box drops here on its own).
        free_stack(stack_top);
    }

    crate::klog_info!("[smp] secondary bring-up done — {} CPU_ON issued", started);
}

/// Called by each secondary from `secondary_init` to announce it is running.
pub fn ap_signal_online() {
    // Join the online set, exactly as x86's `ap_signal_online` does. Without
    // this the mask stayed at "BSP only" forever on aarch64, so every consumer
    // of `cpu_online_mask()` / `online_cpu_count()` — the `/proc` accounting and
    // the affinity syscalls among them — reported a uniprocessor machine no
    // matter how many cores had actually come up.
    crate::common::ipi::mark_cpu_online(super::cpu::cpu_id() as usize);
    AP_ONLINE_COUNT.fetch_add(1, Ordering::Release);
}

fn stack_layout() -> alloc::alloc::Layout {
    alloc::alloc::Layout::from_size_align(STACK_SIZE, PAGE_SIZE).unwrap()
}

fn alloc_stack() -> Option<usize> {
    let base = unsafe { alloc::alloc::alloc_zeroed(stack_layout()) };
    if base.is_null() {
        None
    } else {
        Some(base as usize + STACK_SIZE)
    }
}

/// Give back a stack from [`alloc_stack`] whose core never started.
fn free_stack(stack_top: usize) {
    let base = (stack_top - STACK_SIZE) as *mut u8;
    unsafe { alloc::alloc::dealloc(base, stack_layout()) };
}

/// PSCI `CPU_ON` via the HVC conduit (QEMU `virt` uses HVC, as does `reset`).
unsafe fn psci_cpu_on(target: u64, entry: u64, context: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "hvc #0",
        inout("x0") PSCI_CPU_ON => ret,
        in("x1") target,
        in("x2") entry,
        in("x3") context,
        out("x4") _,
        out("x5") _,
        out("x6") _,
        out("x7") _,
        options(nostack),
    );
    ret
}

/// Secondary entry, reached via PSCI with the MMU **off** and `x0` = physical
/// `*const SecondaryContext`. Restores the BSP translation regs, enables the MMU,
/// and jumps to the kernel entry on the AP stack. Position-independent: it must
/// not reference any absolute (high-half) symbol before the MMU is on.
#[unsafe(naked)]
unsafe extern "C" fn secondary_trampoline() -> ! {
    core::arch::naked_asm!(
        "ldr x1, [x0, #0]", // ttbr0 (identity table)
        "msr ttbr0_el1, x1",
        "ldr x1, [x0, #8]", // ttbr1 (kernel table)
        "msr ttbr1_el1, x1",
        "ldr x1, [x0, #16]", // tcr
        "msr tcr_el1, x1",
        "ldr x1, [x0, #24]", // mair
        "msr mair_el1, x1",
        // Load stack/entry (virtual) into callee regs before the MMU is on, while
        // the physical context pointer in x0 is still valid.
        "ldr x9, [x0, #40]",  // sp
        "ldr x10, [x0, #48]", // entry
        "dsb sy",
        "isb",
        "tlbi vmalle1",
        "dsb sy",
        "isb",
        "ldr x1, [x0, #32]", // sctlr (with M/C/I set) -> enable MMU
        "msr sctlr_el1, x1",
        "isb",
        "mov sp, x9",
        "br x10",
    )
}

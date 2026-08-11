//! IA32_PAT programming and the write-combining retrofit for the framebuffer.
//!
//! Out of reset the PAT holds `[WB, WT, UC-, UC, WB, WT, UC-, UC]`, and the
//! page-table flag conversion in `vm.rs` used to emit plain `PCD|PWT` (PAT
//! index 3 = UC) for *every* non-cached policy — including
//! [`CachePolicy::WriteCombining`], whose PAT bit was never set and whose PAT
//! entry never existed. Net effect: the GOP framebuffer (reached through the
//! physmap alias, which rboot maps with no cache attribute at all → WB, or
//! through VMOs requesting WC → UC) was blitted through **uncached** stores,
//! where every 8-byte write is its own serialized bus transaction. That is the
//! difference between ~50-100 MB/s and multiple GB/s of blit throughput, paid
//! on every console line and every compositor frame.
//!
//! Two pieces fix it:
//! - [`init_this_cpu`] rewrites PAT entry 7 (selected by `PAT|PCD|PWT`) from
//!   UC to WC. Entry 7 is chosen because nothing can currently reference it
//!   (no code sets the PTE PAT bit), so redefining it cannot change the type
//!   of any existing mapping. Runs on the BSP and on every AP — the PAT MSR
//!   is per-core and Intel requires it consistent across cores.
//! - [`enable_framebuffer_wc`] walks the live boot page table and flips the
//!   4 KiB PTEs covering the framebuffer's physmap alias to `PAT|PCD|PWT`
//!   (index 7 = WC). rboot maps the physmap exclusively with 4 KiB pages
//!   (`rboot/src/page_table.rs`, `Mapper<Size4KiB>`), and every process page
//!   table shares the kernel-half PDPTs by PML4E cloning
//!   (`pt_clone_kernel_space`), so a single in-place edit propagates to every
//!   address space. The range was UC/WB-but-only-written before, so no cache
//!   flush is needed beyond `invlpg`.
//!
//! New mappings that ask for [`CachePolicy::WriteCombining`] get the PAT bit
//! from `X86PTE::set_flags` in `vm.rs` once [`pat_wc_ready`] reports true.

use core::sync::atomic::{AtomicBool, Ordering};

use x86_64::instructions::tlb;
use x86_64::registers::model_specific::Msr;

use crate::mem::phys_to_virt;
use crate::KCONFIG;

const IA32_PAT: u32 = 0x277;
/// Memory-type encodings for PAT entries (Intel SDM vol. 3A, table 11-10).
const PAT_TYPE_WC: u64 = 0x01;

/// Set once the BSP has programmed a WC entry into its PAT; APs replicate the
/// same value before they can touch any WC mapping. Read by `vm.rs` so the
/// WriteCombining PAT bit is only ever emitted when entry 7 really is WC.
static PAT_WC_READY: AtomicBool = AtomicBool::new(false);

/// Whether PAT entry 7 has been redefined to write-combining.
#[inline]
pub fn pat_wc_ready() -> bool {
    PAT_WC_READY.load(Ordering::Acquire)
}

/// Program PAT entry 7 = WC on the calling CPU. Idempotent, per-core.
///
/// Entry 7 is unreferenced until `vm.rs`/`enable_framebuffer_wc` start
/// emitting the PTE PAT bit, so this rewrite cannot retype an existing
/// mapping and needs none of the SDM's cache-disable ceremony.
pub fn init_this_cpu() {
    let mut msr = Msr::new(IA32_PAT);
    // SAFETY: IA32_PAT exists on every x86_64 CPU this kernel supports
    // (P4+/all 64-bit parts); rewriting an unused entry is side-effect free.
    unsafe {
        let old = msr.read();
        let new = (old & !(0xffu64 << 56)) | (PAT_TYPE_WC << 56);
        if old != new {
            msr.write(new);
        }
    }
    PAT_WC_READY.store(true, Ordering::Release);
}

const PTE_PWT: u64 = 1 << 3;
const PTE_PCD: u64 = 1 << 4;
/// PAT bit position in a 4 KiB PTE (bit 7; in PDE/PDPTE leaves bit 7 is PS
/// and the PAT bit moves to bit 12).
const PTE_PAT_4K: u64 = 1 << 7;
const PTE_PRESENT: u64 = 1 << 0;
const PTE_HUGE: u64 = 1 << 7;
const PHYS_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Flip the leaf PTEs covering the framebuffer's physmap alias to PAT
/// entry 7 (WC). Call on the BSP, after [`init_this_cpu`], before the
/// graphic console starts pushing frames. Safe to call again (idempotent).
///
/// Only 4 KiB leaves are converted; a huge-page leaf (which rboot never
/// creates for the physmap) is left untouched — UC/WB there is slower, not
/// incorrect — and reported once.
pub fn enable_framebuffer_wc() {
    if !pat_wc_ready() || KCONFIG.fb_addr == 0 || KCONFIG.fb_size == 0 {
        return;
    }
    let root = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64() as usize;
    let va_base = phys_to_virt(KCONFIG.fb_addr as usize);
    let pages = (KCONFIG.fb_size as usize).div_ceil(4096);
    let mut converted = 0usize;
    let mut skipped_huge = 0usize;
    for i in 0..pages {
        let va = va_base + i * 4096;
        match convert_pte_to_wc(root, va) {
            PteConvert::Converted => {
                tlb::flush(x86_64::VirtAddr::new(va as u64));
                converted += 1;
            }
            PteConvert::AlreadyWc => {}
            PteConvert::HugeLeaf => skipped_huge += 1,
            PteConvert::NotMapped => {}
        }
    }
    if converted > 0 || skipped_huge > 0 {
        log::warn!(
            "pat: framebuffer {:#x}..{:#x} write-combining ({} PTEs converted{})",
            KCONFIG.fb_addr,
            KCONFIG.fb_addr + KCONFIG.fb_size,
            converted,
            if skipped_huge > 0 {
                ", huge leaves skipped"
            } else {
                ""
            },
        );
    }
}

enum PteConvert {
    Converted,
    AlreadyWc,
    HugeLeaf,
    NotMapped,
}

/// Walk the 4-level tree for `va` and set `PAT|PCD|PWT` on its 4 KiB PTE.
fn convert_pte_to_wc(root: usize, va: usize) -> PteConvert {
    let idx = |level: usize| (va >> (12 + 9 * level)) & 0x1ff;
    let mut table_pa = root;
    // Levels 3..1 (PML4, PDPT, PD): descend, refusing huge leaves.
    for level in (1..=3).rev() {
        // SAFETY: the physmap covers all page-table frames; entries are u64.
        let entry = unsafe {
            core::ptr::read_volatile(
                (phys_to_virt(table_pa) as *const u64).add(idx(level)),
            )
        };
        if entry & PTE_PRESENT == 0 {
            return PteConvert::NotMapped;
        }
        if level < 3 && entry & PTE_HUGE != 0 {
            return PteConvert::HugeLeaf;
        }
        table_pa = (entry & PHYS_ADDR_MASK) as usize;
    }
    let pte_ptr = unsafe { (phys_to_virt(table_pa) as *mut u64).add(idx(0)) };
    // SAFETY: `pte_ptr` addresses a live PTE through the physmap.
    unsafe {
        let pte = core::ptr::read_volatile(pte_ptr);
        if pte & PTE_PRESENT == 0 {
            return PteConvert::NotMapped;
        }
        let wc = pte | PTE_PWT | PTE_PCD | PTE_PAT_4K;
        if wc == pte {
            return PteConvert::AlreadyWc;
        }
        core::ptr::write_volatile(pte_ptr, wc);
    }
    PteConvert::Converted
}

use alloc::vec::Vec;
use core::arch::x86_64::{__cpuid, _mm_clflush, _mm_mfence};
use core::ops::Range;

use uefi::table::boot::MemoryType;

use crate::{mem::phys_to_virt, PhysAddr, KCONFIG, PAGE_SIZE};

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    KCONFIG
        .memory_map
        .iter()
        .filter_map(|r| {
            if r.ty == MemoryType::CONVENTIONAL {
                let start = r.phys_start as usize;
                let end = start + r.page_count as usize * PAGE_SIZE;
                Some(start..end)
            } else {
                None
            }
        })
        .collect()
}

// Get cache line size in bytes.
fn cacheline_size() -> usize {
    let leaf = unsafe { __cpuid(1).ebx };
    (((leaf >> 8) & 0xff) << 3) as usize
}

/// Flush the physical frame.
pub fn frame_flush(target: PhysAddr) {
    unsafe {
        for paddr in (target..target + PAGE_SIZE).step_by(cacheline_size()) {
            _mm_clflush(phys_to_virt(paddr) as *const u8);
        }
        _mm_mfence();
    }
}

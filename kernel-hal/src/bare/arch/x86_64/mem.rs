use core::arch::x86_64::{__cpuid, _mm_clflush, _mm_mfence};

use super::super::mem::phys_to_virt;
use crate::{PhysAddr, PAGE_SIZE};

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

use alloc::vec::Vec;
use core::ops::Range;

use crate::{addr::align_up, PhysAddr, KCONFIG, PAGE_SIZE};

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    extern "C" {
        fn end();
    }
    let start = align_up(end as usize + PAGE_SIZE) - KCONFIG.phys_to_virt_offset;
    // TODO: get physical memory end from device tree.
    alloc::vec![start..KCONFIG.phys_mem_end]
}

pub fn frame_flush(_target: crate::PhysAddr) {
    unimplemented!()
}

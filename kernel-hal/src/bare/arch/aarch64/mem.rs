use core::ops::Range;
use crate::PhysAddr;
use alloc::vec::Vec;
use crate::imp::config::*;

extern "C" {
    fn ekernel();
}

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    let mut regions = Vec::new();
    let start = ekernel as usize & PHYS_ADDR_MASK;
    regions.push(start as PhysAddr..PHYS_MEMORY_END as PhysAddr);
    regions
}

/// Flush the physical frame.
pub fn frame_flush(_target: PhysAddr) {
    unimplemented!()
}
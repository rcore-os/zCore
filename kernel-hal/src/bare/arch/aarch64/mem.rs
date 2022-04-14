use core::ops::Range;
use crate::PhysAddr;
use alloc::vec::Vec;

extern "C" {
    fn ekernel();
}

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    let mut regions = Vec::new();
    let start = ekernel as usize & 0x0000_ffff_ffff_ffff;
    regions.push(start as PhysAddr..(start + 16 * 1024 * 1024) as PhysAddr);
    regions
}

/// Flush the physical frame.
pub fn frame_flush(_target: PhysAddr) {
    unimplemented!()
}
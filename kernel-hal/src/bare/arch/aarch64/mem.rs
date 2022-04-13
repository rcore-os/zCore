use core::ops::Range;
use crate::PhysAddr;
use alloc::vec::Vec;

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    unimplemented!()
}

/// Flush the physical frame.
pub fn frame_flush(_target: PhysAddr) {
    unimplemented!()
}
use crate::PhysAddr;

pub use crate::common::memory::*;

/// Read physical memory from `paddr` to `buf`.
pub fn pmem_read(_paddr: PhysAddr, _buf: &mut [u8]) {
    unimplemented!()
}

/// Write physical memory to `paddr` from `buf`.
pub fn pmem_write(_paddr: PhysAddr, _buf: &[u8]) {
    unimplemented!()
}

/// Zero physical memory at `[paddr, paddr + len)`.
pub fn pmem_zero(_paddr: PhysAddr, _len: usize) {
    unimplemented!()
}

/// Copy content of `src` frame to `target` frame.
pub fn frame_copy(_src: PhysAddr, _target: PhysAddr) {
    unimplemented!()
}

/// Flush the physical frame.
pub fn frame_flush(_target: PhysAddr) {
    unimplemented!()
}

pub fn frame_alloc() -> Option<PhysAddr> {
    unimplemented!()
}

pub fn frame_alloc_contiguous(_size: usize, _align_log2: usize) -> Option<PhysAddr> {
    unimplemented!()
}

pub fn frame_dealloc(_paddr: PhysAddr) {
    unimplemented!()
}

pub fn zero_frame_addr() -> PhysAddr {
    unimplemented!()
}

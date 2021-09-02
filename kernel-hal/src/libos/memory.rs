use super::mem_common::{ensure_mmap_pmem, phys_to_virt, AVAILABLE_FRAMES, PMEM_SIZE};
use crate::{PhysAddr, PAGE_SIZE};

pub use crate::common::memory::*;

/// Read physical memory from `paddr` to `buf`.
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    trace!("pmem read: paddr={:#x}, len={:#x}", paddr, buf.len());
    assert!(paddr + buf.len() <= PMEM_SIZE);
    ensure_mmap_pmem();
    unsafe {
        (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
    assert!(paddr + buf.len() <= PMEM_SIZE);
    ensure_mmap_pmem();
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
    }
}

/// Zero physical memory at `[paddr, paddr + len)`
pub fn pmem_zero(paddr: PhysAddr, len: usize) {
    trace!("pmem_zero: addr={:#x}, len={:#x}", paddr, len);
    assert!(paddr + len <= PMEM_SIZE);
    ensure_mmap_pmem();
    unsafe {
        core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, len);
    }
}

/// Copy content of `src` frame to `target` frame
pub fn frame_copy(src: PhysAddr, target: PhysAddr) {
    trace!("frame_copy: {:#x} <- {:#x}", target, src);
    assert!(src + PAGE_SIZE <= PMEM_SIZE && target + PAGE_SIZE <= PMEM_SIZE);
    ensure_mmap_pmem();
    unsafe {
        let buf = phys_to_virt(src) as *const u8;
        buf.copy_to_nonoverlapping(phys_to_virt(target) as _, PAGE_SIZE);
    }
}

pub fn frame_flush(_target: PhysAddr) {
    // do nothing
}

pub fn frame_alloc() -> Option<PhysAddr> {
    let ret = AVAILABLE_FRAMES.lock().unwrap().pop_front();
    trace!("frame alloc: {:?}", ret);
    ret
}

pub fn frame_alloc_contiguous(_size: usize, _align_log2: usize) -> Option<PhysAddr> {
    unimplemented!()
}

pub fn frame_dealloc(paddr: PhysAddr) {
    trace!("frame dealloc: {:?}", paddr);
    AVAILABLE_FRAMES.lock().unwrap().push_back(paddr);
}

pub fn zero_frame_addr() -> PhysAddr {
    0
}

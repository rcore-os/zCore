use alloc::vec::Vec;

use crate::{PhysAddr, PAGE_SIZE};

#[repr(C)]
#[derive(Debug)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    pub fn alloc() -> Option<Self> {
        crate::memory::frame_alloc().map(|paddr| Self { paddr })
    }

    pub fn alloc_contiguous_base(size: usize, align_log2: usize) -> Option<PhysAddr> {
        crate::memory::frame_alloc_contiguous(size, align_log2)
    }

    pub fn alloc_contiguous(size: usize, align_log2: usize) -> Vec<Self> {
        Self::alloc_contiguous_base(size, align_log2).map_or(Vec::new(), |base| {
            (0..size)
                .map(|i| PhysFrame {
                    paddr: base + i * PAGE_SIZE,
                })
                .collect()
        })
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }

    pub fn zero_frame_addr() -> PhysAddr {
        crate::memory::zero_frame_addr()
    }
}

impl Drop for PhysFrame {
    fn drop(&mut self) {
        crate::memory::frame_dealloc(self.paddr)
    }
}

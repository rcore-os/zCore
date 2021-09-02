use alloc::vec::Vec;

use crate::{PhysAddr, PAGE_SIZE};

#[repr(C)]
#[derive(Debug)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    /// # Safety
    ///
    /// This function is unsafe because the user must ensure that this is an available physical
    /// frame.
    pub unsafe fn from_paddr(paddr: PhysAddr) -> Self {
        assert!(crate::addr::is_aligned(paddr));
        Self { paddr }
    }

    pub fn alloc() -> Option<Self> {
        crate::mem::frame_alloc().map(|paddr| Self { paddr })
    }

    fn alloc_contiguous_base(size: usize, align_log2: usize) -> Option<PhysAddr> {
        crate::mem::frame_alloc_contiguous(size, align_log2)
    }

    pub fn alloc_contiguous(size: usize, align_log2: usize) -> Vec<Self> {
        Self::alloc_contiguous_base(size, align_log2).map_or(Vec::new(), |base| {
            (0..size)
                .map(|i| Self {
                    paddr: base + i * PAGE_SIZE,
                })
                .collect()
        })
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }

    pub fn zero_frame_addr() -> PhysAddr {
        crate::mem::zero_frame_addr()
    }
}

impl Drop for PhysFrame {
    fn drop(&mut self) {
        crate::mem::frame_dealloc(self.paddr)
    }
}

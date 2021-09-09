use alloc::vec::Vec;

use crate::{PhysAddr, PAGE_SIZE};

#[repr(C)]
#[derive(Debug)]
pub struct PhysFrame {
    paddr: PhysAddr,
    allocated: bool,
}

impl PhysFrame {
    /// Allocate one physical frame.
    pub fn new() -> Option<Self> {
        crate::mem::frame_alloc().map(|paddr| Self {
            paddr,
            allocated: true,
        })
    }

    /// Allocate one physical frame and fill with zero.
    pub fn new_zero() -> Option<Self> {
        Self::new().map(|mut f| {
            f.zero();
            f
        })
    }

    fn alloc_contiguous_base(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
        crate::mem::frame_alloc_contiguous(frame_count, align_log2)
    }

    /// Allocate contiguous physical frames.
    pub fn new_contiguous(frame_count: usize, align_log2: usize) -> Vec<Self> {
        Self::alloc_contiguous_base(frame_count, align_log2).map_or(Vec::new(), |base| {
            (0..frame_count)
                .map(|i| Self {
                    paddr: base + i * PAGE_SIZE,
                    allocated: true,
                })
                .collect()
        })
    }

    /// # Safety
    ///
    /// This function is unsafe because the user must ensure that this is an available physical
    /// frame.
    pub unsafe fn from_paddr(paddr: PhysAddr) -> Self {
        assert!(crate::addr::is_aligned(paddr));
        Self {
            paddr,
            allocated: false,
        }
    }

    /// Get the start physical address of this frame.
    pub fn paddr(&self) -> PhysAddr {
        self.paddr
    }

    /// Fill `self` with zero.
    pub fn zero(&mut self) {
        crate::mem::pmem_zero(self.paddr, PAGE_SIZE);
    }
}

impl Drop for PhysFrame {
    fn drop(&mut self) {
        crate::mem::frame_dealloc(self.paddr)
    }
}

lazy_static::lazy_static! {
    /// The global physical frame contains all zeros.
    pub static ref ZERO_FRAME: PhysFrame = PhysFrame::new_zero().expect("failed to alloc zero frame");
}

use bitmap_allocator::BitAlloc;

use super::mem::FRAME_ALLOCATOR;
use crate::kernel_handler::{DummyKernelHandler, KernelHandler};
use crate::{PhysAddr, PAGE_SIZE};

impl KernelHandler for DummyKernelHandler {
    fn frame_alloc(&self) -> Option<PhysAddr> {
        let ret = FRAME_ALLOCATOR.lock().alloc().map(|id| id * PAGE_SIZE);
        trace!("Allocate frame: {:x?}", ret);
        ret
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        let ret = FRAME_ALLOCATOR
            .lock()
            .alloc_contiguous(frame_count, align_log2)
            .map(|id| id * PAGE_SIZE);
        trace!(
            "Allocate contiguous frames: {:x?} ~ {:x?}",
            ret,
            ret.map(|x| x + frame_count)
        );
        ret
    }

    fn frame_dealloc(&self, paddr: PhysAddr) {
        trace!("Deallocate frame: {:x}", paddr);
        FRAME_ALLOCATOR.lock().dealloc(paddr / PAGE_SIZE);
    }
}

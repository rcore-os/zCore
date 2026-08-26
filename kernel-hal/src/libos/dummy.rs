use bitmap_allocator::BitAlloc;

use super::mem::FRAME_ALLOCATOR;
use crate::kernel_handler::{DummyKernelHandler, KernelHandler};
use crate::{PhysAddr, PAGE_SIZE};

impl KernelHandler for DummyKernelHandler {
    fn frame_alloc(&self) -> Option<PhysAddr> {
        let ret = FRAME_ALLOCATOR.lock().alloc().map(|id| id * PAGE_SIZE);
        if ret.is_some() {
            super::mem::USED_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        trace!("Allocate frame: {:x?}", ret);
        ret
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        let ret = FRAME_ALLOCATOR
            .lock()
            .alloc_contiguous(frame_count, align_log2)
            .map(|id| id * PAGE_SIZE);
        if ret.is_some() {
            super::mem::USED_PAGES.fetch_add(frame_count, core::sync::atomic::Ordering::Relaxed);
        }
        trace!(
            "Allocate contiguous frames: {:x?} ~ {:x?}",
            ret,
            ret.map(|x| x + frame_count * PAGE_SIZE)
        );
        ret
    }

    fn frame_dealloc(&self, paddr: PhysAddr) {
        trace!("Deallocate frame: {:x}", paddr);
        FRAME_ALLOCATOR.lock().dealloc(paddr / PAGE_SIZE);
        super::mem::USED_PAGES.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
}

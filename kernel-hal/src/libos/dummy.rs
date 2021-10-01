use super::mem_common::AVAILABLE_FRAMES;
use crate::kernel_handler::{DummyKernelHandler, KernelHandler};
use crate::PhysAddr;

impl KernelHandler for DummyKernelHandler {
    fn frame_alloc(&self) -> Option<PhysAddr> {
        let ret = AVAILABLE_FRAMES.lock().unwrap().pop_front();
        trace!("frame alloc: {:?}", ret);
        ret
    }

    fn frame_dealloc(&self, paddr: PhysAddr) {
        trace!("frame dealloc: {:?}", paddr);
        AVAILABLE_FRAMES.lock().unwrap().push_back(paddr);
    }
}

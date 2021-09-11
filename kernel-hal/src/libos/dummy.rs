use super::mem_common::AVAILABLE_FRAMES;
use crate::{KernelHandler, PhysAddr};

impl KernelHandler for crate::DummyKernelHandler {
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

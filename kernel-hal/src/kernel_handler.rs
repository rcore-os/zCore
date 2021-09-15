//! Handlers implemented in kernel and called by HAL.

use crate::{utils::init_once::InitOnce, MMUFlags, PhysAddr, VirtAddr};

pub trait KernelHandler: Send + Sync + 'static {
    /// Allocate one physical frame.
    fn frame_alloc(&self) -> Option<PhysAddr> {
        unimplemented!()
    }

    /// Allocate contiguous `frame_count` physical frames.
    fn frame_alloc_contiguous(&self, _frame_count: usize, _align_log2: usize) -> Option<PhysAddr> {
        unimplemented!()
    }

    /// Deallocate a physical frame.
    fn frame_dealloc(&self, _paddr: PhysAddr) {
        unimplemented!()
    }

    /// Handle kernel mode page fault.
    fn handle_page_fault(&self, _fault_vaddr: VirtAddr, _access_flags: MMUFlags) {
        // do nothing
    }
}

#[allow(dead_code)]
pub(crate) struct DummyKernelHandler;

#[cfg(feature = "libos")]
pub(crate) static KHANDLER: InitOnce<&dyn KernelHandler> =
    InitOnce::new_with_default(&DummyKernelHandler);

#[cfg(not(feature = "libos"))]
pub(crate) static KHANDLER: InitOnce<&dyn KernelHandler> = InitOnce::new();

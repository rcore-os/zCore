use kernel_hal::{KernelHandler, MMUFlags};
use zircon_object::task::Thread;

use super::memory;

pub struct ZcoreKernelHandler;

impl KernelHandler for ZcoreKernelHandler {
    fn frame_alloc(&self) -> Option<usize> {
        memory::frame_alloc()
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        memory::frame_alloc_contiguous(frame_count, align_log2)
    }

    fn frame_dealloc(&self, paddr: usize) {
        memory::frame_dealloc(paddr)
    }

    fn handle_page_fault(&self, fault_vaddr: usize, access_flags: MMUFlags) {
        let any = kernel_hal::thread::get_current_thread().unwrap();
        let thread = any.downcast::<Thread>().unwrap();
        let vmar = thread.proc().vmar();
        if let Err(err) = vmar.handle_page_fault(fault_vaddr, access_flags) {
            panic!(
                "handle kernel page fault error: {:?} vaddr(0x{:x}) flags({:?})",
                err, fault_vaddr, access_flags
            );
        }
    }
}

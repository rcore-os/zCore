use super::ffi;
use crate::{PhysAddr, VirtAddr, PAGE_SIZE};

hal_fn_impl! {
    impl mod crate::defs::mem {
        fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
            unsafe { ffi::PHYS_TO_VIRT_OFFSET + paddr }
        }

        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
            trace!("pmem_read: addr={:#x}, len={:#x}", paddr, buf.len());
            unsafe {
                (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
            }
        }

        fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
            trace!(
                "pmem_write: addr={:#x}, len={:#x}, vaddr = {:#x}",
                paddr,
                buf.len(),
                phys_to_virt(paddr)
            );
            unsafe {
                buf.as_ptr()
                    .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
            }
        }

        fn pmem_zero(paddr: PhysAddr, len: usize) {
            trace!("pmem_zero: addr={:#x}, len={:#x}", paddr, len);
            unsafe {
                core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, len);
            }
        }

        fn frame_copy(src: PhysAddr, target: PhysAddr) {
            trace!("frame_copy: {:#x} <- {:#x}", target, src);
            unsafe {
                let buf = phys_to_virt(src) as *const u8;
                buf.copy_to_nonoverlapping(phys_to_virt(target) as _, PAGE_SIZE);
            }
        }

        fn frame_flush(target: PhysAddr) {
            super::arch::mem::frame_flush(target)
        }

        fn frame_alloc() -> Option<PhysAddr> {
            unsafe { ffi::hal_frame_alloc() }
        }

        fn frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
            unsafe { ffi::hal_frame_alloc_contiguous(frame_count, align_log2) }
        }

        fn frame_dealloc(paddr: PhysAddr) {
            unsafe { ffi::hal_frame_dealloc(paddr) }
        }
    }
}

use super::mem_common::{ensure_mmap_pmem, phys_to_virt, AVAILABLE_FRAMES, PMEM_SIZE};
use crate::{PhysAddr, PAGE_SIZE};

hal_fn_impl! {
    impl mod crate::defs::mem {
        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
            trace!("pmem read: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            ensure_mmap_pmem();
            unsafe {
                (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
            }
        }

        fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
            trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            ensure_mmap_pmem();
            unsafe {
                buf.as_ptr()
                    .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
            }
        }

        fn pmem_zero(paddr: PhysAddr, len: usize) {
            trace!("pmem_zero: addr={:#x}, len={:#x}", paddr, len);
            assert!(paddr + len <= PMEM_SIZE);
            ensure_mmap_pmem();
            unsafe {
                core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, len);
            }
        }

        fn frame_copy(src: PhysAddr, target: PhysAddr) {
            trace!("frame_copy: {:#x} <- {:#x}", target, src);
            assert!(src + PAGE_SIZE <= PMEM_SIZE && target + PAGE_SIZE <= PMEM_SIZE);
            ensure_mmap_pmem();
            unsafe {
                let buf = phys_to_virt(src) as *const u8;
                buf.copy_to_nonoverlapping(phys_to_virt(target) as _, PAGE_SIZE);
            }
        }

        fn frame_flush(_target: PhysAddr) {
            // do nothing
        }

        fn frame_alloc() -> Option<PhysAddr> {
            let ret = AVAILABLE_FRAMES.lock().unwrap().pop_front();
            trace!("frame alloc: {:?}", ret);
            ret
        }

        fn frame_alloc_contiguous(_frame_count: usize, _align_log2: usize) -> Option<PhysAddr> {
            unimplemented!()
        }

        fn frame_dealloc(paddr: PhysAddr) {
            trace!("frame dealloc: {:?}", paddr);
            AVAILABLE_FRAMES.lock().unwrap().push_back(paddr);
        }

        fn zero_frame_addr() -> PhysAddr {
            0
        }
    }
}

use super::mem_common::{ensure_mmap_pmem, AVAILABLE_FRAMES, PMEM_BASE, PMEM_SIZE};
use crate::{PhysAddr, VirtAddr};

hal_fn_impl! {
    impl mod crate::defs::mem {
        fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
            PMEM_BASE + paddr
        }

        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
            trace!("pmem read: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            ensure_mmap_pmem();
            let src = phys_to_virt(paddr) as _;
            unsafe { buf.as_mut_ptr().copy_from_nonoverlapping(src, buf.len()) };
        }

        fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
            trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            ensure_mmap_pmem();
            let dst = phys_to_virt(paddr) as *mut u8;
            unsafe { dst.copy_from_nonoverlapping(buf.as_ptr(), buf.len()) };
        }

        fn pmem_zero(paddr: PhysAddr, len: usize) {
            trace!("pmem_zero: addr={:#x}, len={:#x}", paddr, len);
            assert!(paddr + len <= PMEM_SIZE);
            ensure_mmap_pmem();
            unsafe { core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, len) };
        }

        fn pmem_copy(dst: PhysAddr, src: PhysAddr, len: usize) {
            trace!("pmem_copy: {:#x} <- {:#x}, len={:#x}", dst, src, len);
            assert!(src + len <= PMEM_SIZE && dst + len <= PMEM_SIZE);
            ensure_mmap_pmem();
            let dst = phys_to_virt(dst) as *mut u8;
            unsafe { dst.copy_from_nonoverlapping(phys_to_virt(src) as _, len) };
        }

        fn frame_flush(_target: PhysAddr) {
            // do nothing
        }

        fn frame_alloc() -> Option<PhysAddr> {
            let ret = AVAILABLE_FRAMES.lock().unwrap().pop_front();
            trace!("frame alloc: {:?}", ret);
            ret
        }

        fn frame_dealloc(paddr: PhysAddr) {
            trace!("frame dealloc: {:?}", paddr);
            AVAILABLE_FRAMES.lock().unwrap().push_back(paddr);
        }
    }
}

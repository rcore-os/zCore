use alloc::vec::Vec;
use core::ops::Range;

use crate::{PhysAddr, VirtAddr, KCONFIG};

hal_fn_impl! {
    impl mod crate::hal_fn::mem {
        fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
            KCONFIG.phys_to_virt_offset + paddr
        }

        fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
            super::arch::mem::free_pmem_regions()
        }

        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
            trace!("pmem_read: paddr={:#x}, len={:#x}", paddr, buf.len());
            let src = phys_to_virt(paddr) as _;
            unsafe { buf.as_mut_ptr().copy_from_nonoverlapping(src, buf.len()) };
        }

        fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
            trace!("pmem_write: paddr={:#x}, len={:#x}", paddr, buf.len());
            let dst = phys_to_virt(paddr) as *mut u8;
            unsafe { dst.copy_from_nonoverlapping(buf.as_ptr(), buf.len()) };
        }

        fn pmem_zero(paddr: PhysAddr, len: usize) {
            trace!("pmem_zero: paddr={:#x}, len={:#x}", paddr, len);
            unsafe { core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, len) };
        }

        fn pmem_copy(dst: PhysAddr, src: PhysAddr, len: usize) {
            trace!("pmem_copy: {:#x} <- {:#x}, len={:#x}", dst, src, len);
            let dst = phys_to_virt(dst) as *mut u8;
            unsafe { dst.copy_from_nonoverlapping(phys_to_virt(src) as _, len) };
        }

        fn frame_flush(target: PhysAddr) {
            super::arch::mem::frame_flush(target)
        }
    }
}

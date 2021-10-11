use bitmap_allocator::BitAlloc;
use spin::Mutex;

use super::mock_mem::MockMemory;
use crate::{MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

type FrameAlloc = bitmap_allocator::BitAlloc1M;

/// Map physical memory from here.
pub(super) const PMEM_BASE: VirtAddr = 0x8_0000_0000;
/// Physical memory size = 1GiB
pub(super) const PMEM_SIZE: usize = 0x4000_0000;

lazy_static! {
    pub(super) static ref FRAME_ALLOCATOR: Mutex<FrameAlloc> = {
        let mut allocator = FrameAlloc::DEFAULT;
        allocator.insert(1..PMEM_SIZE / PAGE_SIZE);
        Mutex::new(allocator)
    };
    pub(super) static ref MOCK_PHYS_MEM: MockMemory = {
        let mock_phys_mem = MockMemory::new(PMEM_SIZE);
        mock_phys_mem.mmap(
            phys_to_virt(0),
            PMEM_SIZE,
            0,
            MMUFlags::READ | MMUFlags::WRITE,
        );
        mock_phys_mem
    };
}

hal_fn_impl! {
    impl mod crate::hal_fn::mem {
        fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
            PMEM_BASE + paddr
        }

        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
            trace!("pmem read: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            let src = MOCK_PHYS_MEM.as_ptr(paddr);
            unsafe { buf.as_mut_ptr().copy_from_nonoverlapping(src, buf.len()) };
        }

        fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
            trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
            assert!(paddr + buf.len() <= PMEM_SIZE);
            let dst = MOCK_PHYS_MEM.as_mut_ptr::<u8>(paddr);
            unsafe { dst.copy_from_nonoverlapping(buf.as_ptr(), buf.len()) };
        }

        fn pmem_zero(paddr: PhysAddr, len: usize) {
            trace!("pmem_zero: addr={:#x}, len={:#x}", paddr, len);
            assert!(paddr + len <= PMEM_SIZE);
            unsafe { core::ptr::write_bytes(MOCK_PHYS_MEM.as_mut_ptr::<u8>(paddr), 0, len) };
        }

        fn pmem_copy(dst: PhysAddr, src: PhysAddr, len: usize) {
            trace!("pmem_copy: {:#x} <- {:#x}, len={:#x}", dst, src, len);
            assert!(src + len <= PMEM_SIZE && dst + len <= PMEM_SIZE);
            let dst = MOCK_PHYS_MEM.as_mut_ptr::<u8>(dst);
            let src = MOCK_PHYS_MEM.as_ptr::<u8>(src);
            unsafe { dst.copy_from_nonoverlapping(src, len) };
        }

        fn frame_flush(_target: PhysAddr) {
            // do nothing
        }
    }
}

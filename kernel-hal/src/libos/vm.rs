use super::mem::{MOCK_PHYS_MEM, PMEM_MAP_VADDR, PMEM_SIZE};
use crate::{addr::is_aligned, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
        fn current_vmtoken() -> PhysAddr {
            0
        }
    }
}

/// Page Table
pub struct PageTable;

impl PageTable {
    pub fn new() -> Self {
        Self
    }

    pub fn from_current() -> Self {
        Self
    }

    pub fn clone_kernel(&self) -> Self {
        Self::new()
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl GenericPageTable for PageTable {
    fn table_phys(&self) -> PhysAddr {
        0
    }

    fn map(&mut self, page: Page, paddr: PhysAddr, flags: MMUFlags) -> PagingResult {
        debug_assert!(page.size as usize == PAGE_SIZE);
        debug_assert!(is_aligned(paddr));
        if paddr < PMEM_SIZE {
            MOCK_PHYS_MEM.mmap(page.vaddr, PAGE_SIZE, paddr, flags);
            Ok(())
        } else {
            Err(PagingError::NoMemory)
        }
    }

    fn unmap(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)> {
        self.unmap_cont(vaddr, PAGE_SIZE)?;
        Ok((0, PageSize::Size4K))
    }

    fn update(
        &mut self,
        vaddr: VirtAddr,
        _paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize> {
        debug_assert!(is_aligned(vaddr));
        if let Some(flags) = flags {
            MOCK_PHYS_MEM.mprotect(vaddr as _, PAGE_SIZE, flags);
        }
        Ok(PageSize::Size4K)
    }

    fn query(&self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, MMUFlags, PageSize)> {
        debug_assert!(is_aligned(vaddr));
        if PMEM_MAP_VADDR <= vaddr && vaddr < PMEM_MAP_VADDR + PMEM_SIZE {
            Ok((
                vaddr - PMEM_MAP_VADDR,
                MMUFlags::READ | MMUFlags::WRITE,
                PageSize::Size4K,
            ))
        } else {
            Err(PagingError::NotMapped)
        }
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, size: usize) -> PagingResult {
        if size == 0 {
            return Ok(());
        }
        debug_assert!(is_aligned(vaddr));
        MOCK_PHYS_MEM.munmap(vaddr as _, size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid virtual address base to mmap.
    const VBASE: VirtAddr = 0x2_00000000;

    #[test]
    fn map_unmap() {
        let mut pt = PageTable::new();
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        // map 2 pages to 1 frame
        pt.map(Page::new_aligned(VBASE, PageSize::Size4K), 0x1000, flags)
            .unwrap();
        pt.map(
            Page::new_aligned(VBASE + 0x1000, PageSize::Size4K),
            0x1000,
            flags,
        )
        .unwrap();

        unsafe {
            const MAGIC: usize = 0xdead_beaf;
            (VBASE as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x1000) as *mut usize).read(), MAGIC);
        }

        pt.unmap(VBASE + 0x1000).unwrap();
    }
}

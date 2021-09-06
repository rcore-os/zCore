use crate::{HalResult, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

pub trait PageTableTrait: Sync + Send {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> HalResult {
        crate::vm::map_page(self.table_phys(), vaddr, paddr, flags)
    }

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, vaddr: VirtAddr) -> HalResult {
        crate::vm::unmap_page(self.table_phys(), vaddr)
    }

    /// Change the `flags` of the page of `vaddr`.
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> HalResult {
        crate::vm::update_page(self.table_phys(), vaddr, None, Some(flags))
    }

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&mut self, vaddr: VirtAddr) -> HalResult<PhysAddr> {
        crate::vm::query(self.table_phys(), vaddr).map(|(paddr, _)| paddr)
    }

    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr;

    /// Activate this page table.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it switches the page table.
    unsafe fn activate(&self) {
        crate::vm::activate_paging(self.table_phys());
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> HalResult<()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

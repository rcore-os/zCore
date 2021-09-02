use super::{HalResult, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

pub trait PageTableTrait: Sync + Send {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    fn map(&mut self, _vaddr: VirtAddr, _paddr: PhysAddr, _flags: MMUFlags) -> HalResult<()>;

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, _vaddr: VirtAddr) -> HalResult<()>;

    /// Change the `flags` of the page of `vaddr`.
    fn protect(&mut self, _vaddr: VirtAddr, _flags: MMUFlags) -> HalResult<()>;

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&mut self, _vaddr: VirtAddr) -> HalResult<PhysAddr>;

    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr;

    #[cfg(target_arch = "riscv64")]
    /// Activate this page table
    fn activate(&self);

    fn map_many(
        &mut self,
        mut vaddr: VirtAddr,
        paddrs: &[PhysAddr],
        flags: MMUFlags,
    ) -> HalResult<()> {
        for &paddr in paddrs {
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    fn map_cont(
        &mut self,
        mut vaddr: VirtAddr,
        paddr: PhysAddr,
        pages: usize,
        flags: MMUFlags,
    ) -> HalResult<()> {
        for i in 0..pages {
            let paddr = paddr + i * PAGE_SIZE;
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> HalResult<()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

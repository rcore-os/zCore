use crate::{HalResult, MMUFlags, PhysAddr, VirtAddr};

pub use crate::common::paging::*;

pub struct PageTable;

impl PageTable {
    /// Create a new `PageTable`.
    pub fn new() -> Self {
        PageTable
    }

    /// Get the current root page table physical address. (e.g. CR3, SATP, ...)
    pub fn current() -> Self {
        unimplemented!();
    }
}

impl PageTableTrait for PageTable {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    fn map(&mut self, _vaddr: VirtAddr, _paddr: PhysAddr, _flags: MMUFlags) -> HalResult<()> {
        unimplemented!();
    }

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, _vaddr: VirtAddr) -> HalResult<()> {
        unimplemented!();
    }

    /// Change the `flags` of the page of `vaddr`.
    fn protect(&mut self, _vaddr: VirtAddr, _flags: MMUFlags) -> HalResult<()> {
        unimplemented!();
    }

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&mut self, _vaddr: VirtAddr) -> HalResult<PhysAddr> {
        unimplemented!();
    }

    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr {
        unimplemented!();
    }

    #[cfg(target_arch = "riscv64")]
    /// Activate this page table
    fn activate(&self) {
        unimplemented!();
    }
}

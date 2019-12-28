//! Hardware Abstraction Layer

use super::*;

type ThreadId = usize;
type PhysAddr = usize;
type VirtAddr = usize;
type MMUFlags = usize;
type APIResult = usize;

#[repr(C)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    #[linkage = "weak"]
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(entry: usize, stack: usize, arg1: usize, arg2: usize) -> Self {
        #[cfg(test)]
        zircon_hal_unix::init();
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_thread_exit"]
    pub fn exit(&mut self) {
        unimplemented!()
    }
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    table_phys: PhysAddr,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        unimplemented!()
    }
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_map"]
    pub fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Unmap the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        unimplemented!()
    }
    /// Change the `flags` of the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Query the physical address which the page of `vaddr` maps to.
    #[linkage = "weak"]
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, vaddr: VirtAddr) -> Result<(PhysAddr, MMUFlags), ()> {
        unimplemented!()
    }
    /// Get the physical address of root page table.
    pub fn table_phys(&self) -> PhysAddr {
        self.table_phys
    }

    pub fn map_many(
        &mut self,
        mut vaddr: VirtAddr,
        paddrs: &[PhysAddr],
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for &paddr in paddrs {
            self.map(vaddr, paddr, flags)?;
            vaddr += 0x1000;
        }
        Ok(())
    }

    pub fn map_cont(
        &mut self,
        mut vaddr: VirtAddr,
        paddr: PhysAddr,
        pages: usize,
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for i in 0..pages {
            let paddr = paddr + i * 0x1000;
            self.map(vaddr, paddr, flags)?;
            vaddr += 0x1000;
        }
        Ok(())
    }
}

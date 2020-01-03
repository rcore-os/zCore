//! Hardware Abstraction Layer

use super::*;
use crate::vm::PAGE_SIZE;

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
    pub fn spawn(entry: usize, stack: usize, arg1: usize, arg2: usize, tls: usize) -> Self {
        #[cfg(test)]
        zircon_hal_unix::init();
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_thread_exit"]
    pub fn exit(&mut self) {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_thread_tls"]
    pub fn tls() -> usize {
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
            vaddr += PAGE_SIZE;
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
            let paddr = paddr + i * PAGE_SIZE;
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    pub fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), ()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        unimplemented!()
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }
}

impl Drop for PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        unimplemented!()
    }
}

/// Read physical memory from `paddr` to `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    unimplemented!()
}

/// Write physical memory to `paddr` from `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    unimplemented!()
}

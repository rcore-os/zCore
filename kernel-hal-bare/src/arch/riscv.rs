use super::*;
use kernel_hal::{PageTableTrait, PhysAddr, VirtAddr, Result};
use riscv::addr::Page;
use riscv::paging::{PageTableFlags as PTF, *};
use riscv::register::satp;

/// Page Table
#[repr(C)]
pub struct PageTableImpl {
    root_paddr: PhysAddr,
}

impl PageTableImpl {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        let root_frame = Frame::alloc().expect("failed to alloc frame");
        let root_vaddr = phys_to_virt(root_frame.paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        root.zero();
        let current =
            phys_to_virt(satp::read().frame().start_address().as_usize()) as *const PageTable;
        map_kernel(root_vaddr as _, current as _);
        trace!("create page table @ {:#x}", root_frame.paddr);
        PageTableImpl {
            root_paddr: root_frame.paddr,
        }
    }

    #[cfg(target_arch = "riscv32")]
    fn get(&mut self) -> Rv32PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        Rv32PageTable::new(root, phys_to_virt(0))
    }

    #[cfg(target_arch = "riscv64")]
    fn get(&mut self) -> Rv39PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        Rv39PageTable::new(root, phys_to_virt(0))
    }
}

impl PageTableTrait for PageTableImpl {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<()> {
        let mut pt = self.get();
        let page = Page::of_addr(vaddr);
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(paddr));
        pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
        trace!("map: {:x?} -> {:x?}, flags={:?}", vaddr, paddr, flags);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.unmap(page).unwrap().1.flush();
        trace!("unmap: {:x?}", vaddr);
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.update_flags(page, flags.to_ptf()).unwrap().flush();
        trace!("protect: {:x?}, flags={:?}", vaddr, flags);
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<riscv::addr::PhysAddr> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let res = pt.ref_entry(page);
        trace!("query: {:x?} => {:x?}", vaddr, res);
        match res {
            Ok(entry) => Ok(entry.addr()),
            Err(_) => Err(()),
        }
    }

    /// Get the physical address of root page table.
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
    }
}

pub unsafe fn set_page_table(vmtoken: usize) {
    #[cfg(target_arch = "riscv32")]
    let mode = satp::Mode::Sv32;
    #[cfg(target_arch = "riscv64")]
    let mode = satp::Mode::Sv39;
    satp::set(mode, 0, vmtoken >> 12);
}

trait FlagsExt {
    fn to_ptf(self) -> PTF;
}

impl FlagsExt for MMUFlags {
    fn to_ptf(self) -> PTF {
        let mut flags = PTF::VALID;
        if self.contains(MMUFlags::READ) {
            flags |= PTF::READABLE;
        }
        if self.contains(MMUFlags::WRITE) {
            flags |= PTF::WRITABLE;
        }
        if self.contains(MMUFlags::EXECUTE) {
            flags |= PTF::EXECUTABLE;
        }
        if self.contains(MMUFlags::USER) {
            flags |= PTF::USER;
        }
        flags
    }
}

struct FrameAllocatorImpl;

impl FrameAllocator for FrameAllocatorImpl {
    fn alloc(&mut self) -> Option<riscv::addr::Frame> {
        Frame::alloc().map(|f| {
            let paddr = riscv::addr::PhysAddr::new(f.paddr);
            riscv::addr::Frame::of_addr(paddr)
        })
    }
}

impl FrameDeallocator for FrameAllocatorImpl {
    fn dealloc(&mut self, frame: riscv::addr::Frame) {
        Frame {
            paddr: frame.start_address().as_usize(),
        }
        .dealloc()
    }
}

pub fn init() {}

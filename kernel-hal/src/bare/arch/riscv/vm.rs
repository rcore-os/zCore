use riscv::{
    addr::Page,
    asm::sfence_vma_all,
    paging::{
        FrameAllocator, FrameDeallocator, Mapper, PageTable as PT, PageTableFlags as PTF,
        Rv39PageTable,
    },
    register::satp,
};

use super::super::{ffi, mem::phys_to_virt};
use super::consts;
use crate::{HalError, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

pub use crate::common::vm::*;

// First core stores its SATP here.
static mut SATP: usize = 0;

pub(crate) unsafe fn set_page_table(vmtoken: usize) {
    #[cfg(target_arch = "riscv32")]
    let mode = satp::Mode::Sv32;
    #[cfg(target_arch = "riscv64")]
    let mode = satp::Mode::Sv39;
    debug!("set user table: {:#x?}", vmtoken);
    satp::set(mode, 0, vmtoken >> 12);
    //刷TLB好像很重要
    sfence_vma_all();
}

fn map_range(
    page_table: &mut Rv39PageTable,
    mut start_addr: VirtAddr,
    mut end_addr: VirtAddr,
    linear_offset: usize,
    flags: PTF,
) -> Result<(), ()> {
    trace!("Mapping range addr: {:#x} ~ {:#x}", start_addr, end_addr);

    start_addr = start_addr & !(PAGE_SIZE - 1);
    let mut start_page = start_addr / PAGE_SIZE;

    //end_addr = (end_addr + PAGE_SIZE - 1) & !(PAGE_SIZE -1);
    //let end_page = (end_addr - 1) / PAGE_SIZE;
    end_addr = end_addr & !(PAGE_SIZE - 1);
    let end_page = end_addr / PAGE_SIZE;

    while start_page <= end_page {
        let vaddr: VirtAddr = start_page * PAGE_SIZE;
        let page = riscv::addr::Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(vaddr - linear_offset));

        start_page += 1;

        trace!(
            "map_range: {:#x} -> {:#x}, flags={:?}",
            vaddr,
            vaddr - linear_offset,
            flags
        );
        page_table
            .map_to(page, frame, flags, &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
    }
    info!(
        "map range from {:#x} to {:#x}, flags: {:?}",
        start_addr,
        end_page * PAGE_SIZE,
        flags
    );

    Ok(())
}

/// remap kernel with 4K page
pub fn remap_the_kernel(dtb: usize) {
    extern "C" {
        fn start();

        fn stext();
        fn etext();
        fn srodata();
        fn erodata();
        fn sdata();
        fn edata();

        fn bootstack();
        fn bootstacktop();

        fn sbss();
        fn ebss();

        fn end();
    }

    let root_paddr = crate::mem::frame_alloc().expect("failed to alloc frame");
    let root_vaddr = phys_to_virt(root_paddr);
    let root = unsafe { &mut *(root_vaddr as *mut PT) };
    root.zero();
    let mut pt = Rv39PageTable::new(root, consts::PHYSICAL_MEMORY_OFFSET);

    let linear_offset = consts::PHYSICAL_MEMORY_OFFSET;
    //let mut flags = PTF::VALID | PTF::READABLE | PTF::WRITABLE | PTF::EXECUTABLE | PTF::USER;

    map_range(
        &mut pt,
        stext as usize,
        etext as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::EXECUTABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        srodata as usize,
        erodata as usize,
        linear_offset,
        PTF::VALID | PTF::READABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        sdata as usize,
        edata as usize,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // Stack
    map_range(
        &mut pt,
        bootstack as usize,
        bootstacktop as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    map_range(
        &mut pt,
        sbss as usize,
        ebss as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    info!("map Heap ...");
    // Heap
    map_range(
        &mut pt,
        end as usize,
        end as usize + PAGE_SIZE * 5120,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();
    info!("... Heap");

    // Device Tree
    #[cfg(feature = "board_qemu")]
    map_range(
        &mut pt,
        dtb,
        dtb + consts::MAX_DTB_SIZE,
        linear_offset,
        PTF::VALID | PTF::READABLE,
    )
    .unwrap();

    // PLIC
    map_range(
        &mut pt,
        phys_to_virt(consts::PLIC_PRIORITY),
        phys_to_virt(consts::PLIC_PRIORITY) + PAGE_SIZE * 0xf,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        phys_to_virt(consts::PLIC_THRESHOLD),
        phys_to_virt(consts::PLIC_THRESHOLD) + PAGE_SIZE * 0xf,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // UART0, VIRTIO
    map_range(
        &mut pt,
        phys_to_virt(consts::UART_BASE),
        phys_to_virt(consts::UART_BASE) + PAGE_SIZE * 0xf,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    //写satp
    let token = root_paddr;
    unsafe {
        set_page_table(token);
        SATP = token;
    }

    //use core::mem;
    //mem::forget(pt);

    info!("remap the kernel @ {:#x}", token);
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    pub(super) root_paddr: PhysAddr,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        let root_paddr = crate::mem::frame_alloc().expect("failed to alloc frame");
        let root_vaddr = phys_to_virt(root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PT) };
        root.zero();

        let current = phys_to_virt(satp::read().frame().start_address().as_usize()) as *const PT;
        unsafe { ffi::hal_pt_map_kernel(root_vaddr as _, current as _) };
        trace!("create page table @ {:#x}", root_paddr);
        PageTable { root_paddr }
    }

    pub fn current() -> Self {
        #[cfg(target_arch = "riscv32")]
        let _mode = satp::Mode::Sv32;
        #[cfg(target_arch = "riscv64")]
        let _mode = satp::Mode::Sv39;
        let root_paddr = satp::read().ppn() << 12;
        PageTable { root_paddr }
    }

    #[cfg(target_arch = "riscv32")]
    pub(super) fn get(&mut self) -> Rv32PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PT) };
        Rv32PageTable::new(root, phys_to_virt(0))
    }

    #[cfg(target_arch = "riscv64")]
    pub(super) fn get(&mut self) -> Rv39PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PT) };
        Rv39PageTable::new(root, phys_to_virt(0))
    }
}

impl PageTableTrait for PageTable {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(paddr));
        pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap()
            .flush();

        trace!(
            "PageTable: {:#X}, map: {:x?} -> {:x?}, flags={:?}",
            self.table_phys() as usize,
            vaddr,
            paddr,
            flags
        );
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.unmap(page).unwrap().1.flush();
        trace!(
            "PageTable: {:#X}, unmap: {:x?}",
            self.table_phys() as usize,
            vaddr
        );
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.update_flags(page, flags.to_ptf()).unwrap().flush();

        trace!(
            "PageTable: {:#X}, protect: {:x?}, flags={:?}",
            self.table_phys() as usize,
            vaddr,
            flags
        );
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let res = pt.ref_entry(page);
        trace!("query: {:x?} => {:#x?}", vaddr, res);
        match res {
            Ok(entry) => Ok(entry.addr().as_usize()),
            Err(_) => Err(HalError),
        }
    }

    /// Get the physical address of root page table.
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
    }

    /// Activate this page table
    #[export_name = "hal_pt_activate"]
    fn activate(&self) {
        let now_token = satp::read().bits();
        let new_token = self.table_phys();
        if now_token != new_token {
            debug!("switch table {:x?} -> {:x?}", now_token, new_token);
            unsafe {
                set_page_table(new_token);
            }
        }
    }
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
        crate::mem::frame_alloc().map(|f| {
            let paddr = riscv::addr::PhysAddr::new(f);
            riscv::addr::Frame::of_addr(paddr)
        })
    }
}

impl FrameDeallocator for FrameAllocatorImpl {
    fn dealloc(&mut self, frame: riscv::addr::Frame) {
        crate::mem::frame_dealloc(frame.start_address().as_usize());
    }
}

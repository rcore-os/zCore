use riscv::{
    addr::Page,
    asm,
    paging::{FrameAllocator, FrameDeallocator, Mapper, PageTable as PT, PageTableFlags as PTF},
    register::satp,
};

use super::consts;
use crate::vm::{PageTable, PageTableTrait};
use crate::{mem::phys_to_virt, HalError, HalResult, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

#[cfg(target_arch = "riscv32")]
type RvPageTable<'a> = riscv::paging::Rv32PageTable<'a>;
#[cfg(target_arch = "riscv64")]
type RvPageTable<'a> = riscv::paging::Rv39PageTable<'a>;

fn map_range(
    vmtoken: PhysAddr,
    start_addr: VirtAddr,
    end_addr: VirtAddr,
    linear_offset: usize,
    flags: MMUFlags,
) -> HalResult {
    trace!("Mapping range addr: {:#x} ~ {:#x}", start_addr, end_addr);

    let start_addr = start_addr & !(PAGE_SIZE - 1);
    let mut start_page = start_addr / PAGE_SIZE;

    //end_addr = (end_addr + PAGE_SIZE - 1) & !(PAGE_SIZE -1);
    //let end_page = (end_addr - 1) / PAGE_SIZE;
    let end_addr = end_addr & !(PAGE_SIZE - 1);
    let end_page = end_addr / PAGE_SIZE;

    while start_page <= end_page {
        let vaddr: VirtAddr = start_page * PAGE_SIZE;
        map_page(vmtoken, vaddr, vaddr - linear_offset, flags)?;
        start_page += 1;
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
pub fn remap_the_kernel(dtb: usize) -> HalResult {
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

    let pt = PageTable::new();
    let root_paddr = pt.table_phys();
    let linear_offset = consts::PHYSICAL_MEMORY_OFFSET;

    map_range(
        root_paddr,
        stext as usize,
        etext as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::EXECUTE,
    )?;
    map_range(
        root_paddr,
        srodata as usize,
        erodata as usize,
        linear_offset,
        MMUFlags::READ,
    )?;
    map_range(
        root_paddr,
        sdata as usize,
        edata as usize,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    // Stack
    map_range(
        root_paddr,
        bootstack as usize,
        bootstacktop as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    map_range(
        root_paddr,
        sbss as usize,
        ebss as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    info!("map Heap ...");
    // Heap
    map_range(
        root_paddr,
        end as usize,
        end as usize + PAGE_SIZE * 5120,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    info!("... Heap");

    // Device Tree
    #[cfg(feature = "board_qemu")]
    map_range(
        root_paddr,
        dtb,
        dtb + consts::MAX_DTB_SIZE,
        linear_offset,
        MMUFlags::READ,
    )?;

    // PLIC
    map_range(
        root_paddr,
        phys_to_virt(consts::PLIC_PRIORITY),
        phys_to_virt(consts::PLIC_PRIORITY) + PAGE_SIZE * 0xf,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    map_range(
        root_paddr,
        phys_to_virt(consts::PLIC_THRESHOLD),
        phys_to_virt(consts::PLIC_THRESHOLD) + PAGE_SIZE * 0xf,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    // UART0, VIRTIO
    map_range(
        root_paddr,
        phys_to_virt(consts::UART_BASE),
        phys_to_virt(consts::UART_BASE) + PAGE_SIZE * 0xf,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    unsafe {
        pt.activate();
        core::mem::forget(pt);
    }

    info!("remap the kernel @ {:#x}", root_paddr);
    Ok(())
}

fn page_table_of<'a>(root_paddr: PhysAddr) -> RvPageTable<'a> {
    let root_vaddr = phys_to_virt(root_paddr);
    let root = unsafe { &mut *(root_vaddr as *mut PT) };
    RvPageTable::new(root, phys_to_virt(0))
}

hal_fn_impl! {
    impl mod crate::defs::vm {
        fn map_page(vmtoken: PhysAddr, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> HalResult {
            let mut pt = page_table_of(vmtoken);
            let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
            let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(paddr));
            pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
                .unwrap()
                .flush();

            trace!(
                "PageTable: {:#X}, map: {:x?} -> {:x?}, flags={:?}",
                vmtoken,
                vaddr,
                paddr,
                flags
            );
            Ok(())
        }

        fn unmap_page(vmtoken: PhysAddr, vaddr: VirtAddr) -> HalResult {
            let mut pt = page_table_of(vmtoken);
            let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
            pt.unmap(page).unwrap().1.flush();
            trace!("PageTable: {:#X}, unmap: {:x?}", vmtoken, vaddr);
            Ok(())
        }

        fn update_page(
            vmtoken: PhysAddr,
            vaddr: VirtAddr,
            paddr: Option<PhysAddr>,
            flags: Option<MMUFlags>,
        ) -> HalResult {
            debug_assert!(paddr.is_none());
            let mut pt = page_table_of(vmtoken);
            if let Some(flags) = flags {
                let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
                pt.update_flags(page, flags.to_ptf()).unwrap().flush();
                trace!(
                    "PageTable: {:#X}, protect: {:x?}, flags={:?}",
                    vmtoken,
                    vaddr,
                    flags
                );
            }
            Ok(())
        }

        fn query(vmtoken: PhysAddr, vaddr: VirtAddr) -> HalResult<(PhysAddr, MMUFlags)> {
            let mut pt = page_table_of(vmtoken);
            let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
            let res = pt.ref_entry(page);
            trace!("query: {:x?} => {:#x?}", vaddr, res);
            match res {
                Ok(entry) => Ok((entry.addr().as_usize(), MMUFlags::from_ptf(entry.flags()))),
                Err(_) => Err(HalError),
            }
        }

        fn activate_paging(vmtoken: PhysAddr) {
            let old_token = current_vmtoken();
            if old_token != vmtoken {
                #[cfg(target_arch = "riscv32")]
                let mode = satp::Mode::Sv32;
                #[cfg(target_arch = "riscv64")]
                let mode = satp::Mode::Sv39;
                debug!("switch table {:x?} -> {:x?}", old_token, vmtoken);
                unsafe {
                    satp::set(mode, 0, vmtoken >> 12);
                    //刷TLB好像很重要
                    asm::sfence_vma_all();
                }
            }
        }

        fn current_vmtoken() -> PhysAddr {
            satp::read().ppn() << 12
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            unsafe {
                if let Some(vaddr) = vaddr {
                    asm::sfence_vma(0, vaddr)
                } else {
                    asm::sfence_vma_all();
                }
            }
        }
    }
}

trait FlagsExt {
    fn to_ptf(self) -> PTF;
    fn from_ptf(f: PTF) -> Self;
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

    fn from_ptf(f: PTF) -> Self {
        let mut ret = Self::empty();
        if f.contains(PTF::READABLE) {
            ret |= Self::READ;
        }
        if f.contains(PTF::WRITABLE) {
            ret |= Self::WRITE;
        }
        if f.contains(PTF::EXECUTABLE) {
            ret |= Self::EXECUTE;
        }
        if f.contains(PTF::USER) {
            ret |= Self::USER;
        }
        ret
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

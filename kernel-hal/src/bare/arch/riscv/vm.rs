use core::fmt::{Debug, Formatter, Result};

use riscv::{asm, paging::PageTableFlags as PTF, register::satp};

use super::consts;
use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel3};
use crate::{mem::phys_to_virt, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

#[cfg(target_arch = "riscv32")]
type RvPageTable<'a> = riscv::paging::Rv32PageTable<'a>;
#[cfg(target_arch = "riscv64")]
type RvPageTable<'a> = riscv::paging::Rv39PageTable<'a>;

fn map_range(
    pt: &mut PageTable,
    start_addr: VirtAddr,
    end_addr: VirtAddr,
    linear_offset: usize,
    flags: MMUFlags,
) -> PagingResult {
    info!(
        "Mapping range addr: {:#x} ~ {:#x} {:?}",
        start_addr, end_addr, flags
    );

    let start_addr = start_addr & !(PAGE_SIZE - 1);
    let mut start_page = start_addr / PAGE_SIZE;

    //end_addr = (end_addr + PAGE_SIZE - 1) & !(PAGE_SIZE -1);
    //let end_page = (end_addr - 1) / PAGE_SIZE;
    let end_addr = end_addr & !(PAGE_SIZE - 1);
    let end_page = end_addr / PAGE_SIZE;

    while start_page <= end_page {
        let vaddr: VirtAddr = start_page * PAGE_SIZE;
        let page = Page::new_aligned(vaddr, PageSize::Size4K);
        pt.map(page, vaddr - linear_offset, flags)?;
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
pub fn remap_the_kernel(dtb: usize) -> PagingResult {
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

    let mut pt = PageTable::new();
    let root_paddr = pt.table_phys();
    let linear_offset = consts::PHYSICAL_MEMORY_OFFSET;

    map_range(
        &mut pt,
        stext as usize,
        etext as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::EXECUTE,
    )?;
    map_range(
        &mut pt,
        srodata as usize,
        erodata as usize,
        linear_offset,
        MMUFlags::READ,
    )?;
    map_range(
        &mut pt,
        sdata as usize,
        edata as usize,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    // Stack
    map_range(
        &mut pt,
        bootstack as usize,
        bootstacktop as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    map_range(
        &mut pt,
        sbss as usize,
        ebss as usize - 1,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    info!("map Heap ...");
    // Heap
    map_range(
        &mut pt,
        end as usize,
        end as usize + PAGE_SIZE * 5120,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    info!("... Heap");

    // Device Tree
    #[cfg(feature = "board_qemu")]
    map_range(
        &mut pt,
        dtb,
        dtb + consts::MAX_DTB_SIZE,
        linear_offset,
        MMUFlags::READ,
    )?;

    // PLIC
    map_range(
        &mut pt,
        phys_to_virt(consts::PLIC_PRIORITY),
        phys_to_virt(consts::PLIC_PRIORITY) + PAGE_SIZE * 0xf,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    map_range(
        &mut pt,
        phys_to_virt(consts::PLIC_THRESHOLD),
        phys_to_virt(consts::PLIC_THRESHOLD) + PAGE_SIZE * 0xf,
        linear_offset,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    // UART0, VIRTIO
    map_range(
        &mut pt,
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

hal_fn_impl! {
    impl mod crate::defs::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            let old_token = current_vmtoken();
            if old_token != vmtoken {
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

impl From<MMUFlags> for PTF {
    fn from(f: MMUFlags) -> Self {
        let mut flags = PTF::VALID;
        if f.contains(MMUFlags::READ) {
            flags |= PTF::READABLE;
        }
        if f.contains(MMUFlags::WRITE) {
            flags |= PTF::WRITABLE;
        }
        if f.contains(MMUFlags::EXECUTE) {
            flags |= PTF::EXECUTABLE;
        }
        if f.contains(MMUFlags::USER) {
            flags |= PTF::USER;
        }
        flags
    }
}

impl From<PTF> for MMUFlags {
    fn from(f: PTF) -> Self {
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

const PHYS_ADDR_MASK: u64 = 0x003f_ffff_ffff_fc00; // 10..54

#[derive(Clone)]
#[repr(transparent)]
pub struct Rv64PTE(u64);

impl GenericPTE for Rv64PTE {
    fn addr(&self) -> PhysAddr {
        ((self.0 & PHYS_ADDR_MASK) << 2) as _
    }
    fn flags(&self) -> MMUFlags {
        PTF::from_bits_truncate(self.0 as usize).into()
    }
    fn is_unused(&self) -> bool {
        self.0 == 0
    }
    fn is_present(&self) -> bool {
        PTF::from_bits_truncate(self.0 as usize).contains(PTF::VALID)
    }
    fn is_leaf(&self) -> bool {
        PTF::from_bits_truncate(self.0 as usize).contains(PTF::READABLE | PTF::EXECUTABLE)
    }

    fn set_leaf(&mut self, paddr: Option<PhysAddr>, flags: Option<MMUFlags>, _is_huge: bool) {
        let paddr_bits = if let Some(paddr) = paddr {
            (paddr as u64 >> 2) & PHYS_ADDR_MASK
        } else {
            self.0 & PHYS_ADDR_MASK
        };
        let flags_bits = if let Some(flags) = flags {
            let flags = PTF::from(flags) | PTF::ACCESSED | PTF::DIRTY;
            debug_assert!(flags.contains(PTF::READABLE | PTF::EXECUTABLE));
            flags.bits() as u64
        } else {
            self.0 & !PHYS_ADDR_MASK
        };
        self.0 = paddr_bits | flags_bits;
    }
    fn set_table(&mut self, paddr: PhysAddr) {
        self.0 = ((paddr as u64 >> 2) & PHYS_ADDR_MASK) | PTF::VALID.bits() as u64;
    }
    fn clear(&mut self) {
        self.0 = 0
    }
}

impl Debug for Rv64PTE {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let mut f = f.debug_struct("Rv64PTE");
        f.field("raw", &self.0);
        f.field("addr", &self.addr());
        f.field("flags", &self.flags());
        f.finish()
    }
}

pub type PageTable = PageTableImpl<PageTableLevel3, Rv64PTE>;

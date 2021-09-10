use core::fmt::{Debug, Formatter, Result};

use riscv::{asm, paging::PageTableFlags as PTF, register::satp};

use super::consts;
use crate::addr::{align_down, align_up};
use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel3};
use crate::{mem::phys_to_virt, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

#[cfg(target_arch = "riscv32")]
type RvPageTable<'a> = riscv::paging::Rv32PageTable<'a>;
#[cfg(target_arch = "riscv64")]
type RvPageTable<'a> = riscv::paging::Rv39PageTable<'a>;

/// remap kernel with 4K page
pub(super) fn remap_the_kernel() -> PagingResult {
    extern "C" {
        fn stext();
        fn etext();
        fn srodata();
        fn erodata();
        fn sdata();
        fn edata();
        fn sbss();
        fn ebss();

        fn bootstack();
        fn bootstacktop();

        fn end();
    }

    let mut pt = PageTable::new();
    let root_paddr = pt.table_phys();
    let mut map_range = |start: VirtAddr, end: VirtAddr, flags: MMUFlags| -> PagingResult {
        pt.map_cont(
            start,
            crate::addr::align_up(end - start),
            start - unsafe { super::super::ffi::PHYS_TO_VIRT_OFFSET },
            flags | MMUFlags::HUGE_PAGE,
        )
    };

    map_range(
        stext as usize,
        etext as usize,
        MMUFlags::READ | MMUFlags::EXECUTE,
    )?;
    map_range(srodata as usize, erodata as usize, MMUFlags::READ)?;
    map_range(
        sdata as usize,
        edata as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    map_range(
        sbss as usize,
        ebss as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // stack
    map_range(
        bootstack as usize,
        bootstacktop as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // physical memory
    map_range(
        align_up(end as usize + PAGE_SIZE),
        phys_to_virt(align_down(consts::MEMORY_END)),
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // PLIC
    map_range(
        phys_to_virt(consts::PLIC_BASE),
        phys_to_virt(consts::PLIC_BASE + 0x40_0000), // 4M
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // UART0, VIRTIO
    map_range(
        phys_to_virt(consts::UART_BASE),
        phys_to_virt(consts::UART_BASE + 0x1000),
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

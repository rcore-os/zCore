use core::fmt::{Debug, Formatter, Result};
use core::slice;

use riscv::{asm, register::satp};
use spin::Mutex;

use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel3};
use crate::{mem::phys_to_virt, MMUFlags, PhysAddr, VirtAddr, KCONFIG};

lazy_static! {
    static ref KERNEL_PT: Mutex<PageTable> = Mutex::new(init_kernel_page_table().unwrap());
}

/// remap kernel ELF segments with 4K page
fn init_kernel_page_table() -> PagingResult<PageTable> {
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
    }

    let mut pt = PageTable::new();
    let mut map_range = |start: VirtAddr, end: VirtAddr, flags: MMUFlags| -> PagingResult {
        pt.map_cont(
            start,
            crate::addr::align_up(end - start),
            start - KCONFIG.phys_to_virt_offset,
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
    // physical frames
    let region = &crate::mem::free_pmem_regions()[0];
    map_range(
        phys_to_virt(region.start),
        phys_to_virt(region.end),
        MMUFlags::READ | MMUFlags::WRITE,
    )?;

    info!("initialized kernel page table @ {:#x}", pt.table_phys());
    Ok(pt)
}

pub(super) fn kernel_page_table() -> &'static Mutex<PageTable> {
    &KERNEL_PT
}

pub(super) fn init() {
    unsafe { KERNEL_PT.lock().activate() };
}

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
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

        fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr) {
            let entry_range = 0x100..0x200; // 0xFFFF_FFC0_0000_0000 .. 0xFFFF_FFFF_FFFF_FFFF
            let dst_table = unsafe { slice::from_raw_parts_mut(phys_to_virt(dst_pt_root) as *mut Rv64PTE, 512) };
            let src_table = unsafe { slice::from_raw_parts(phys_to_virt(src_pt_root) as *const Rv64PTE, 512) };
            for i in entry_range {
                dst_table[i] = src_table[i];
                if !dst_table[i].is_unused() {
                    dst_table[i].0 |= PTF::GLOBAL.bits() as u64;
                }
            }
        }
    }
}

bitflags::bitflags! {
    /// Possible flags for a page table entry.
    pub struct PTF: usize {
        const VALID =       1 << 0;
        const READABLE =    1 << 1;
        const WRITABLE =    1 << 2;
        const EXECUTABLE =  1 << 3;
        const USER =        1 << 4;
        const GLOBAL =      1 << 5;
        const ACCESSED =    1 << 6;
        const DIRTY =       1 << 7;
        const RESERVED1 =   1 << 8;
        const RESERVED2 =   1 << 9;
    }
}

impl From<MMUFlags> for PTF {
    fn from(f: MMUFlags) -> Self {
        if f.is_empty() {
            return PTF::empty();
        }
        let mut flags = PTF::VALID;
        if f.contains(MMUFlags::READ) {
            flags |= PTF::READABLE;
        }
        if f.contains(MMUFlags::WRITE) {
            flags |= PTF::READABLE | PTF::WRITABLE;
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

#[derive(Clone, Copy)]
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
        PTF::from_bits_truncate(self.0 as usize).intersects(PTF::READABLE | PTF::EXECUTABLE)
    }

    fn set_addr(&mut self, paddr: PhysAddr) {
        self.0 = (self.0 & !PHYS_ADDR_MASK) | ((paddr as u64 >> 2) & PHYS_ADDR_MASK);
    }
    fn set_flags(&mut self, flags: MMUFlags, _is_huge: bool) {
        let flags = PTF::from(flags) | PTF::ACCESSED | PTF::DIRTY;
        debug_assert!(flags.contains(PTF::READABLE | PTF::EXECUTABLE));
        self.0 = (self.0 & PHYS_ADDR_MASK) | flags.bits() as u64;
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

/// Sv39: Page-Based 39-bit Virtual-Memory System
pub type PageTable = PageTableImpl<PageTableLevel3, Rv64PTE>;

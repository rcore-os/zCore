use crate::{PhysAddr, VirtAddr, KCONFIG};
use cortex_a::registers::*;
use tock_registers::interfaces::{Writeable, Readable};
use crate::hal_fn::mem::phys_to_virt;
use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel4};
use crate::MMUFlags;
use core::fmt::{Debug, Formatter, Result};
use crate::imp::config::*;
use spin::Mutex;

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
        fn boot_stack();
        fn boot_stack_top();
    }

    let mut pt = PageTable::new();
    let mut map_range = |start: VirtAddr, end: VirtAddr, flags: MMUFlags| -> PagingResult {
        pt.map_cont(
            crate::addr::align_down(start),
            crate::addr::align_up(end - start),
            start - KCONFIG.phys_to_virt_offset,
            flags,
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
        boot_stack as usize,
        boot_stack_top as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // uart
    map_range(
        0xffff_0000_0900_0000,
        0xffff_0000_0900_0000 + 0x1000,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // gic
    map_range(
        0xffff_0000_0800_0000,
        0xffff_0000_0800_0000 + 0x20000,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // physical frames
    for r in crate::mem::free_pmem_regions() {
        map_range(
            phys_to_virt(r.start),
            phys_to_virt(r.end),
            MMUFlags::READ | MMUFlags::WRITE,
        )?;
    }

    Ok(pt)
}

pub fn init() {
    let mut pt = KERNEL_PT.lock();
    info!("initialized kernel page table @ {:#x}", pt.table_phys());
    unsafe {
        pt.activate();
        TTBR0_EL1.set(0);
        flush_tlb_all();
    }
}

pub fn flush_tlb_all() {
    unsafe { core::arch::asm!("tlbi vmalle1; dsb sy; isb") };
}

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            info!("set page_table @ {:#x}", vmtoken);
            TTBR1_EL1.set(vmtoken as _);
        }

        fn current_vmtoken() -> PhysAddr {
            TTBR1_EL1.get() as _
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            // Translations used at EL1 for the specified address, for all ASID values,
            // in the Inner Shareable shareability domain.
            unsafe {
                core::arch::asm!(
                    "dsb ishst
                    tlbi vaae1is, {0}
                    dsb ish
                    isb",
                    in(reg) vaddr.unwrap() >> 12
                );
            }
        }
    }
}

bitflags::bitflags! {
    /// Possible flags for a page table entry.
    struct PTF: usize {
        // Attribute fields in stage 1 VMSAv8-64 Block and Page descriptors:
        /// Whether the descriptor is valid.
        const VALID =       1 << 0;
        /// The descriptor gives the address of the next level of translation table or 4KB page.
        /// (not a 2M, 1G block)
        const NON_BLOCK =   1 << 1;
        /// Memory attributes index field.
        const ATTR_INDX =   0b111 << 2;
        /// Non-secure bit. For memory accesses from Secure state, specifies whether the output
        /// address is in Secure or Non-secure memory.
        const NS =          1 << 5;
        /// Access permission: accessable at EL0.
        const AP_EL0 =      1 << 6;
        /// Access permission: read-only.
        const AP_RO =       1 << 7;
        /// Shareability: Inner Shareable (otherwise Outer Shareable).
        const INNER =       1 << 8;
        /// Shareability: Inner or Outer Shareable (otherwise Non-shareable).
        const SHAREABLE =   1 << 9;
        /// The Access flag.
        const AF =          1 << 10;
        /// The not global bit.
        const NG =          1 << 11;
        /// Indicates that 16 adjacent translation table entries point to contiguous memory regions.
        const CONTIGUOUS =  1 <<  52;
        /// The Privileged execute-never field.
        const PXN =         1 <<  53;
        /// The Execute-never or Unprivileged execute-never field.
        const UXN =         1 <<  54;

        // Next-level attributes in stage 1 VMSAv8-64 Table descriptors:

        /// PXN limit for subsequent levels of lookup.
        const PXN_TABLE =           1 << 59;
        /// XN limit for subsequent levels of lookup.
        const XN_TABLE =            1 << 60;
        /// Access permissions limit for subsequent levels of lookup: access at EL0 not permitted.
        const AP_NO_EL0_TABLE =     1 << 61;
        /// Access permissions limit for subsequent levels of lookup: write access not permitted.
        const AP_NO_WRITE_TABLE =   1 << 62;
        /// For memory accesses from Secure state, specifies the Security state for subsequent
        /// levels of lookup.
        const NS_TABLE =            1 << 63;
    }
}

#[repr(u64)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum MemType {
    #[allow(dead_code)]
    Device = 0,
    Normal = 1,
}

impl PTF {
    const ATTR_INDEX_MASK: u64 = 0b111_00;

    const fn from_mem_type(mem_type: MemType) -> Self {
        let mut bits = (mem_type as u64) << 2;
        if matches!(mem_type, MemType::Normal) {
            bits |= (Self::INNER.bits() | Self::SHAREABLE.bits()) as u64;
        }
        Self::from_bits_truncate(bits as usize)
    }

    #[allow(dead_code)]
    fn mem_type(&self) -> MemType {
        let idx = (self.bits() as u64 & Self::ATTR_INDEX_MASK) >> 2;
        match idx {
            0 => MemType::Device,
            1 => MemType::Normal,
            _ => panic!("Invalid memory attribute index"),
        }
    }
}

impl From<MMUFlags> for PTF {
    fn from(f: MMUFlags) -> Self {
        let mut flags = Self::from_mem_type(MemType::Normal);
        if f.is_empty() {
            return flags;
        }
        if f.contains(MMUFlags::READ) {
            flags |= PTF::VALID;
        }
        if !f.contains(MMUFlags::WRITE) {
            flags |= PTF::AP_RO;
        }
        if f.contains(MMUFlags::USER) {
            flags |= PTF::AP_EL0 | PTF::PXN;
            if !f.contains(MMUFlags::EXECUTE) {
                flags |= PTF::UXN;
            }
        } else {
            flags |= PTF::UXN;
            if !f.contains(MMUFlags::EXECUTE) {
                flags |= PTF::PXN;
            }
        }
        flags
    }
}

impl From<PTF> for MMUFlags {
    fn from(f: PTF) -> Self {
        let mut ret = Self::empty();
        if f.contains(PTF::VALID) {
            ret |= Self::READ;
        }
        if !f.contains(PTF::AP_RO) {
            ret |= Self::WRITE;
        }
        if f.contains(PTF::AP_EL0) {
            ret |= Self::USER;
            if !f.contains(PTF::UXN) {
                ret |= Self::EXECUTE;
            }
        } else if f.intersects(PTF::PXN) {
            ret |= Self::EXECUTE;
        }
        ret
    }
}

/// Page table entry.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct AARCH64PTE(u64);
impl GenericPTE for AARCH64PTE {
    fn addr(&self) -> PhysAddr {
        (self.0 as usize & PHYS_ADDR_MASK) as _
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
        !PTF::from_bits_truncate(self.0 as usize).intersects(PTF::NON_BLOCK)
    }
    fn set_addr(&mut self, paddr: PhysAddr) {
        self.0 = (paddr & PHYS_ADDR_MASK) as u64;
    }
    fn set_flags(&mut self, flags: MMUFlags, is_huge: bool) {
        let mut flags = PTF::from(flags) | PTF::AF;
        if !is_huge { flags |= PTF::NON_BLOCK }
        self.0 = (self.0 & PHYS_ADDR_MASK as u64) | flags.bits() as u64;
    }
    fn set_table(&mut self, paddr: PhysAddr) {
        self.0 = (((paddr as usize) & PHYS_ADDR_MASK) | PTF::VALID.bits() | PTF::NON_BLOCK.bits()) as u64;
    }
    fn clear(&mut self) {
        self.0 = 0
    }
}

impl Debug for AARCH64PTE {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let mut f = f.debug_struct("AARCH64PTE");
        f.field("raw", &self.0);
        f.field("addr", &self.addr());
        f.field("flags", &self.flags());
        f.finish()
    }
}

/// Sv48: Page-Based 48-bit Virtual-Memory System.
pub type PageTable = PageTableImpl<PageTableLevel4, AARCH64PTE>;
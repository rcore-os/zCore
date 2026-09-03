use crate::hal_fn::mem::phys_to_virt;
use crate::imp::config::*;
use crate::sync::Mutex;
#[cfg(feature = "page-64k")]
use crate::utils::page_table::PageTableLevel3;
#[cfg(not(feature = "page-64k"))]
use crate::utils::page_table::PageTableLevel4;
use crate::utils::page_table::{GenericPTE, PageTableImpl};
use crate::MMUFlags;
use crate::{PhysAddr, VirtAddr, KCONFIG};
use core::fmt::{Debug, Formatter, Result};
use cortex_a::registers::*;
use tock_registers::interfaces::{Readable, Writeable};
use zcore_drivers::irq::gic_400::{GICC_SIZE, GICD_SIZE};

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
        let aligned_start = crate::addr::align_down(start);
        let aligned_end = crate::addr::align_up(end);
        let size = aligned_end - aligned_start;
        if size == 0 {
            return Ok(());
        }
        pt.map_cont(
            aligned_start,
            size,
            aligned_start - KCONFIG.phys_to_virt_offset,
            flags,
        )
    };

    map_range(
        stext as *const () as usize,
        etext as *const () as usize,
        MMUFlags::READ | MMUFlags::EXECUTE,
    )?;
    map_range(
        srodata as *const () as usize,
        erodata as *const () as usize,
        MMUFlags::READ,
    )?;
    map_range(
        sdata as *const () as usize,
        edata as *const () as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    map_range(
        sbss as *const () as usize,
        ebss as *const () as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // stack
    map_range(
        boot_stack as *const () as usize,
        boot_stack_top as *const () as usize,
        MMUFlags::READ | MMUFlags::WRITE,
    )?;
    // uart
    map_range(
        phys_to_virt(KCONFIG.uart_base),
        phys_to_virt(KCONFIG.uart_base) + UART_SIZE,
        MMUFlags::READ | MMUFlags::WRITE | MMUFlags::DEVICE,
    )?;
    // gic
    map_range(
        phys_to_virt(KCONFIG.gic_base + 0x1_0000),
        phys_to_virt(KCONFIG.gic_base + 0x1_0000) + GICC_SIZE,
        MMUFlags::READ | MMUFlags::WRITE | MMUFlags::DEVICE,
    )?;
    map_range(
        phys_to_virt(KCONFIG.gic_base),
        phys_to_virt(KCONFIG.gic_base) + GICD_SIZE,
        MMUFlags::READ | MMUFlags::WRITE | MMUFlags::DEVICE,
    )?;
    // virtio_drivers
    map_range(
        phys_to_virt(VIRTIO_BASE),
        phys_to_virt(VIRTIO_BASE) + VIRTIO_SIZE,
        MMUFlags::READ | MMUFlags::WRITE | MMUFlags::DEVICE,
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
    unsafe {
        pt.activate();
        flush_tlb_all();
    }
}

pub fn flush_tlb_all() {
    unsafe {
        core::arch::asm!(
            "dsb ishst
             tlbi vmalle1is
             dsb ish
             isb"
        );
    }
}

#[unsafe(naked)]
unsafe extern "C" fn switch_kernel_table_trampoline(
    _new_ttbr1: u64,
    _tcr_transition: u64,
    _final_tcr: u64,
    _target_virt_pc: usize,
) {
    core::arch::naked_asm!(
        "msr ttbr1_el1, x0",
        "msr tcr_el1, x1",
        "isb",
        "tlbi vmalle1",
        "dsb sy",
        "isb",
        "mov x0, x2",
        "br x3",
    );
}

#[unsafe(naked)]
unsafe extern "C" fn after_switch_trampoline(_final_tcr: u64) {
    core::arch::naked_asm!(
        "msr tcr_el1, x0",
        "msr ttbr0_el1, xzr",
        "isb",
        "tlbi vmalle1",
        "dsb sy",
        "isb",
        "ret",
    );
}

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            let check_if_user = (vmtoken & USER_TABLE_FLAG) != 0;
            let vmtoken = vmtoken & PHYS_ADDR_MASK;
            info!("set {} page_table @ {:#x}", if check_if_user { "user" } else { "kernel" }, vmtoken);

            // TCR_EL1 base for the configured granule, *without* IPS.
            #[cfg(feature = "page-16k")]
            const TCR_BASE: u64 = 0x7510_b510;
            #[cfg(feature = "page-64k")]
            const TCR_BASE: u64 = 0xf510_7510;
            #[cfg(not(any(feature = "page-16k", feature = "page-64k")))]
            const TCR_BASE: u64 = 0xb510_3510;

            let ips = ID_AA64MMFR0_EL1.read(ID_AA64MMFR0_EL1::PARange);
            let tcr = TCR_BASE | (ips << 32);

            if check_if_user {
                TTBR0_EL1.set(vmtoken as _);
                unsafe {
                    core::arch::asm!("msr tcr_el1, {0}", in(reg) tcr);
                    core::arch::asm!("isb");
                }
            } else {
                let ttbr0 = TTBR0_EL1.get();
                if ttbr0 != 0 {
                    let current_tcr: u64;
                    unsafe {
                        core::arch::asm!("mrs {0}, tcr_el1", out(reg) current_tcr);
                    }
                    let tcr_transition = (tcr & 0xffff_ffff_ffff_0000) | (current_tcr & 0x0000_ffff);
                    let fn_phys = (switch_kernel_table_trampoline as *const () as usize) - KCONFIG.phys_to_virt_offset;
                    let trampoline: unsafe extern "C" fn(u64, u64, u64, usize) = unsafe { core::mem::transmute(fn_phys) };
                    unsafe {
                        trampoline(vmtoken as u64, tcr_transition, tcr, after_switch_trampoline as *const () as usize);
                    }
                } else {
                    TTBR1_EL1.set(vmtoken as _);
                    unsafe {
                        core::arch::asm!("msr tcr_el1, {0}", in(reg) tcr);
                        core::arch::asm!("isb");
                    }
                }
            }
            flush_tlb_all();
        }

        fn current_vmtoken() -> PhysAddr {
            TTBR1_EL1.get() as _
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            // Translations used at EL1 for the specified address, for all ASID values,
            // in the Inner Shareable shareability domain.
            if let Some(vaddr) = vaddr {
                unsafe {
                    core::arch::asm!(
                        "dsb ishst
                        tlbi vaae1is, {0}
                        dsb ish
                        isb",
                        in(reg) vaddr >> crate::PAGE_SIZE_LOG2
                    );
                }
            } else {
                flush_tlb_all();
            }
        }

        fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr) {
            let entry_count = crate::PAGE_SIZE / core::mem::size_of::<usize>();
            let entry_range = (entry_count / 2)..entry_count;
            let dst_table = unsafe { core::slice::from_raw_parts_mut(phys_to_virt(dst_pt_root) as *mut AARCH64PTE, entry_count) };
            let src_table = unsafe { core::slice::from_raw_parts(phys_to_virt(src_pt_root) as *const AARCH64PTE, entry_count) };
            for i in entry_range {
                dst_table[i] = src_table[i];
                if dst_table[i].is_unused() {
                    dst_table[i].0 |= PTF::NG.bits() as u64;
                }
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
    const ATTR_INDEX_MASK: u64 = 0b1_1100;

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
        let mut flags = Self::from_mem_type(if f.contains(MMUFlags::DEVICE) {
            MemType::Device
        } else {
            MemType::Normal
        });
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
        } else if !f.intersects(PTF::PXN) {
            ret |= Self::EXECUTE;
        }
        if f.mem_type() == MemType::Device {
            ret |= Self::DEVICE;
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
        if !is_huge {
            flags |= PTF::NON_BLOCK
        }
        self.0 = (self.0 & PHYS_ADDR_MASK as u64) | flags.bits() as u64;
    }
    fn set_table(&mut self, paddr: PhysAddr) {
        self.0 = ((paddr & PHYS_ADDR_MASK) | PTF::VALID.bits() | PTF::NON_BLOCK.bits()) as u64;
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

// Number of levels needed to cover a 48-bit VA at each granule:
//
//     4K:  12 + 4*9  ≥ 48 → 4 levels (root = VA[47:39], 9 bits)
//     16K: 14 + 4*11 ≥ 48 → 4 levels (root = VA[47],    1 bit)
//     64K: 16 + 3*13 ≥ 48 → 3 levels (root = VA[47:42], 6 bits)
//
// A 48-bit VA is not optional: `phys_to_virt` offsets physical addresses into
// the TTBR1 half, and the resulting direct-map addresses (e.g. the UART at
// 0xffff_7f03_c445_b800) have VA[47] = 0. Narrowing to a 47-bit VA (T1SZ = 17,
// 3-level) makes those addresses untranslatable, faulting on the first access
// after the kernel table is installed.
//
// The narrow root indices are handled by `root_index` in `utils::page_table`.
#[cfg(not(feature = "page-64k"))]
pub type PageTable = PageTableImpl<PageTableLevel4, AARCH64PTE>;
#[cfg(feature = "page-64k")]
pub type PageTable = PageTableImpl<PageTableLevel3, AARCH64PTE>;

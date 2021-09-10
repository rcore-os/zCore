use core::convert::TryFrom;
use core::fmt::{Debug, Formatter, Result};

use x86_64::{
    instructions::tlb,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::page_table::PageTableFlags as PTF,
};

use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel4};
use crate::{CachePolicy, MMUFlags, PhysAddr, VirtAddr};

hal_fn_impl! {
    impl mod crate::defs::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            use x86_64::structures::paging::PhysFrame;
            let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(vmtoken as _));
            if Cr3::read().0 != frame {
                unsafe { Cr3::write(frame, Cr3Flags::empty()) };
                debug!("set page_table @ {:#x}", vmtoken);
            }
        }

        fn current_vmtoken() -> PhysAddr {
            Cr3::read().0.start_address().as_u64() as _
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            if let Some(vaddr) = vaddr {
                tlb::flush(x86_64::VirtAddr::new(vaddr as u64))
            } else {
                tlb::flush_all()
            }
        }
    }
}

impl From<MMUFlags> for PTF {
    fn from(f: MMUFlags) -> Self {
        let mut flags = PTF::empty();
        if f.contains(MMUFlags::READ) {
            flags |= PTF::PRESENT;
        }
        if f.contains(MMUFlags::WRITE) {
            flags |= PTF::WRITABLE;
        }
        if !f.contains(MMUFlags::EXECUTE) {
            flags |= PTF::NO_EXECUTE;
        }
        if f.contains(MMUFlags::USER) {
            flags |= PTF::USER_ACCESSIBLE;
        }
        let cache_policy = (f.bits() & 3) as u32; // 最低三位用于储存缓存策略
        match CachePolicy::try_from(cache_policy) {
            Ok(CachePolicy::Cached) => {
                flags.remove(PTF::WRITE_THROUGH);
            }
            Ok(CachePolicy::Uncached) | Ok(CachePolicy::UncachedDevice) => {
                flags |= PTF::NO_CACHE | PTF::WRITE_THROUGH;
            }
            Ok(CachePolicy::WriteCombining) => {
                flags |= PTF::NO_CACHE | PTF::WRITE_THROUGH;
                // 当位于level=1时，页面更大，在1<<12位上（0x100）为1
                // 但是bitflags里面没有这一位。由页表自行管理标记位去吧
            }
            Err(_) => unreachable!("invalid cache policy"),
        }
        flags
    }
}

impl From<PTF> for MMUFlags {
    fn from(f: PTF) -> Self {
        let mut ret = Self::empty();
        if f.contains(PTF::PRESENT) {
            ret |= Self::READ;
        }
        if f.contains(PTF::WRITABLE) {
            ret |= Self::WRITE;
        }
        if !f.contains(PTF::NO_EXECUTE) {
            ret |= Self::EXECUTE;
        }
        if f.contains(PTF::USER_ACCESSIBLE) {
            ret |= Self::USER;
        }
        if f.contains(PTF::NO_CACHE | PTF::WRITE_THROUGH) {
            ret |= Self::CACHE_1;
        }
        ret
    }
}

const PHYS_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000; // 12..52

#[derive(Clone)]
#[repr(transparent)]
pub struct X86PTE(u64);

impl GenericPTE for X86PTE {
    fn addr(&self) -> PhysAddr {
        (self.0 & PHYS_ADDR_MASK) as _
    }
    fn flags(&self) -> MMUFlags {
        PTF::from_bits_truncate(self.0).into()
    }
    fn is_unused(&self) -> bool {
        self.0 == 0
    }
    fn is_present(&self) -> bool {
        PTF::from_bits_truncate(self.0).contains(PTF::PRESENT)
    }
    fn is_leaf(&self) -> bool {
        PTF::from_bits_truncate(self.0).contains(PTF::HUGE_PAGE)
    }

    fn set_leaf(&mut self, paddr: Option<PhysAddr>, flags: Option<MMUFlags>, is_huge: bool) {
        let paddr_bits = if let Some(paddr) = paddr {
            paddr as u64 & PHYS_ADDR_MASK
        } else {
            self.0 & PHYS_ADDR_MASK
        };
        let flags_bits = if let Some(flags) = flags {
            if is_huge {
                (PTF::from(flags) | PTF::HUGE_PAGE).bits()
            } else {
                PTF::from(flags).bits()
            }
        } else {
            self.0 & !PHYS_ADDR_MASK
        };
        self.0 = paddr_bits | flags_bits;
    }
    fn set_table(&mut self, paddr: PhysAddr) {
        self.0 = (paddr as u64 & PHYS_ADDR_MASK)
            | (PTF::PRESENT | PTF::WRITABLE | PTF::USER_ACCESSIBLE).bits();
    }
    fn clear(&mut self) {
        self.0 = 0
    }
}

impl Debug for X86PTE {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let mut f = f.debug_struct("X86PTE");
        f.field("raw", &self.0);
        f.field("addr", &self.addr());
        f.field("flags", &self.flags());
        f.finish()
    }
}

pub type PageTable = PageTableImpl<PageTableLevel4, X86PTE>;

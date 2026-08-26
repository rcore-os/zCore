//! Virtual memory operations.

use core::fmt::{Debug, Formatter, Result};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{convert::TryFrom, slice};

static KERNEL_VMTOKEN: AtomicUsize = AtomicUsize::new(0);

use x86_64::{
    instructions::tlb,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::page_table::PageTableFlags as PTF,
};

use crate::utils::page_table::{GenericPTE, PageTableImpl, PageTableLevel4};
use crate::{mem::phys_to_virt, CachePolicy, MMUFlags, PhysAddr, VirtAddr};

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            use x86_64::structures::paging::PhysFrame;
            let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(vmtoken as _));
            if Cr3::read().0 != frame {
                // Publish BEFORE the hardware switch: a TLB-shootdown initiator
                // that reads the old token and skips this CPU races a CR3 write
                // that flushes every non-global entry anyway; reading the NEW
                // token early merely costs one spurious IPI. The reverse order
                // would let an initiator skip a CPU that already runs the new
                // tables — a missed invalidation. See `remote_flush_tlb_aspace`.
                crate::common::ipi::note_active_vmtoken(frame.start_address().as_u64() as usize);
                unsafe { Cr3::write(frame, Cr3Flags::empty()) };
                debug!("set page_table @ {:#x}", vmtoken);
            }
        }

        fn current_vmtoken() -> PhysAddr {
            Cr3::read().0.start_address().as_u64() as _
        }

        fn pin_kernel_vmtoken() {
            let token = current_vmtoken();
            let prev = KERNEL_VMTOKEN.swap(token, Ordering::Release);
            if prev != 0 && prev != token {
                crate::klog_warn!(
                    "pin_kernel_vmtoken: retoken {:#x} -> {:#x}",
                    prev,
                    token
                );
            }
        }

        fn kernel_vmtoken() -> PhysAddr {
            KERNEL_VMTOKEN.load(Ordering::Acquire)
        }

        fn activate_kernel_paging() {
            let token = KERNEL_VMTOKEN.load(Ordering::Acquire);
            // Skip the CR3 write when the kernel table is already active: the
            // executor's idle callback invokes this on EVERY idle iteration,
            // and a redundant CR3 reload is a full non-global TLB flush. The
            // write is only needed to drop a lingering user CR3 (lazy TLB);
            // when CR3 already points at the kernel tree there is nothing
            // stale to release.
            if token != 0 && current_vmtoken() != token {
                activate_paging(token);
            }
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            if let Some(vaddr) = vaddr {
                let v = vaddr as u64;
                if v <= 0x0000_7fff_ffff_ffff || v >= 0xffff_8000_0000_0000 {
                    tlb::flush(x86_64::VirtAddr::new(v));
                } else {
                    warn!("flush_tlb: non-canonical vaddr {:#x}", vaddr);
                }
            } else {
                tlb::flush_all()
            }
        }

        fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr) {
            let entry_range = 0x100..0x200; // 0xFFFF_8000_0000_0000 .. 0xFFFF_FFFF_FFFF_FFFF
            let dst_table = unsafe { slice::from_raw_parts_mut(phys_to_virt(dst_pt_root) as *mut X86PTE, 512) };
            let src_table = unsafe { slice::from_raw_parts(phys_to_virt(src_pt_root) as *const X86PTE, 512) };
            for i in entry_range {
                dst_table[i] = src_table[i];
                // Do NOT set PTF::GLOBAL here. Bit 8 (the G bit of *leaf*
                // entries) is IGNORED in a PML4E on Intel but RESERVED
                // (must-be-zero) on AMD: with it set, the first hardware page
                // walk through this entry raises #PF with the RSVD error bit.
                // Every kernel address in the new user address space resolves
                // through these entries — including the fault handler itself —
                // so on an AMD CPU (QEMU/KVM or VirtualBox on an AMD host, or
                // bare metal) activating the first user CR3 escalated to a
                // triple fault and rebooted the machine right when boot
                // reached 100%. Intel silently ignored the bit, which is why
                // this only ever crashed on AMD. (See AMD APM Vol. 2 §5.3.3,
                // and KVM's `nonleaf_bit8_rsvd` in arch/x86/kvm/mmu.c.)
                // Global-TLB retention for kernel mappings, if ever wanted,
                // must be done via the G bit on leaf PTEs/PDEs instead.
            }
        }
    }
}

impl From<MMUFlags> for PTF {
    fn from(f: MMUFlags) -> Self {
        if f.is_empty() {
            return PTF::empty();
        }
        let mut flags = PTF::PRESENT;
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
        if f.is_empty() {
            return Self::empty();
        }
        let mut ret = Self::READ;
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

/// Page table entry on x86.
#[derive(Clone, Copy)]
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

    fn set_addr(&mut self, paddr: PhysAddr) {
        self.0 = (self.0 & !PHYS_ADDR_MASK) | (paddr as u64 & PHYS_ADDR_MASK);
    }
    fn set_flags(&mut self, flags: MMUFlags, is_huge: bool) {
        let mmu_flags = flags;
        let mut flags: PTF = flags.into();
        if is_huge {
            flags |= PTF::HUGE_PAGE;
        }
        let mut bits = self.addr() as u64 | flags.bits();
        // WriteCombining selects PAT entry 7 (PAT|PCD|PWT). `From<MMUFlags>`
        // already contributed PCD|PWT; the PAT bit cannot live in `PTF`
        // because its position is level-dependent — bit 7 in a 4 KiB PTE,
        // bit 12 in a 2 MiB/1 GiB leaf (bit 7 there is PS). Only emitted once
        // `pat` has actually redefined entry 7 to WC; before that, index 7 is
        // UC and plain PCD|PWT (index 3, also UC) is the honest encoding.
        let cache_policy = (mmu_flags.bits() & 3) as u32;
        if cache_policy == CachePolicy::WriteCombining as u32 && super::pat::pat_wc_ready() {
            bits |= if is_huge { 1 << 12 } else { 1 << 7 };
        }
        self.0 = bits;
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

/// The 4-level page table on x86.
pub type PageTable = PageTableImpl<PageTableLevel4, X86PTE>;

use core::convert::TryFrom;

use x86_64::{
    instructions::tlb,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{mapper, FrameAllocator, FrameDeallocator, Mapper, Translate},
    structures::paging::{OffsetPageTable, PageTable as PT, PageTableFlags as PTF},
    structures::paging::{Page, PhysFrame, Size4KiB},
};

use crate::{mem::phys_to_virt, CachePolicy, HalError, HalResult, MMUFlags, PhysAddr, VirtAddr};

fn page_table_of<'a>(root_paddr: PhysAddr) -> OffsetPageTable<'a> {
    let root_vaddr = phys_to_virt(root_paddr);
    let root = unsafe { &mut *(root_vaddr as *mut PT) };
    let offset = x86_64::VirtAddr::new(phys_to_virt(0) as u64);
    unsafe { OffsetPageTable::new(root, offset) }
}

hal_fn_impl! {
    impl mod crate::defs::vm {
        fn map_page(vmtoken: PhysAddr, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> HalResult {
            let mut pt = page_table_of(vmtoken);
            unsafe {
                pt.map_to_with_table_flags(
                    Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(vaddr as u64)).unwrap(),
                    PhysFrame::from_start_address(x86_64::PhysAddr::new(paddr as u64)).unwrap(),
                    flags.to_ptf(),
                    PTF::PRESENT | PTF::WRITABLE | PTF::USER_ACCESSIBLE,
                    &mut FrameAllocatorImpl,
                )
                .unwrap()
                .flush();
            };
            debug!(
                "map: {:x?} -> {:x?}, flags={:?} in {:#x?}",
                vaddr, paddr, flags, vmtoken
            );
            Ok(())
        }

        fn unmap_page(vmtoken: PhysAddr, vaddr: VirtAddr) -> HalResult {
            let mut pt = page_table_of(vmtoken);
            let page = Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(vaddr as u64)).unwrap();
            // This is a workaround to an issue in the x86-64 crate
            // A page without PRESENT bit is not unmappable AND mapable
            // So we add PRESENT bit here
            unsafe {
                pt.update_flags(page, PTF::PRESENT | PTF::NO_EXECUTE).ok();
            }
            match pt.unmap(page) {
                Ok((_, flush)) => {
                    flush.flush();
                    trace!("unmap: {:x?} in {:#x?}", vaddr, vmtoken);
                }
                Err(mapper::UnmapError::PageNotMapped) => {
                    trace!("unmap not mapped, skip: {:x?} in {:#x?}", vaddr, vmtoken);
                    return Ok(());
                }
                Err(err) => {
                    debug!(
                        "unmap failed: {:x?} err={:x?} in {:#x?}",
                        vaddr, err, vmtoken
                    );
                    return Err(HalError);
                }
            }
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
                let page =
                    Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(vaddr as u64)).unwrap();
                if let Ok(flush) = unsafe { pt.update_flags(page, flags.to_ptf()) } {
                    flush.flush();
                }
                trace!("protect: {:x?}, flags={:?}", vaddr, flags);
            }
            Ok(())
        }

        fn query(vmtoken: PhysAddr, vaddr: VirtAddr) -> HalResult<(PhysAddr, MMUFlags)> {
            let pt = page_table_of(vmtoken);
            let ret = pt.translate(x86_64::VirtAddr::new(vaddr as u64));
            trace!("query: {:x?} => {:x?}", vaddr, ret);
            match ret {
                mapper::TranslateResult::Mapped {
                    frame,
                    offset,
                    flags,
                } => Ok((
                    (frame.start_address().as_u64() + offset) as PhysAddr,
                    MMUFlags::from_ptf(flags),
                )),
                _ => Err(HalError),
            }
        }

        fn activate_paging(vmtoken: PhysAddr) {
            let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(vmtoken as _));
            unsafe {
                if Cr3::read().0 == frame {
                    return;
                }
                Cr3::write(frame, Cr3Flags::empty());
            }
            debug!("set page_table @ {:#x}", vmtoken);
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

trait FlagsExt {
    fn to_ptf(self) -> PTF;
    fn from_ptf(f: PTF) -> Self;
}

impl FlagsExt for MMUFlags {
    fn to_ptf(self) -> PTF {
        let mut flags = PTF::empty();
        if self.contains(MMUFlags::READ) {
            flags |= PTF::PRESENT;
        }
        if self.contains(MMUFlags::WRITE) {
            flags |= PTF::WRITABLE;
        }
        if !self.contains(MMUFlags::EXECUTE) {
            flags |= PTF::NO_EXECUTE;
        }
        if self.contains(MMUFlags::USER) {
            flags |= PTF::USER_ACCESSIBLE;
        }
        let cache_policy = (self.bits() & 3) as u32; // 最低三位用于储存缓存策略
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

    fn from_ptf(f: PTF) -> Self {
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

struct FrameAllocatorImpl;

unsafe impl FrameAllocator<Size4KiB> for FrameAllocatorImpl {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        crate::mem::frame_alloc().map(|f| {
            let paddr = x86_64::PhysAddr::new(f as u64);
            PhysFrame::from_start_address(paddr).unwrap()
        })
    }
}

impl FrameDeallocator<Size4KiB> for FrameAllocatorImpl {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame) {
        crate::mem::frame_dealloc(frame.start_address().as_u64() as PhysAddr);
    }
}

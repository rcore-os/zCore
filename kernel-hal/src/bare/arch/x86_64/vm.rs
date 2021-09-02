use core::convert::TryFrom;

use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{mapper, FrameAllocator, FrameDeallocator, Mapper, Translate},
    structures::paging::{OffsetPageTable, PageTable as X86PageTable, PageTableFlags as PTF},
    structures::paging::{Page, PhysFrame, Size4KiB},
};

use super::super::{ffi, mem::phys_to_virt};
use crate::{CachePolicy, HalError, HalResult, MMUFlags, PhysAddr, VirtAddr};

pub use crate::common::vm::*;

/// Set page table.
///
/// # Safety
/// This function will set CR3 to `vmtoken`.
pub(crate) unsafe fn set_page_table(vmtoken: usize) {
    let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(vmtoken as _));
    if Cr3::read().0 == frame {
        return;
    }
    Cr3::write(frame, Cr3Flags::empty());
    debug!("set page_table @ {:#x}", vmtoken);
}

fn frame_to_page_table(frame: PhysFrame) -> *mut X86PageTable {
    let vaddr = phys_to_virt(frame.start_address().as_u64() as usize);
    vaddr as *mut X86PageTable
}

/// Page Table
pub struct PageTable {
    root_paddr: PhysAddr,
}

impl PageTable {
    pub fn current() -> Self {
        PageTable {
            root_paddr: Cr3::read().0.start_address().as_u64() as _,
        }
    }

    /// Create a new `PageTable`.
    pub fn new() -> Self {
        let root_paddr = crate::mem::frame_alloc().expect("failed to alloc frame");
        let root_vaddr = phys_to_virt(root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut X86PageTable) };
        root.zero();
        unsafe { ffi::hal_pt_map_kernel(root_vaddr as _, frame_to_page_table(Cr3::read().0) as _) };
        trace!("create page table @ {:#x}", root_paddr);
        PageTable { root_paddr }
    }

    fn get(&mut self) -> OffsetPageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut X86PageTable) };
        let offset = x86_64::VirtAddr::new(phys_to_virt(0) as u64);
        unsafe { OffsetPageTable::new(root, offset) }
    }
}

impl PageTableTrait for PageTable {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> HalResult<()> {
        let mut pt = self.get();
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
            vaddr, paddr, flags, self.root_paddr
        );
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, vaddr: VirtAddr) -> HalResult<()> {
        let mut pt = self.get();
        let page =
            Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(vaddr as u64)).unwrap();
        // This is a workaround to an issue in the x86-64 crate
        // A page without PRESENT bit is not unmappable AND mapable
        // So we add PRESENT bit here
        unsafe {
            pt.update_flags(page, PTF::PRESENT | PTF::NO_EXECUTE).ok();
        }
        match pt.unmap(page) {
            Ok((_, flush)) => {
                flush.flush();
                trace!("unmap: {:x?} in {:#x?}", vaddr, self.root_paddr);
            }
            Err(mapper::UnmapError::PageNotMapped) => {
                trace!(
                    "unmap not mapped, skip: {:x?} in {:#x?}",
                    vaddr,
                    self.root_paddr
                );
                return Ok(());
            }
            Err(err) => {
                debug!(
                    "unmap failed: {:x?} err={:x?} in {:#x?}",
                    vaddr, err, self.root_paddr
                );
                return Err(HalError);
            }
        }
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> HalResult<()> {
        let mut pt = self.get();
        let page =
            Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(vaddr as u64)).unwrap();
        if let Ok(flush) = unsafe { pt.update_flags(page, flags.to_ptf()) } {
            flush.flush();
        }
        trace!("protect: {:x?}, flags={:?}", vaddr, flags);
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&mut self, vaddr: VirtAddr) -> HalResult<PhysAddr> {
        let pt = self.get();
        let ret = pt
            .translate_addr(x86_64::VirtAddr::new(vaddr as u64))
            .map(|addr| addr.as_u64() as PhysAddr)
            .ok_or(HalError);
        trace!("query: {:x?} => {:x?}", vaddr, ret);
        ret
    }

    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
    }
}

trait FlagsExt {
    fn to_ptf(self) -> PTF;
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

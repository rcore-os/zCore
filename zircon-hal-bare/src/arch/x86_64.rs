use super::*;
use x86_64::structures::paging::{PageTableFlags as PTF, *};

type MMUFlags = usize;

/// Page Table
#[repr(C)]
pub struct PageTableImpl {
    root: &'static mut PageTable,
}

impl PageTableImpl {
    /// Create a new `PageTable`.
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        let root_frame = Frame::alloc().expect("failed to alloc frame");
        let root_vaddr = phys_to_virt(root_frame.paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        root.zero();
        map_kernel(root_vaddr as _);
        PageTableImpl { root }
    }

    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    pub fn map(
        &mut self,
        vaddr: x86_64::VirtAddr,
        paddr: x86_64::PhysAddr,
        _flags: MMUFlags,
    ) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::<Size4KiB>::from_start_address(vaddr).unwrap();
        let frame = unsafe { UnusedPhysFrame::new(PhysFrame::from_start_address(paddr).unwrap()) };
        let flags = PTF::PRESENT | PTF::WRITABLE;
        pt.map_to(page, frame, flags, &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, vaddr: x86_64::VirtAddr) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::<Size4KiB>::from_start_address(vaddr).unwrap();
        pt.unmap(page).unwrap().1.flush();
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, _vaddr: x86_64::VirtAddr, _flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, _vaddr: x86_64::VirtAddr) -> Result<(PhysAddr, MMUFlags), ()> {
        unimplemented!()
    }

    fn get(&mut self) -> OffsetPageTable<'_> {
        let offset = x86_64::VirtAddr::new(PMEM_BASE as u64);
        unsafe { OffsetPageTable::new(self.root, offset) }
    }
}

struct FrameAllocatorImpl;

unsafe impl FrameAllocator<Size4KiB> for FrameAllocatorImpl {
    fn allocate_frame(&mut self) -> Option<UnusedPhysFrame> {
        Frame::alloc().map(|f| unsafe {
            let paddr = x86_64::PhysAddr::new(f.paddr as u64);
            UnusedPhysFrame::new(PhysFrame::from_start_address(paddr).unwrap())
        })
    }
}

impl FrameDeallocator<Size4KiB> for FrameAllocatorImpl {
    fn deallocate_frame(&mut self, frame: UnusedPhysFrame) {
        Frame {
            paddr: frame.frame().start_address().as_u64() as usize,
        }
        .dealloc()
    }
}

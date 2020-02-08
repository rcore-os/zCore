use {
    super::*,
    apic::{LocalApic, XApic},
    bitflags::bitflags,
    core::fmt::{Arguments, Write},
    spin::Mutex,
    uart_16550::SerialPort,
    x86_64::{
        registers::control::Cr3,
        structures::paging::{PageTableFlags as PTF, *},
    },
};

extern "C" {
    #[link_name = "hal_lapic_addr"]
    static LAPIC_ADDR: usize;
}

/// Page Table
#[repr(C)]
pub struct PageTableImpl {
    root_paddr: PhysAddr,
}

impl PageTableImpl {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        let root_frame = Frame::alloc().expect("failed to alloc frame");
        let root_vaddr = phys_to_virt(root_frame.paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        root.zero();
        map_kernel(root_vaddr as _);
        trace!("create page table @ {:#x}", root_frame.paddr);
        PageTableImpl {
            root_paddr: root_frame.paddr,
        }
    }

    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    pub fn map(
        &mut self,
        vaddr: x86_64::VirtAddr,
        paddr: x86_64::PhysAddr,
        flags: MMUFlags,
    ) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::<Size4KiB>::from_start_address(vaddr).unwrap();
        let frame = unsafe { UnusedPhysFrame::new(PhysFrame::from_start_address(paddr).unwrap()) };
        pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
        trace!("map: {:x?} -> {:x?}, flags={:?}", vaddr, paddr, flags);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, vaddr: x86_64::VirtAddr) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::<Size4KiB>::from_start_address(vaddr).unwrap();
        pt.unmap(page).unwrap().1.flush();
        trace!("unmap: {:x?}", vaddr);
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, vaddr: x86_64::VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::<Size4KiB>::from_start_address(vaddr).unwrap();
        pt.update_flags(page, flags.to_ptf()).unwrap().flush();
        trace!("protect: {:x?}, flags={:?}", vaddr, flags);
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, vaddr: x86_64::VirtAddr) -> Result<x86_64::PhysAddr, ()> {
        let pt = self.get();
        let ret = pt.translate_addr(vaddr).ok_or(());
        trace!("query: {:x?} => {:x?}", vaddr, ret);
        ret
    }

    fn get(&mut self) -> OffsetPageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        let offset = x86_64::VirtAddr::new(phys_to_virt(0) as u64);
        unsafe { OffsetPageTable::new(root, offset) }
    }
}

pub fn kernel_root_table() -> &'static PageTable {
    unsafe { &*frame_to_page_table(Cr3::read().0) }
}

fn frame_to_page_table(frame: PhysFrame) -> *mut PageTable {
    let vaddr = phys_to_virt(frame.start_address().as_u64() as usize);
    vaddr as *mut PageTable
}

bitflags! {
    pub struct MMUFlags: usize {
        #[allow(clippy::identity_op)]
        const READ      = 1 << 0;
        const WRITE     = 1 << 1;
        const EXECUTE   = 1 << 2;
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
        flags
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

static COM1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });

pub fn putfmt(fmt: Arguments) {
    unsafe {
        COM1.force_unlock();
    }
    COM1.lock().write_fmt(fmt).unwrap();
}

#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    unsafe {
        COM1.force_unlock();
    }
    for byte in s.bytes() {
        COM1.lock().send(byte);
    }
}

pub fn timer_init() {
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.cpu_init();
}

#[inline(always)]
pub fn ack(_irq: u8) {
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.eoi();
}

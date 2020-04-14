use {
    super::*,
    apic::{LocalApic, XApic},
    core::fmt::{Arguments, Write},
    core::time::Duration,
    rcore_console::{Console, ConsoleOnGraphic, DrawTarget, Pixel, Rgb888, Size},
    spin::Mutex,
    uart_16550::SerialPort,
    x86_64::{
        registers::control::{Cr3, Cr3Flags, Cr4, Cr4Flags},
        structures::paging::{PageTableFlags as PTF, *},
    },
};

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
        map_kernel(root_vaddr as _, frame_to_page_table(Cr3::read().0) as _);
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
        let flush = pt
            .map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap();
        if flags.contains(MMUFlags::USER) {
            self.allow_user_access(vaddr);
        }
        flush.flush();
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
        let flush = pt.update_flags(page, flags.to_ptf()).unwrap();
        if flags.contains(MMUFlags::USER) {
            self.allow_user_access(vaddr);
        }
        flush.flush();
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

    /// Set user bit for 4-level PDEs of the page of `vaddr`.
    ///
    /// This is a workaround since `x86_64` crate does not set user bit for PDEs.
    fn allow_user_access(&mut self, vaddr: x86_64::VirtAddr) {
        let mut page_table = phys_to_virt(self.root_paddr) as *mut PageTable;
        for level in 0..4 {
            let index = (vaddr.as_u64() as usize >> (12 + (3 - level) * 9)) & 0o777;
            let entry = unsafe { &mut (&mut *page_table)[index] };
            let flags = entry.flags();
            entry.set_flags(flags | PTF::USER_ACCESSIBLE);
            if level == 3 || flags.contains(PTF::HUGE_PAGE) {
                return;
            }
            page_table = frame_to_page_table(entry.frame().unwrap());
        }
    }
}

/// Set page table.
///
/// # Safety
/// This function will set CR3 to `vmtoken`.
pub unsafe fn set_page_table(vmtoken: usize) {
    let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(vmtoken as _));
    if Cr3::read().0 == frame {
        return;
    }
    Cr3::write(frame, Cr3Flags::empty());
}

fn frame_to_page_table(frame: PhysFrame) -> *mut PageTable {
    let vaddr = phys_to_virt(frame.start_address().as_u64() as usize);
    vaddr as *mut PageTable
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

static CONSOLE: Mutex<Option<ConsoleOnGraphic<Framebuffer>>> = Mutex::new(None);

struct Framebuffer {
    width: u32,
    height: u32,
    buf: &'static mut [u32],
}

impl DrawTarget<Rgb888> for Framebuffer {
    type Error = core::convert::Infallible;

    fn draw_pixel(&mut self, item: Pixel<Rgb888>) -> Result<(), Self::Error> {
        let idx = (item.0.x as u32 + item.0.y as u32 * self.width) as usize;
        self.buf[idx] = unsafe { core::mem::transmute(item.1) };
        Ok(())
    }

    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

/// Initialize console on framebuffer.
pub fn init_framebuffer(width: u32, height: u32, paddr: PhysAddr) {
    let fb = Framebuffer {
        width,
        height,
        buf: unsafe {
            core::slice::from_raw_parts_mut(
                phys_to_virt(paddr) as *mut u32,
                (width * height) as usize,
            )
        },
    };
    let console = Console::on_frame_buffer(fb);
    *CONSOLE.lock() = Some(console);
}

static COM1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });

pub fn putfmt(fmt: Arguments) {
    COM1.lock().write_fmt(fmt).unwrap();
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.write_fmt(fmt).unwrap();
    }
}

#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    putfmt(format_args!("{}", s));
}

fn tsc_frequency() -> u16 {
    match raw_cpuid::CpuId::new().get_processor_frequency_info() {
        Some(info) => info.processor_base_frequency(),
        None => 3000, // QEMU
    }
}

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    Duration::from_nanos(tsc * 1000 / tsc_frequency() as u64)
}

fn timer_init() {
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.cpu_init();
}

#[export_name = "hal_irq_ack"]
pub fn irq_ack(_irq: u8) {
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.eoi();
}

const LAPIC_ADDR: usize = 0xfee0_0000;

#[export_name = "hal_vdso_constants"]
fn vdso_constants() -> VdsoConstants {
    let tsc_frequency = tsc_frequency();
    VdsoConstants {
        max_num_cpus: 1,
        features: Features {
            cpu: 0,
            hw_breakpoint_count: 0,
            hw_watchpoint_count: 0,
        },
        dcache_line_size: 0,
        icache_line_size: 0,
        ticks_per_second: tsc_frequency as u64 * 1_000_000,
        ticks_to_mono_numerator: 1000,
        ticks_to_mono_denominator: tsc_frequency as u32,
        physmem: 0,
        buildid: Default::default(),
    }
}

/// Initialize the HAL.
pub fn init() {
    timer_init();
    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
    }
}

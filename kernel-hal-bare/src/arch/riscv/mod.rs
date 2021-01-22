use super::super::*;
use kernel_hal::{PageTableTrait, PhysAddr, VirtAddr};
use riscv::addr::Page;
use riscv::paging::{PageTableFlags as PTF, *};
use riscv::register::{time, satp, sie};
//use crate::sbi;
use core::fmt::{ self, Write };
use alloc::{collections::VecDeque, vec::Vec};

mod sbi;

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
        let current =
            phys_to_virt(satp::read().frame().start_address().as_usize()) as *const PageTable;
        map_kernel(root_vaddr as _, current as _);
        trace!("create page table @ {:#x}", root_frame.paddr);
        PageTableImpl {
            root_paddr: root_frame.paddr,
        }
    }

    #[cfg(target_arch = "riscv32")]
    fn get(&mut self) -> Rv32PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        Rv32PageTable::new(root, phys_to_virt(0))
    }

    #[cfg(target_arch = "riscv64")]
    fn get(&mut self) -> Rv39PageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        Rv39PageTable::new(root, phys_to_virt(0))
    }
}

impl PageTableTrait for PageTableImpl {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(paddr));
        pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
        trace!("map: {:x?} -> {:x?}, flags={:?}", vaddr, paddr, flags);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.unmap(page).unwrap().1.flush();
        trace!("unmap: {:x?}", vaddr);
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.update_flags(page, flags.to_ptf()).unwrap().flush();
        trace!("protect: {:x?}, flags={:?}", vaddr, flags);
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let res = pt.ref_entry(page);
        trace!("query: {:x?} => {:x?}", vaddr, res);
        match res {
            Ok(entry) => Ok(entry.addr().as_usize()),
            Err(_) => Err(()),
        }
    }

    /// Get the physical address of root page table.
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
    }
}

pub unsafe fn set_page_table(vmtoken: usize) {
    #[cfg(target_arch = "riscv32")]
    let mode = satp::Mode::Sv32;
    #[cfg(target_arch = "riscv64")]
    let mode = satp::Mode::Sv39;
    satp::set(mode, 0, vmtoken >> 12);
}

trait FlagsExt {
    fn to_ptf(self) -> PTF;
}

impl FlagsExt for MMUFlags {
    fn to_ptf(self) -> PTF {
        let mut flags = PTF::VALID;
        if self.contains(MMUFlags::READ) {
            flags |= PTF::READABLE;
        }
        if self.contains(MMUFlags::WRITE) {
            flags |= PTF::WRITABLE;
        }
        if self.contains(MMUFlags::EXECUTE) {
            flags |= PTF::EXECUTABLE;
        }
        if self.contains(MMUFlags::USER) {
            flags |= PTF::USER;
        }
        flags
    }
}

struct FrameAllocatorImpl;

impl FrameAllocator for FrameAllocatorImpl {
    fn alloc(&mut self) -> Option<riscv::addr::Frame> {
        Frame::alloc().map(|f| {
            let paddr = riscv::addr::PhysAddr::new(f.paddr);
            riscv::addr::Frame::of_addr(paddr)
        })
    }
}

impl FrameDeallocator for FrameAllocatorImpl {
    fn dealloc(&mut self, frame: riscv::addr::Frame) {
        Frame {
            paddr: frame.start_address().as_usize(),
        }
        .dealloc()
    }
}

lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> = Mutex::new(Vec::new());
}

//调用这里
/// Put a char by serial interrupt handler.
fn serial_put(mut x: u8) {
    if x == b'\r' {
        x = b'\n';
    }
    STDIN.lock().push_back(x);
    STDIN_CALLBACK.lock().retain(|f| !f());
}

#[export_name = "hal_serial_set_callback"]
pub fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
    STDIN_CALLBACK.lock().push(callback);
}

#[export_name = "hal_serial_read"]
pub fn serial_read(buf: &mut [u8]) -> usize {
    let mut stdin = STDIN.lock();
    let len = stdin.len().min(buf.len());
    for c in &mut buf[..len] {
        *c = stdin.pop_front().unwrap();
    }
    len
}

#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    putfmt(format_args!("{}", s));
}

// Get TSC frequency.
fn tsc_frequency() -> u16 {
    const DEFAULT: u16 = 2600;

    // FIXME: QEMU, AMD, VirtualBox
    DEFAULT
}

#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    let lapic = 0;
    lapic as u8
}

////////////

pub fn putchar(ch: char){
	sbi::console_putchar(ch as u8 as usize);
}

pub fn puts(s: &str){
	for ch in s.chars(){
		putchar(ch);
	}
}

struct Stdout;

impl fmt::Write for Stdout {
	fn write_str(&mut self, s: &str) -> fmt::Result {
		puts(s);
		Ok(())
	}
}

pub fn putfmt(fmt: fmt::Arguments) {
	Stdout.write_fmt(fmt).unwrap();
}

#[macro_export]
macro_rules! bare_print {
	($($arg:tt)*) => ({
        putfmt(format_args!($($arg)*));
	});
}

#[macro_export]
macro_rules! bare_println {
	() => (bare_print!("\n"));
	($($arg:tt)*) => (bare_print!("{}\n", format_args!($($arg)*)));
}

pub const MMIO_MTIMECMP0: *mut u64 = 0x0200_4000usize as *mut u64;
pub const MMIO_MTIME: *const u64 = 0x0200_BFF8 as *const u64;

fn get_cycle() -> u64 {
    time::read() as u64
    /*
    unsafe {
        MMIO_MTIME.read_volatile()
    }
    */
}

pub fn timer_now() -> Duration {
    const FREQUENCY: u64 = 10_000_000; // ???
    let time = get_cycle();
    //bare_println!("timer_now(): {:?}", time);
    Duration::from_nanos(time * 1_000_000_000 / FREQUENCY as u64)
}

fn clock_set_next_event() {
    let TIMEBASE: u64 = 100000;
    sbi::set_timer(get_cycle() + TIMEBASE);
}

fn timer_init() {
    unsafe {
        sie::set_stimer();
    }
    clock_set_next_event();
}

pub fn init(config: Config) {
    /*
    timer_init();
    interrupt::init();
    serial
    */

}

pub struct Config {
    pub mconfig: u64,
}

static mut CONFIG: Config = Config {
    mconfig: 0,
};

/// This structure represents the information that the bootloader passes to the kernel.
#[repr(C)]
#[derive(Debug)]
pub struct BootInfo {
    pub memory_map: Vec<u64>,
    //pub memory_map: Vec<&'static MemoryDescriptor>,
    /// The offset into the virtual address space where the physical memory is mapped.
    pub physical_memory_offset: u64,
    /// The graphic output information
    pub graphic_info: GraphicInfo,
    /// Physical address of ACPI2 RSDP
    pub acpi2_rsdp_addr: u64,
    /// Physical address of SMBIOS
    pub smbios_addr: u64,
    /// The start physical address of initramfs
    pub initramfs_addr: u64,
    /// The size of initramfs
    pub initramfs_size: u64,
    /// Kernel command line
    pub cmdline: &'static str,
}

/// Graphic output information
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct GraphicInfo {
    /// Graphic mode
    //pub mode: ModeInfo,
    pub mode: u64,
    /// Framebuffer base physical address
    pub fb_addr: u64,
    /// Framebuffer size
    pub fb_size: u64,
}



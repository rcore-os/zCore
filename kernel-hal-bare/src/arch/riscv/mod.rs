use super::super::*;
use kernel_hal::{HalError, PageTableTrait, PhysAddr, VirtAddr};
use riscv::addr::Page;
use riscv::asm::sfence_vma_all;
use riscv::paging::{PageTableFlags as PTF, *};
use riscv::register::{satp, sie, stval, time};
//use crate::sbi;
use alloc::{collections::VecDeque, vec::Vec};
use core::fmt::{self, Write};

mod sbi;

mod consts;

use consts::PHYSICAL_MEMORY_OFFSET;

// First core stores its SATP here.
static mut SATP: usize = 0;

/// remap kernel with 4K page
pub fn remap_the_kernel(dtb: usize) {
    let root_frame = Frame::alloc().expect("failed to alloc frame");
    let root_vaddr = phys_to_virt(root_frame.paddr);
    let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
    root.zero();
    let mut pt = Rv39PageTable::new(root, PHYSICAL_MEMORY_OFFSET);

    let linear_offset = PHYSICAL_MEMORY_OFFSET;
    //let mut flags = PTF::VALID | PTF::READABLE | PTF::WRITABLE | PTF::EXECUTABLE | PTF::USER;

    map_range(
        &mut pt,
        stext as usize,
        etext as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::EXECUTABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        srodata as usize,
        erodata as usize,
        linear_offset,
        PTF::VALID | PTF::READABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        sdata as usize,
        edata as usize,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // Stack
    map_range(
        &mut pt,
        bootstack as usize,
        bootstacktop as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    map_range(
        &mut pt,
        sbss as usize,
        ebss as usize - 1,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // Heap
    map_range(
        &mut pt,
        end as usize,
        end as usize + PAGE_SIZE * 512,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // Device Tree
    map_range(
        &mut pt,
        dtb,
        dtb + consts::MAX_DTB_SIZE,
        linear_offset,
        PTF::VALID | PTF::READABLE,
    )
    .unwrap();

    // CLINT
    map_range(
        &mut pt,
        0x2000000 + PHYSICAL_MEMORY_OFFSET,
        0x2010000 + PHYSICAL_MEMORY_OFFSET,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // PLIC
    map_range(
        &mut pt,
        0xc000000 + PHYSICAL_MEMORY_OFFSET,
        0xc00f000 + PHYSICAL_MEMORY_OFFSET,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();
    map_range(
        &mut pt,
        0xc200000 + PHYSICAL_MEMORY_OFFSET,
        0xc20f000 + PHYSICAL_MEMORY_OFFSET,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    // UART0, VIRTIO
    map_range(
        &mut pt,
        0x10000000 + PHYSICAL_MEMORY_OFFSET,
        0x1000f000 + PHYSICAL_MEMORY_OFFSET,
        linear_offset,
        PTF::VALID | PTF::READABLE | PTF::WRITABLE,
    )
    .unwrap();

    //写satp
    let token = root_frame.paddr;
    unsafe {
        set_page_table(token);
        SATP = token;
    }

    //use core::mem;
    //mem::forget(pt);

    info!("remap the kernel @ {:#x}", token);
}

pub fn map_range(
    page_table: &mut Rv39PageTable,
    mut start_addr: VirtAddr,
    mut end_addr: VirtAddr,
    linear_offset: usize,
    flags: PageTableFlags,
) -> Result<(), ()> {
    trace!("Mapping range addr: {:#x} ~ {:#x}", start_addr, end_addr);

    start_addr = start_addr & !(PAGE_SIZE - 1);
    let mut start_page = start_addr / PAGE_SIZE;

    //end_addr = (end_addr + PAGE_SIZE - 1) & !(PAGE_SIZE -1);
    //let end_page = (end_addr - 1) / PAGE_SIZE;
    end_addr = end_addr & !(PAGE_SIZE - 1);
    let end_page = end_addr / PAGE_SIZE;

    while start_page <= end_page {
        let vaddr: VirtAddr = start_page * PAGE_SIZE;
        let page = riscv::addr::Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(vaddr - linear_offset));

        start_page += 1;

        trace!(
            "map_range: {:#x} -> {:#x}, flags={:?}",
            vaddr,
            vaddr - linear_offset,
            flags
        );
        page_table
            .map_to(page, frame, flags, &mut FrameAllocatorImpl)
            .unwrap()
            .flush();
    }
    info!(
        "map range from {:#x} to {:#x}, flags: {:?}",
        start_addr,
        end_page * PAGE_SIZE,
        flags
    );

    Ok(())
}

extern "C" {
    fn start();

    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();

    fn bootstack();
    fn bootstacktop();

    fn sbss();
    fn ebss();

    fn end();
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
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let frame = riscv::addr::Frame::of_addr(riscv::addr::PhysAddr::new(paddr));
        pt.map_to(page, frame, flags.to_ptf(), &mut FrameAllocatorImpl)
            .unwrap()
            .flush();

        debug!(
            "PageTable: {:#X}, map: {:x?} -> {:x?}, flags={:?}",
            self.table_phys() as usize,
            vaddr,
            paddr,
            flags
        );
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.unmap(page).unwrap().1.flush();
        trace!(
            "PageTable: {:#X}, unmap: {:x?}",
            self.table_phys() as usize,
            vaddr
        );
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        pt.update_flags(page, flags.to_ptf()).unwrap().flush();

        if vaddr == 0x11b000 {
            info!("protect 0x11b3c0: {:#X?}", self.query(0x11b3c0));
        } else if vaddr == 0xc4000 {
            info!("protect 0xc44b6: {:#X?}", self.query(0xc44b6));
        }
        trace!(
            "PageTable: {:#X}, protect: {:x?}, flags={:?}",
            self.table_phys() as usize,
            vaddr,
            flags
        );
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, HalError> {
        let mut pt = self.get();
        let page = Page::of_addr(riscv::addr::VirtAddr::new(vaddr));
        let res = pt.ref_entry(page);
        trace!("query: {:x?} => {:#x?}", vaddr, res);
        match res {
            Ok(entry) => Ok(entry.addr().as_usize()),
            Err(_) => Err(HalError),
        }
    }

    /// Get the physical address of root page table.
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
    }

    /// Activate this page table
    #[export_name = "hal_pt_activate"]
    fn activate(&self) {
        let now_token = satp::read().bits();
        let new_token = self.table_phys();
        if now_token != new_token {
            debug!("switch table {:x?} -> {:x?}", now_token, new_token);
            unsafe {
                set_page_table(new_token);
            }
        }
    }
}

pub unsafe fn set_page_table(vmtoken: usize) {
    #[cfg(target_arch = "riscv32")]
    let mode = satp::Mode::Sv32;
    #[cfg(target_arch = "riscv64")]
    let mode = satp::Mode::Sv39;
    debug!("set user table: {:#x?}", vmtoken);
    satp::set(mode, 0, vmtoken >> 12);
    //刷TLB好像很重要
    sfence_vma_all();
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
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
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
    //putfmt(format_args!("{}", s));
    putfmt_uart(format_args!("{}", s));
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

pub fn getchar_option() -> Option<u8> {
    let c = sbi::console_getchar() as isize;
    match c {
        -1 => None,
        c => Some(c as u8),
    }
}

////////////

pub fn putchar(ch: char) {
    sbi::console_putchar(ch as u8 as usize);
}

pub fn puts(s: &str) {
    for ch in s.chars() {
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
////////////

struct Stdout1;
impl fmt::Write for Stdout1 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        //每次都创建一个新的Uart ? 内存位置始终相同
        write!(
            uart::Uart::new(0x1000_0000 + PHYSICAL_MEMORY_OFFSET),
            "{}",
            s
        )
        .unwrap();

        Ok(())
    }
}
pub fn putfmt_uart(fmt: fmt::Arguments) {
    Stdout1.write_fmt(fmt).unwrap();
}

////////////

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

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    const FREQUENCY: u64 = 10_000_000; // ???
    let time = get_cycle();
    //bare_println!("timer_now(): {:?}", time);
    Duration::from_nanos(time * 1_000_000_000 / FREQUENCY as u64)
}

#[export_name = "hal_timer_set_next"]
fn timer_set_next() {
    //let TIMEBASE: u64 = 100000;
    let TIMEBASE: u64 = 10_000_000;
    sbi::set_timer(get_cycle() + TIMEBASE);
}

fn timer_init() {
    unsafe {
        sie::set_stimer();
    }
    timer_set_next();
}

pub fn init(config: Config) {
    interrupt::init();
    timer_init();

    /*
    interrupt::init_soft();
    sbi::send_ipi(0);
    */

    unsafe {
        llvm_asm!("ebreak"::::"volatile");
    }

    bare_println!("Setup virtio @devicetree {:#x}", config.dtb);
    //virtio::init(config.dtb);

    virtio::device_tree::init(config.dtb);
}

pub struct Config {
    pub mconfig: u64,
    pub dtb: usize,
}

#[export_name = "fetch_fault_vaddr"]
pub fn fetch_fault_vaddr() -> VirtAddr {
    stval::read() as _
}

static mut CONFIG: Config = Config { mconfig: 0, dtb: 0 };

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

    /// Physical address of ACPI2 RSDP, 启动的系统信息表的入口指针
    //pub acpi2_rsdp_addr: u64,
    /// Physical address of SMBIOS, 产品管理信息的结构表
    //pub smbios_addr: u64,
    pub hartid: u64,
    pub dtb_addr: u64,

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

pub mod interrupt;
mod plic;
mod uart;

pub mod virtio;

#[export_name = "hal_current_pgtable"]
pub fn current_page_table() -> usize {
    #[cfg(target_arch = "riscv32")]
    let mode = satp::Mode::Sv32;
    #[cfg(target_arch = "riscv64")]
    let mode = satp::Mode::Sv39;
    satp::read().ppn() << 12
}

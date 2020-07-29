use {
    super::super::*,
    acpi::{parse_rsdp, Acpi, AcpiHandler, PhysicalMapping},
    alloc::{collections::VecDeque, vec::Vec},
    apic::{LocalApic, XApic},
    core::arch::x86_64::{__cpuid, _mm_clflush, _mm_mfence},
    core::convert::TryFrom,
    core::fmt::{Arguments, Write},
    core::ptr::NonNull,
    core::time::Duration,
    git_version::git_version,
    kernel_hal::PageTableTrait,
    rcore_console::{Console, ConsoleOnGraphic, DrawTarget, Pixel, Rgb888, Size},
    spin::Mutex,
    uart_16550::SerialPort,
    x86_64::{
        instructions::port::Port,
        registers::control::{Cr2, Cr3, Cr3Flags, Cr4, Cr4Flags},
        structures::paging::{PageTableFlags as PTF, *},
    },
};

use kernel_hal::vdso::{Features, VdsoConstants};

mod acpi_table;
pub mod interrupt;
mod keyboard;

pub use super::super::phys_to_virt;

/// Page Table
#[repr(C)]
pub struct PageTableImpl {
    root_paddr: PhysAddr,
}

impl PageTableImpl {
    #[export_name = "hal_pt_current"]
    pub fn current() -> Self {
        PageTableImpl {
            root_paddr: Cr3::read().0.start_address().as_u64() as _,
        }
    }

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

    fn get(&mut self) -> OffsetPageTable<'_> {
        let root_vaddr = phys_to_virt(self.root_paddr);
        let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
        let offset = x86_64::VirtAddr::new(phys_to_virt(0) as u64);
        unsafe { OffsetPageTable::new(root, offset) }
    }
}

impl PageTableTrait for PageTableImpl {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
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
        trace!(
            "map: {:x?} -> {:x?}, flags={:?} in {:#x?}",
            vaddr,
            paddr,
            flags,
            self.root_paddr
        );
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
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
            Err(err) => {
                debug!(
                    "unmap failed: {:x?} err={:x?} in {:#x?}",
                    vaddr, err, self.root_paddr
                );
                return Err(());
            }
        }
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
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
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        let pt = self.get();
        let ret = pt
            .translate_addr(x86_64::VirtAddr::new(vaddr as u64))
            .map(|addr| addr.as_u64() as PhysAddr).ok_or(());
        trace!("query: {:x?} => {:x?}", vaddr, ret);
        ret
    }

    /// Get the physical address of root page table.
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.root_paddr
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
        Frame::alloc().map(|f| {
            let paddr = x86_64::PhysAddr::new(f.paddr as u64);
            PhysFrame::from_start_address(paddr).unwrap()
        })
    }
}

impl FrameDeallocator<Size4KiB> for FrameAllocatorImpl {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame) {
        Frame {
            paddr: frame.start_address().as_u64() as usize,
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

lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn FnOnce() + Send + Sync>>> = Mutex::new(Vec::new());
}

/// Put a char by serial interrupt handler.
fn serial_put(mut x: u8) {
    if x == b'\r' {
        x = b'\n';
    }
    STDIN.lock().push_back(x);
    for callback in STDIN_CALLBACK.lock().drain(..) {
        callback();
    }
}

#[export_name = "hal_serial_set_callback"]
pub fn serial_set_callback(callback: Box<dyn FnOnce() + Send + Sync>) {
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

/// Get TSC frequency.
///
/// WARN: This will be very slow on virtual machine since it uses CPUID instruction.
fn tsc_frequency() -> u16 {
    const DEFAULT: u16 = 2600;
    if let Some(info) = raw_cpuid::CpuId::new().get_processor_frequency_info() {
        let f = info.processor_base_frequency();
        return if f == 0 { DEFAULT } else { f };
    }
    // FIXME: QEMU, AMD, VirtualBox
    DEFAULT
}

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    Duration::from_nanos(tsc * 1000 / unsafe { TSC_FREQUENCY } as u64)
}

fn timer_init() {
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.cpu_init();
}

#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    let lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.id() as u8
}

const LAPIC_ADDR: usize = 0xfee0_0000;
const IOAPIC_ADDR: usize = 0xfec0_0000;

#[export_name = "hal_vdso_constants"]
fn vdso_constants() -> VdsoConstants {
    let tsc_frequency = unsafe { TSC_FREQUENCY };
    let mut constants = VdsoConstants {
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
        version_string_len: 0,
        version_string: Default::default(),
    };
    constants.set_version_string(git_version!(
        prefix = "git-",
        args = ["--always", "--abbrev=40", "--dirty=-dirty"]
    ));
    constants
}

/// Initialize the HAL.
pub fn init(config: Config) {
    timer_init();
    interrupt::init();
    COM1.lock().init();
    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
        // store config
        CONFIG = config;
        // get tsc frequency
        TSC_FREQUENCY = tsc_frequency();

        // start multi-processors
        fn ap_main() {
            info!("processor {} started", apic_local_id());
            unsafe {
                trapframe::init();
            }
            timer_init();
            let ap_fn = unsafe { CONFIG.ap_fn };
            ap_fn()
        }
        fn stack_fn(pid: usize) -> usize {
            // split and reuse the current stack
            unsafe {
                let mut stack: usize;
                asm!("mov {}, rsp", out(reg) stack);
                stack -= 0x4000 * pid;
                stack
            }
        }
        x86_smpboot::start_application_processors(ap_main, stack_fn, phys_to_virt);
    }
}

/// Configuration of HAL.
pub struct Config {
    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}

#[export_name = "fetch_fault_vaddr"]
pub fn fetch_fault_vaddr() -> VirtAddr {
    Cr2::read().as_u64() as _
}

/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
#[export_name = "hal_pc_firmware_tables"]
pub fn pc_firmware_tables() -> (u64, u64) {
    unsafe { (CONFIG.acpi_rsdp, CONFIG.smbios) }
}

static mut CONFIG: Config = Config {
    acpi_rsdp: 0,
    smbios: 0,
    ap_fn: || unreachable!(),
};

static mut TSC_FREQUENCY: u16 = 2600;

/// Build ACPI Table
struct AcpiHelper {}
impl AcpiHandler for AcpiHelper {
    unsafe fn map_physical_region<T>(
        &mut self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<T> {
        #[allow(non_snake_case)]
        let OFFSET = 0;
        let page_start = physical_address / PAGE_SIZE;
        let page_end = (physical_address + size + PAGE_SIZE - 1) / PAGE_SIZE;
        PhysicalMapping::<T> {
            physical_start: physical_address,
            virtual_start: NonNull::new_unchecked(phys_to_virt(physical_address + OFFSET) as *mut T),
            mapped_length: size,
            region_length: PAGE_SIZE * (page_end - page_start),
        }
    }
    fn unmap_physical_region<T>(&mut self, _region: PhysicalMapping<T>) {}
}

#[export_name = "hal_acpi_table"]
pub fn get_acpi_table() -> Option<Acpi> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut handler = AcpiHelper {};
        match unsafe { parse_rsdp(&mut handler, pc_firmware_tables().0 as usize) } {
            Ok(table) => Some(table),
            Err(info) => {
                warn!("get_acpi_table error: {:#x?}", info);
                None
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    None
}

/// IO Port in/out instruction
#[export_name = "hal_outpd"]
pub fn outpd(port: u16, value: u32) {
    unsafe {
        Port::new(port).write(value);
    }
}

#[export_name = "hal_inpd"]
pub fn inpd(port: u16) -> u32 {
    unsafe { Port::new(port).read() }
}

/// Flush the physical frame.
#[export_name = "hal_frame_flush"]
pub fn frame_flush(target: PhysAddr) {
    unsafe {
        for paddr in (target..target + PAGE_SIZE).step_by(cacheline_size()) {
            _mm_clflush(phys_to_virt(paddr) as *const u8);
        }
        _mm_mfence();
    }
}

/// Get cache line size in bytes.
fn cacheline_size() -> usize {
    let leaf = unsafe { __cpuid(1).ebx };
    (((leaf >> 8) & 0xff) << 3) as usize
}

use super::*;
use crate::vdso::VdsoConstants;
use acpi::Acpi;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use core::ops::FnOnce;
use core::pin::Pin;
use core::time::Duration;

type ThreadId = usize;

#[repr(C)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    /// Spawn a new thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(
        _future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        _vmtoken: usize,
    ) -> Self {
        unimplemented!()
    }

    /// Set tid and pid of current task.
    #[linkage = "weak"]
    #[export_name = "hal_thread_set_tid"]
    pub fn set_tid(_tid: u64, _pid: u64) {
        unimplemented!()
    }

    /// Get tid and pid of current task.
    #[linkage = "weak"]
    #[export_name = "hal_thread_get_tid"]
    pub fn get_tid() -> (u64, u64) {
        unimplemented!()
    }
}

#[linkage = "weak"]
#[export_name = "hal_context_run"]
pub fn context_run(_context: &mut UserContext) {
    unimplemented!()
}

pub trait PageTableTrait: Sync + Send {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    fn map(&mut self, _vaddr: VirtAddr, _paddr: PhysAddr, _flags: MMUFlags) -> Result<(), ()>;

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, _vaddr: VirtAddr) -> Result<(), ()>;

    /// Change the `flags` of the page of `vaddr`.
    fn protect(&mut self, _vaddr: VirtAddr, _flags: MMUFlags) -> Result<(), ()>;

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&mut self, _vaddr: VirtAddr) -> Result<PhysAddr, ()>;

    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr;

    fn map_many(
        &mut self,
        mut vaddr: VirtAddr,
        paddrs: &[PhysAddr],
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for &paddr in paddrs {
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    fn map_cont(
        &mut self,
        mut vaddr: VirtAddr,
        paddr: PhysAddr,
        pages: usize,
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for i in 0..pages {
            let paddr = paddr + i * PAGE_SIZE;
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), ()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    table_phys: PhysAddr,
}

impl PageTable {
    /// Get current page table
    #[linkage = "weak"]
    #[export_name = "hal_pt_current"]
    pub fn current() -> Self {
        unimplemented!()
    }

    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[linkage = "weak"]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        unimplemented!()
    }
}

impl PageTableTrait for PageTable {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_map"]
    fn map(&mut self, _vaddr: VirtAddr, _paddr: PhysAddr, _flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Unmap the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, _vaddr: VirtAddr) -> Result<(), ()> {
        unimplemented!()
    }
    /// Change the `flags` of the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, _vaddr: VirtAddr, _flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Query the physical address which the page of `vaddr` maps to.
    #[linkage = "weak"]
    #[export_name = "hal_pt_query"]
    fn query(&mut self, _vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        unimplemented!()
    }
    /// Get the physical address of root page table.
    #[linkage = "weak"]
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        self.table_phys
    }
    #[linkage = "weak"]
    #[export_name = "hal_pt_unmap_cont"]
    fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), ()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_alloc"]
    pub extern "C" fn alloc() -> Option<Self> {
        unimplemented!()
    }

    #[linkage = "weak"]
    #[export_name = "hal_frame_alloc_contiguous"]
    pub extern "C" fn alloc_contiguous_base(_size: usize, _align_log2: usize) -> Option<PhysAddr> {
        unimplemented!()
    }

    pub fn alloc_contiguous(size: usize, align_log2: usize) -> Vec<Self> {
        PhysFrame::alloc_contiguous_base(size, align_log2).map_or(Vec::new(), |base| {
            (0..size)
                .map(|i| PhysFrame {
                    paddr: base + i * PAGE_SIZE,
                })
                .collect()
        })
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }

    #[linkage = "weak"]
    #[export_name = "hal_zero_frame_paddr"]
    pub fn zero_frame_addr() -> PhysAddr {
        unimplemented!()
    }
}

impl Drop for PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        unimplemented!()
    }
}

/// Read physical memory from `paddr` to `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_read"]
pub fn pmem_read(_paddr: PhysAddr, _buf: &mut [u8]) {
    unimplemented!()
}

/// Write physical memory to `paddr` from `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_write"]
pub fn pmem_write(_paddr: PhysAddr, _buf: &[u8]) {
    unimplemented!()
}

/// Copy content of `src` frame to `target` frame.
#[linkage = "weak"]
#[export_name = "hal_frame_copy"]
pub fn frame_copy(_src: PhysAddr, _target: PhysAddr) {
    unimplemented!()
}

/// Zero `target` frame.
#[linkage = "weak"]
#[export_name = "hal_frame_zero"]
pub fn frame_zero_in_range(_target: PhysAddr, _start: usize, _end: usize) {
    unimplemented!()
}

/// Flush the physical frame.
#[linkage = "weak"]
#[export_name = "hal_frame_flush"]
pub fn frame_flush(_target: PhysAddr) {
    unimplemented!()
}

/// Register a callback of serial readable event.
#[linkage = "weak"]
#[export_name = "hal_serial_set_callback"]
pub fn serial_set_callback(_callback: Box<dyn FnOnce() + Send + Sync>) {
    unimplemented!()
}

/// Read a string from console.
#[linkage = "weak"]
#[export_name = "hal_serial_read"]
pub fn serial_read(_buf: &mut [u8]) -> usize {
    unimplemented!()
}

/// Output a string to console.
#[linkage = "weak"]
#[export_name = "hal_serial_write"]
pub fn serial_write(_s: &str) {
    unimplemented!()
}

/// Get current time.
#[linkage = "weak"]
#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    unimplemented!()
}

/// Set a new timer. After `deadline`, the `callback` will be called.
#[linkage = "weak"]
#[export_name = "hal_timer_set"]
pub fn timer_set(_deadline: Duration, _callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    unimplemented!()
}

/// Check timers, call when timer interrupt happened.
#[linkage = "weak"]
#[export_name = "hal_timer_tick"]
pub fn timer_tick() {
    unimplemented!()
}

pub struct InterruptManager {}
impl InterruptManager {
    /// Handle IRQ.
    #[linkage = "weak"]
    #[export_name = "hal_irq_handle"]
    pub fn handle(_irq: u8) {
        unimplemented!()
    }
    ///
    #[linkage = "weak"]
    #[export_name = "hal_ioapic_set_handle"]
    pub fn set_ioapic_handle(_global_irq: u32, _handle: Box<dyn Fn() + Send + Sync>) -> Option<u8> {
        unimplemented!()
    }
    /// Add an interrupt handle to an irq
    #[linkage = "weak"]
    #[export_name = "hal_irq_add_handle"]
    pub fn add_handle(_global_irq: u8, _handle: Box<dyn Fn() + Send + Sync>) -> Option<u8> {
        unimplemented!()
    }
    ///
    #[linkage = "weak"]
    #[export_name = "hal_ioapic_reset_handle"]
    pub fn reset_ioapic_handle(_global_irq: u32) -> bool {
        unimplemented!()
    }
    /// Remove the interrupt handle of an irq
    #[linkage = "weak"]
    #[export_name = "hal_irq_remove_handle"]
    pub fn remove_handle(_irq: u8) -> bool {
        unimplemented!()
    }
    /// Allocate contiguous positions for irq
    #[linkage = "weak"]
    #[export_name = "hal_irq_allocate_block"]
    pub fn allocate_block(_irq_num: u32) -> Option<(usize, usize)> {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_irq_free_block"]
    pub fn free_block(_irq_start: u32, _irq_num: u32) {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_irq_overwrite_handler"]
    pub fn overwrite_handler(_msi_id: u32, _handle: Box<dyn Fn() + Send + Sync>) -> bool {
        unimplemented!()
    }

    /// Enable IRQ.
    #[linkage = "weak"]
    #[export_name = "hal_irq_enable"]
    pub fn enable(_global_irq: u32) {
        unimplemented!()
    }

    /// Disable IRQ.
    #[linkage = "weak"]
    #[export_name = "hal_irq_disable"]
    pub fn disable(_global_irq: u32) {
        unimplemented!()
    }
    /// Get IO APIC maxinstr
    #[linkage = "weak"]
    #[export_name = "hal_irq_maxinstr"]
    pub fn maxinstr(_irq: u32) -> Option<u8> {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_irq_configure"]
    pub fn configure(
        _irq: u32,
        _vector: u8,
        _dest: u8,
        _level_trig: bool,
        _active_high: bool,
    ) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_irq_isvalid"]
    pub fn is_valid(_irq: u32) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_wait_for_interrupt"]
    pub fn wait_for_interrupt() {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_page_fault"]
    pub fn is_page_fault(_trap: usize) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_is_syscall"]
    pub fn is_syscall(_trap: usize) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_is_intr"]
    pub fn is_intr(_trap: usize) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_is_timer_intr"]
    pub fn is_timer_intr(_trap: usize) -> bool {
        unimplemented!()
    }
    #[linkage = "weak"]
    #[export_name = "hal_is_reserved_inst"]
    pub fn is_reserved_inst(_trap: usize) -> bool {
        unimplemented!()
    }
}

/// Get platform specific information.
#[linkage = "weak"]
#[export_name = "hal_vdso_constants"]
pub fn vdso_constants() -> VdsoConstants {
    unimplemented!()
}

/// Get fault address of the last page fault.
#[linkage = "weak"]
#[export_name = "fetch_fault_vaddr"]
pub fn fetch_fault_vaddr() -> VirtAddr {
    unimplemented!()
}

/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
#[linkage = "weak"]
#[export_name = "hal_pc_firmware_tables"]
pub fn pc_firmware_tables() -> (u64, u64) {
    unimplemented!()
}

/// Get ACPI Table
#[linkage = "weak"]
#[export_name = "hal_acpi_table"]
pub fn get_acpi_table() -> Option<Acpi> {
    unimplemented!()
}

/// IO Ports access on x86 platform
#[linkage = "weak"]
#[export_name = "hal_outpd"]
pub fn outpd(_port: u16, _value: u32) {
    unimplemented!()
}

#[linkage = "weak"]
#[export_name = "hal_inpd"]
pub fn inpd(_port: u16) -> u32 {
    unimplemented!()
}

/// Get local APIC ID
#[linkage = "weak"]
#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    unimplemented!()
}

/// Fill random bytes to the buffer
#[cfg(target_arch = "x86_64")]
pub fn fill_random(buf: &mut [u8]) {
    // TODO: optimize
    for x in buf.iter_mut() {
        let mut r = 0;
        unsafe {
            core::arch::x86_64::_rdrand16_step(&mut r);
        }
        *x = r as _;
    }
}

#[cfg(target_arch = "aarch64")]
pub fn fill_random(_buf: &mut [u8]) {
    // TODO
}

use crate::types::*;
use crate::error::*;
use crate::pmem::{PMEM, Page};
use crate::vm;
pub use super::*;
pub use super::defs::*;
use core::fmt::Debug;
use core::time::Duration;
use super::vdso::{Features, VdsoConstants};
use git_version::git_version;
use crate::kt;
use crate::control;
use alloc::boxed::Box;
use trapframe::UserContext;
use crate::thread::LocalContext;
use crate::user::*;
use alloc::sync::Arc;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::future::Future;
use alloc::vec::Vec;
use lazy_static::lazy_static;

const PHYS_IDMAP_BASE: usize = 0x6000_0000_0000;
pub static IDMAP: IdMap = unsafe { IdMap::assume_idmap(PHYS_IDMAP_BASE) };

#[repr(C)]
pub struct Thread {
    thread: usize,
}

impl Thread {
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(
        future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        vmtoken: usize,
    ) -> Self {
        struct UserProcessSwitchWrapper {
            inner: Pin<Box<dyn Future<Output = ()> + Send>>,
            vmtoken: usize,
        }
        impl Future for UserProcessSwitchWrapper {
            type Output = ();
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                // We don't actually own the remote `Arc`. `PageTable` owns it.
                let remote_arc = unsafe {
                    Arc::from_raw(self.vmtoken as *const UserProcess)
                };
                let local_arc = remote_arc.clone();
                Arc::into_raw(remote_arc);

                *LocalContext::current().user_process.borrow_mut() = Some(local_arc);
                println!("UPSW poll");
                self.as_mut().inner.as_mut().poll(cx)
            }
        }

        println!("thread spawn, vmtoken = {:x}", vmtoken);

        crate::executor::spawn(UserProcessSwitchWrapper {
            inner: future,
            vmtoken,
        });
        Thread { thread: 0 }
    }

    #[export_name = "hal_thread_set_tid"]
    pub fn set_tid(_tid: u64, _pid: u64) {
    }

    #[export_name = "hal_thread_get_tid"]
    pub fn get_tid() -> (u64, u64) {
        (0, 0)
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: usize,
}

impl Debug for PhysFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        write!(f, "PhysFrame({:#x})", self.paddr)
    }
}

impl PhysFrame {
    #[export_name = "hal_frame_alloc"]
    pub extern "C" fn alloc() -> Option<Self> {
        let page = match Page::new() {
            Ok(x) => x,
            Err(e) => {
                println!("hal_frame_alloc: allocation failed");
                return None;
            }
        };
        let paddr = page.region().paddr;
        match vm::K.lock().insert_page(phys_to_kvirt(paddr), page) {
            Ok(_) => {}
            Err(e) => {
                println!("hal_frame_alloc: insert failed");
                return None;
            }
        }
        Some(PhysFrame {
            paddr,
        })
    }

    #[export_name = "hal_zero_frame_paddr"]
    pub fn zero_frame_addr() -> PhysAddr {
        0
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }

    pub fn alloc_contiguous(_size: usize, _align_log2: usize) -> Vec<Self> {
        unimplemented!("alloc_contiguous");
    }
}

impl Drop for PhysFrame {
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        assert_eq!(vm::K.lock().remove_page(phys_to_kvirt(self.paddr)), true);
    }
}

fn phys_to_kvirt(phys: PhysAddr) -> VirtAddr {
    PHYS_IDMAP_BASE + phys
}

/// Read physical memory from `paddr` to `buf`.
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    unsafe {
        (phys_to_kvirt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_kvirt(paddr) as _, buf.len());
    }
}

/// Copy content of `src` frame to `target` frame
#[export_name = "hal_frame_copy"]
pub fn frame_copy(src: PhysAddr, target: PhysAddr) {
    unsafe {
        (phys_to_kvirt(src) as *const u8).copy_to_nonoverlapping(phys_to_kvirt(target) as *mut u8, PAGE_SIZE);
    }
}

/// Zero `target` frame.
#[export_name = "hal_frame_zero"]
pub fn frame_zero(target: PhysAddr) {
    unsafe {
        core::ptr::write_bytes(phys_to_kvirt(target) as *mut u8, 0, PAGE_SIZE);
    }
}

/// Zero `target` frame.
pub fn frame_zero_in_range(target: PhysAddr, start: usize, end: usize) {
    unsafe {
        core::ptr::write_bytes(phys_to_kvirt(target + start) as *mut u8, 0, end - start);
    }
}

/// Flush the physical frame.
#[export_name = "hal_frame_flush"]
pub fn frame_flush(_target: PhysAddr) {
    // do nothing
}

/// Get current time.
#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    Duration::from_nanos(crate::timer::now())
}

#[export_name = "hal_context_run"]
pub fn context_run(context: &mut UserContext) {
    println!("context run");
    let user_process = LocalContext::current().user_process.borrow();
    let user_process = user_process
        .as_ref().expect("context_run: no user process");
    let user_thread = user_process.get_thread().expect("context_run: cannot get user thread");
    let (entry_reason, user_thread) = user_thread.run(context);
    user_process.put_thread(user_thread);

    match entry_reason {
        KernelEntryReason::UnknownSyscall => {
            // TODO: usermode vdso injection for register swapping
            context.general.rdx = context.general.rax;
            context.trap_num = 0x100; // emulate x86-64 hardware trap number
        }
        _ => {
            // TODO: handle this
            context.trap_num = 0x0;
        }
    }
}

/// Set a new timer.
///
/// After `deadline`, the `callback` will be called.
#[export_name = "hal_timer_set"]
pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    kt::spawn(move || {
        let now = timer_now();
        if deadline > now {
            control::sleep((deadline - now).as_nanos() as u64);
        }
        callback(timer_now());
    }).expect("timer_set: cannot spawn thread");
}

#[export_name = "hal_vdso_constants"]
pub fn vdso_constants() -> VdsoConstants {
    let tsc_frequency = 3000u16;
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
        physmem: PMEM.size() as u64,
        version_string_len: 0,
        version_string: Default::default(),
    };
    constants.set_version_string(git_version!(
        prefix = "git-",
        args = ["--always", "--abbrev=40", "--dirty=-dirty"]
    ));
    constants
}

#[repr(C)]
pub struct PageTable {
    user_process: Arc<UserProcess>,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    #[inline(never)]
    pub fn new() -> Self {
        PageTable {
            user_process: UserProcess::new().expect("PageTable::new: cannot create user process"),
        }
    }

    pub fn current() -> Self {
        PageTable {
            user_process: LocalContext::current().user_process.borrow()
                .as_ref().expect("PageTable::current: no current process")
                .clone()
        }
    }
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

impl PageTableTrait for PageTable {
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
        let page = vm::K.lock().page_at(phys_to_kvirt(paddr)).expect("PageTable::map: bad physical address")
            .try_clone().expect("PageTable::map: cannot clone page reference");
        println!("pt map {:x} {:x}", vaddr, paddr);
        self.user_process.vm.lock().insert_page(vaddr, page)
            .map_err(|e| {
                println!("PageTable::map: insert failed: {:?}", e);
                ()
            })
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        match self.user_process.vm.lock().remove_page(vaddr) {
            true => Ok(()),
            false => Err(()),
        }
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        // TODO: implement this
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        match self.user_process.vm.lock().page_at(vaddr) {
            Some(x) => Ok(x.region().paddr),
            None => Err(()),
        }
    }

    /// Get the physical address of root page table.
    /// FIXME: The returned value is used as `vmtoken`. Rename this function!
    #[export_name = "hal_pt_table_phys"]
    fn table_phys(&self) -> PhysAddr {
        Arc::as_ptr(&self.user_process) as usize
    }
}

#[export_name = "hal_serial_set_callback"]
pub fn serial_set_callback(callback: Box<dyn FnOnce() + Send + Sync>) {

}

#[export_name = "hal_serial_read"]
pub fn serial_read(buf: &mut [u8]) -> usize {
    unimplemented!()
}

/// Output a char to console.
#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    print!("{}", s);
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

/// Get fault address of the last page fault.
#[linkage = "weak"]
#[export_name = "fetch_fault_vaddr"]
pub fn fetch_fault_vaddr() -> VirtAddr {
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
/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
#[linkage = "weak"]
#[export_name = "hal_pc_firmware_tables"]
pub fn pc_firmware_tables() -> (u64, u64) {
    unimplemented!()
}

pub fn kcounters_page() -> PhysAddr {
    lazy_static! {
        static ref PAGE: PhysAddr = {
            let phys_frame = PhysFrame::alloc().expect("cannot create kcounters page");
            let addr = phys_frame.paddr;
            core::mem::forget(phys_frame);
            addr
        };
    }
    *PAGE
}
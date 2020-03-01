#![feature(asm, global_asm)]
#![feature(linkage)]
#![deny(warnings)]

#[macro_use]
extern crate log;

extern crate alloc;

use {
    alloc::collections::VecDeque,
    alloc::sync::Arc,
    core::{future::Future, pin::Pin},
    lazy_static::lazy_static,
    std::fmt::{Debug, Formatter},
    std::fs::{File, OpenOptions},
    std::io::Error,
    std::os::unix::io::AsRawFd,
    std::sync::Mutex,
    std::time::{Duration, SystemTime},
    tempfile::tempdir,
};

pub use self::trap::syscall_entry;
pub use kernel_hal::defs::*;

#[cfg(target_os = "linux")]
include!("fsbase_linux.rs");
#[cfg(target_os = "macos")]
include!("fsbase_macos.rs");

mod trap;

#[repr(C)]
pub struct Thread {
    thread: usize,
}

impl Thread {
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(thread: Arc<usize>, mut regs: GeneralRegs) -> Self {
        async_std::task::spawn(async move {
            thread_set_state(&thread, &regs);
            let mut fxstate = [0u128; 512 / 16];
            loop {
                // 判断线程状态是否是RUNNABLE,不是则返回Pending
                unsafe {
                    thread_check_runnable(&thread).await;
                }
                unsafe {
                    core::arch::x86_64::_fxrstor64(fxstate.as_ptr() as _);
                    trap::run_user(&mut regs);
                    core::arch::x86_64::_fxsave64(fxstate.as_mut_ptr() as _);
                }
                let exit = unsafe { handle_syscall(&thread, &mut regs).await };
                if exit {
                    break;
                }
                async_std::task::yield_now().await;
            }
        });
        Thread { thread: 0 }
    }
}

#[linkage = "weak"]
#[export_name = "thread_set_state"]
pub fn thread_set_state(_thread: &Arc<usize>, _state: &GeneralRegs) {
    unimplemented!()
}

/// Check whether a thread is runnable
#[linkage = "weak"]
#[no_mangle]
extern "C" fn thread_check_runnable(
    _thread: &Arc<usize>,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async {})
}

#[linkage = "weak"]
#[no_mangle]
extern "C" fn handle_syscall(
    _thread: &Arc<usize>,
    _regs: &mut GeneralRegs,
) -> Pin<Box<dyn Future<Output = bool> + Send>> {
    // exit by default
    Box::pin(async { true })
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    table_phys: PhysAddr,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        PageTable { table_phys: 0 }
    }

    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    pub fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        debug_assert!(page_aligned(paddr));
        let prot = flags.to_mmap_prot();
        mmap(FRAME_FILE.as_raw_fd(), paddr, PAGE_SIZE, vaddr, prot);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        self.unmap_cont(vaddr, 1)
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        let prot = flags.to_mmap_prot();
        let ret = unsafe { libc::mprotect(vaddr as _, PAGE_SIZE, prot) };
        assert_eq!(ret, 0, "failed to mprotect: {:?}", Error::last_os_error());
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        debug_assert!(page_aligned(vaddr));
        unimplemented!()
    }

    #[export_name = "hal_pt_unmap_cont"]
    pub fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), ()> {
        if pages == 0 {
            return Ok(());
        }
        debug_assert!(page_aligned(vaddr));
        let ret = unsafe { libc::munmap(vaddr as _, PAGE_SIZE * pages) };
        assert_eq!(ret, 0, "failed to munmap: {:?}", Error::last_os_error());
        Ok(())
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl Debug for PhysFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "PhysFrame({:#x})", self.paddr)
    }
}

lazy_static! {
    static ref AVAILABLE_FRAMES: Mutex<VecDeque<usize>> =
        Mutex::new({ (0..PMEM_SIZE).step_by(PAGE_SIZE).collect() });
}

impl PhysFrame {
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        let ret = AVAILABLE_FRAMES
            .lock()
            .unwrap()
            .pop_front()
            .map(|paddr| PhysFrame { paddr });
        trace!("frame alloc: {:?}", ret);
        ret
    }
}

impl Drop for PhysFrame {
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        trace!("frame dealloc: {:?}", self);
        AVAILABLE_FRAMES.lock().unwrap().push_back(self.paddr);
    }
}

fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    /// Map physical memory from here.
    const PMEM_BASE: VirtAddr = 0x8_00000000;

    PMEM_BASE + paddr
}

/// Ensure physical memory are mmapped and accessible.
fn ensure_mmap_pmem() {
    FRAME_FILE.as_raw_fd();
}

/// Read physical memory from `paddr` to `buf`.
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    trace!("pmem read: paddr={:#x}, len={:#x}", paddr, buf.len());
    ensure_mmap_pmem();
    assert!(paddr < PMEM_SIZE);
    unsafe {
        (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
    ensure_mmap_pmem();
    assert!(paddr < PMEM_SIZE);
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
    }
}

const PAGE_SIZE: usize = 0x1000;

fn page_aligned(x: VirtAddr) -> bool {
    x % PAGE_SIZE == 0
}

const PMEM_SIZE: usize = 0x100_00000; // 256MiB

lazy_static! {
    static ref FRAME_FILE: File = create_pmem_file();
}

fn create_pmem_file() -> File {
    let dir = tempdir().expect("failed to create pmem dir");
    let path = dir.path().join("pmem");

    // workaround on macOS to avoid permission denied.
    // see https://jiege.ch/software/2020/02/07/macos-mmap-exec/ for analysis on this problem.
    #[cfg(target_os = "macos")]
    std::mem::forget(dir);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .expect("failed to create pmem file");
    file.set_len(PMEM_SIZE as u64)
        .expect("failed to resize file");
    trace!("create pmem file: path={:?}, size={:#x}", path, PMEM_SIZE);
    let prot = libc::PROT_READ | libc::PROT_WRITE;
    mmap(file.as_raw_fd(), 0, PMEM_SIZE, phys_to_virt(0), prot);
    file
}

/// Mmap frame file `fd` to `vaddr`.
fn mmap(fd: libc::c_int, offset: usize, len: usize, vaddr: VirtAddr, prot: libc::c_int) {
    // workaround on macOS to write text section.
    #[cfg(target_os = "macos")]
    let prot = if prot & libc::PROT_EXEC != 0 {
        prot | libc::PROT_WRITE
    } else {
        prot
    };

    let ret = unsafe {
        let flags = libc::MAP_SHARED | libc::MAP_FIXED;
        libc::mmap(vaddr as _, len, prot, flags, fd, offset as _)
    } as usize;
    trace!(
        "mmap file: fd={}, offset={:#x}, len={:#x}, vaddr={:#x}, prot={:#b}",
        fd,
        offset,
        len,
        vaddr,
        prot,
    );
    assert_eq!(ret, vaddr, "failed to mmap: {:?}", Error::last_os_error());
}

trait FlagsExt {
    fn to_mmap_prot(self) -> libc::c_int;
}

impl FlagsExt for MMUFlags {
    fn to_mmap_prot(self) -> libc::c_int {
        let mut flags = 0;
        if self.contains(MMUFlags::READ) {
            flags |= libc::PROT_READ;
        }
        if self.contains(MMUFlags::WRITE) {
            flags |= libc::PROT_WRITE;
        }
        if self.contains(MMUFlags::EXECUTE) {
            flags |= libc::PROT_EXEC;
        }
        flags
    }
}

/// Output a char to console.
#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    print!("{}", s);
}

/// Get current time.
#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
}

/// Set a new timer.
///
/// After `deadline`, the `callback` will be called.
#[export_name = "hal_timer_set"]
pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline - timer_now());
        callback(timer_now());
    });
}

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init() {
    #[cfg(target_os = "macos")]
    unsafe {
        register_sigsegv_handler();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid virtual address base to mmap.
    const VBASE: VirtAddr = 0x2_00000000;

    #[test]
    fn map_unmap() {
        let mut pt = PageTable::new();
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        // map 2 pages to 1 frame
        pt.map(VBASE, 0x1000, flags).unwrap();
        pt.map(VBASE + 0x1000, 0x1000, flags).unwrap();

        unsafe {
            const MAGIC: usize = 0xdead_beaf;
            (VBASE as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x1000) as *mut usize).read(), MAGIC);
        }

        pt.unmap(VBASE + 0x1000).unwrap();
    }
}

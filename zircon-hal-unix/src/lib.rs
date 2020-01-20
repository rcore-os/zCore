#![feature(asm)]
#![deny(warnings)]

#[macro_use]
extern crate log;

extern crate alloc;

use {
    alloc::sync::Arc,
    lazy_static::lazy_static,
    std::cell::RefCell,
    std::fmt::{Debug, Formatter},
    std::fs::{File, OpenOptions},
    std::io::Error,
    std::os::unix::io::AsRawFd,
    std::sync::atomic::{AtomicUsize, Ordering},
    std::time::{Duration, SystemTime},
    tempfile::tempdir_in,
};

type PhysAddr = usize;
type VirtAddr = usize;
type MMUFlags = usize;

#[repr(C)]
pub struct Thread {
    thread: std::thread::Thread,
}

impl Thread {
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(entry: usize, stack: usize, arg1: usize, arg2: usize, tls: Arc<usize>) -> Self {
        // align stack pointer to 16 bytes
        let stack = stack & !0xf;
        let handle = std::thread::spawn(move || {
            TLS.with(|t| t.replace(Some(tls)));
            unsafe {
                asm!("call $0" :: "r"(entry), "{rsp}"(stack), "{rdi}"(arg1), "{rsi}"(arg2) :: "volatile" "intel");
            }
            unreachable!()
        });
        Thread {
            thread: handle.thread().clone(),
        }
    }

    #[export_name = "hal_thread_exit"]
    pub fn exit(&mut self) {
        unimplemented!()
    }

    #[export_name = "hal_thread_tls"]
    pub fn tls() -> Arc<usize> {
        TLS.with(|t| t.borrow().as_ref().unwrap().clone())
    }

    #[export_name = "hal_thread_park"]
    pub fn park() {
        std::thread::park();
    }

    #[export_name = "hal_thread_get_waker"]
    pub fn get_waker() -> Waker {
        Waker {
            thread: std::thread::current(),
        }
    }
}

#[repr(C)]
pub struct Waker {
    thread: std::thread::Thread,
}

impl Waker {
    #[export_name = "hal_thread_wake"]
    pub fn wake(&self) {
        self.thread.unpark();
    }
}

thread_local! {
    static TLS: RefCell<Option<Arc<usize>>> = RefCell::new(None);
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
    pub fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, _flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        debug_assert!(page_aligned(paddr));
        mmap(FRAME_FILE.as_raw_fd(), paddr, PAGE_SIZE, vaddr);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        let ret = unsafe { libc::munmap(vaddr as _, PAGE_SIZE) };
        assert_eq!(ret, 0, "failed to munmap: {:?}", Error::last_os_error());
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        let ret = unsafe { libc::mprotect(vaddr as _, PAGE_SIZE, flags as libc::c_int) };
        assert_eq!(ret, 0, "failed to mprotect: {:?}", Error::last_os_error());
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, vaddr: VirtAddr) -> Result<(PhysAddr, MMUFlags), ()> {
        debug_assert!(page_aligned(vaddr));
        unimplemented!()
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

impl PhysFrame {
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        let frame_id = GLOBAL_FRAME_ID.fetch_add(1, Ordering::SeqCst);
        let ret = Some(PhysFrame {
            paddr: frame_id * PAGE_SIZE,
        });
        trace!("frame alloc: {:?}", ret);
        ret
    }
}

impl Drop for PhysFrame {
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        trace!("frame dealloc: {:?}", self);
        // we don't deallocate frames
    }
}

/// Next allocated frame ID.
static GLOBAL_FRAME_ID: AtomicUsize = AtomicUsize::new(1);

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
    unsafe {
        (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    trace!("pmem write: paddr={:#x}, len={:#x}", paddr, buf.len());
    ensure_mmap_pmem();
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
    }
}

const PAGE_SIZE: usize = 0x1000;

fn page_aligned(x: VirtAddr) -> bool {
    x % PAGE_SIZE == 0
}

const PMEM_SIZE: usize = 0x10_00000; // 16MiB

lazy_static! {
    static ref FRAME_FILE: File = create_pmem_file();
}

fn create_pmem_file() -> File {
    let dir = tempdir_in("/tmp").expect("failed to create pmem dir");
    let path = dir.path().join("pmem");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .expect("failed to create pmem file");
    file.set_len(PMEM_SIZE as u64)
        .expect("failed to resize file");
    trace!("create pmem file: path={:?}, size={:#x}", path, PMEM_SIZE);
    mmap(file.as_raw_fd(), 0, PMEM_SIZE, phys_to_virt(0));
    file
}

/// Mmap frame file `fd` to `vaddr`.
fn mmap(fd: libc::c_int, offset: usize, len: usize, vaddr: VirtAddr) {
    let ret = unsafe {
        let prot = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;
        let flags = libc::MAP_SHARED | libc::MAP_FIXED;
        libc::mmap(vaddr as _, len, prot, flags, fd, offset as _)
    } as usize;
    trace!(
        "mmap file (fd={}, offset={:#x}, len={:#x}) to {:#x}",
        fd,
        offset,
        len,
        vaddr
    );
    assert_eq!(ret, vaddr, "failed to mmap: {:?}", Error::last_os_error());
}

#[export_name = "hal_serial_write"]
pub fn serial_write(c: char) {
    print!("{}", c);
}

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
}

#[export_name = "hal_timer_set"]
pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline - timer_now());
        callback(timer_now());
    });
}

/// A dummy function.
///
/// Call this anywhere to ensure this lib being linked.
pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid virtual address base to mmap.
    const VBASE: VirtAddr = 0x2_00000000;

    #[test]
    fn map_unmap() {
        let mut pt = PageTable::new();
        // map 2 pages to 1 frame
        pt.map(VBASE, 0x1000, 0).unwrap();
        pt.map(VBASE + 0x1000, 0x1000, 0).unwrap();

        unsafe {
            const MAGIC: usize = 0xdead_beaf;
            (VBASE as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x1000) as *mut usize).read(), MAGIC);
        }

        pt.unmap(VBASE + 0x1000).unwrap();
    }
}

#![feature(asm)]

#[macro_use]
extern crate log;

use lazy_static::lazy_static;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tempfile::{tempdir_in, TempDir};

type ThreadId = usize;
type PhysAddr = usize;
type VirtAddr = usize;
type MMUFlags = usize;
type APIResult = usize;

#[repr(C)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(entry: usize, stack: usize, arg1: usize, arg2: usize) -> Self {
        let handle = std::thread::spawn(move || {
            unsafe {
                asm!("jmp $0" :: "r"(entry), "{rsp}"(stack), "{rdi}"(arg1), "{rsi}"(arg2) :: "volatile" "intel");
            }
            unreachable!()
        });
        let id = 0;
        Thread { id }
    }

    #[export_name = "hal_thread_exit"]
    pub fn exit(&mut self) {
        unimplemented!()
    }
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    table_phys: PhysAddr,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        PageTable { table_phys: 0 }
    }

    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[export_name = "hal_pt_map"]
    fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        debug_assert!(page_aligned(paddr));
        let fd = open_frame_file(paddr);
        mmap(fd, vaddr);
        Ok(())
    }

    /// Unmap the page of `vaddr`.
    #[export_name = "hal_pt_unmap"]
    fn unmap(&mut self, vaddr: VirtAddr) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        let ret = unsafe { libc::munmap(vaddr as _, PAGE_SIZE) };
        assert_eq!(ret, 0, "failed to munmap");
        Ok(())
    }

    /// Change the `flags` of the page of `vaddr`.
    #[export_name = "hal_pt_protect"]
    fn protect(&mut self, vaddr: VirtAddr, flags: MMUFlags) -> Result<(), ()> {
        debug_assert!(page_aligned(vaddr));
        let ret = unsafe { libc::mprotect(vaddr as _, PAGE_SIZE, flags as libc::c_int) };
        assert_eq!(ret, 0, "failed to mprotect");
        Ok(())
    }

    /// Query the physical address which the page of `vaddr` maps to.
    #[export_name = "hal_pt_query"]
    fn query(&mut self, vaddr: VirtAddr) -> Result<(PhysAddr, MMUFlags), ()> {
        debug_assert!(page_aligned(vaddr));
        unimplemented!()
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        let frame_id = GLOBAL_FRAME_ID.fetch_add(1, Ordering::SeqCst);
        Some(PhysFrame {
            paddr: frame_id * PAGE_SIZE,
        })
    }
}

impl Drop for PhysFrame {
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        // we don't deallocate frames
    }
}

/// Next allocated frame ID.
static GLOBAL_FRAME_ID: AtomicUsize = AtomicUsize::new(1);

fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    /// Map physical memory from here.
    const PMEM_BASE: VirtAddr = 0x800000000;

    PMEM_BASE + paddr
}

/// Ensure physical memory in `[paddr, paddr + len)` are mmapped and accessible.
fn ensure_mmap_pmem(paddr: PhysAddr, len: usize) {
    let begin = align_to_page(paddr);
    let end = align_to_page(paddr + len) + PAGE_SIZE;
    for paddr in (begin..end).step_by(PAGE_SIZE) {
        open_frame_file(paddr);
    }
}

/// Read physical memory from `paddr` to `buf`.
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    ensure_mmap_pmem(paddr, buf.len());
    unsafe {
        (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    ensure_mmap_pmem(paddr, buf.len());
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
    }
}

const PAGE_SIZE: usize = 0x1000;

fn page_aligned(x: VirtAddr) -> bool {
    x % PAGE_SIZE == 0
}

fn align_to_page(x: VirtAddr) -> VirtAddr {
    x / PAGE_SIZE * PAGE_SIZE
}

fn open_frame_file(paddr: PhysAddr) -> i32 {
    lazy_static! {
        static ref PHYS_MEM_PATH: TempDir =
            tempdir_in("/tmp").expect("failed to create physical memory dir");
        static ref FILES: Mutex<BTreeMap<PhysAddr, File>> = Mutex::default();
    }
    let mut files = FILES.lock().unwrap();
    if !files.contains_key(&paddr) {
        let file_path = PHYS_MEM_PATH.path().join(format!("{:#x}", paddr));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(file_path)
            .expect("failed to open file");
        file.write(&[0u8; PAGE_SIZE]).unwrap();
        mmap(file.as_raw_fd(), phys_to_virt(paddr));
        files.insert(paddr, file);
        trace!("create frame file: {:#x}", paddr);
    }
    files.get(&paddr).unwrap().as_raw_fd()
}

/// Mmap frame file `fd` to `vaddr`.
fn mmap(fd: libc::c_int, vaddr: VirtAddr) {
    let ptr = unsafe {
        let prot = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;
        let flags = libc::MAP_SHARED | libc::MAP_FIXED;
        libc::mmap(vaddr as _, PAGE_SIZE, prot, flags, fd, 0)
    };
    assert_eq!(ptr as usize, vaddr, "failed to mmap");
    trace!("mmap frame file (fd={}) to {:#x}", fd, vaddr);
}

/// A dummy function.
///
/// Call this anywhere to ensure this lib being linked.
pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid virtual address base to mmap.
    const VBASE: VirtAddr = 0x200000000;

    #[test]
    fn map_unmap() {
        let mut pt = PageTable::new();
        // map 2 pages to 1 frame
        pt.map(VBASE + 0, 0x1000, 0).unwrap();
        pt.map(VBASE + 0x1000, 0x1000, 0).unwrap();

        unsafe {
            const MAGIC: usize = 0xdeadbeaf;
            (VBASE as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x1000) as *mut usize).read(), MAGIC);
        }

        pt.unmap(VBASE + 0x1000).unwrap();
    }
}

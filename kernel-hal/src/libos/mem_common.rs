use alloc::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Error;
use std::os::unix::io::AsRawFd;
use std::sync::Mutex;

use crate::{PhysAddr, VirtAddr, PAGE_SIZE};

/// Map physical memory from here.
pub(super) const PMEM_SIZE: usize = 0x4000_0000; // 1GiB

lazy_static::lazy_static! {
    pub(super) static ref FRAME_FILE: File = create_pmem_file();
    pub(super) static ref AVAILABLE_FRAMES: Mutex<VecDeque<usize>> =
        Mutex::new((PAGE_SIZE..PMEM_SIZE).step_by(PAGE_SIZE).collect());
}

pub(super) fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    /// Map physical memory from here.
    const PMEM_BASE: VirtAddr = 0x8_0000_0000;

    PMEM_BASE + paddr
}

/// Ensure physical memory are mmapped and accessible.
pub(super) fn ensure_mmap_pmem() {
    FRAME_FILE.as_raw_fd();
}

pub(super) fn create_pmem_file() -> File {
    let dir = tempfile::tempdir().expect("failed to create pmem dir");
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
pub(super) fn mmap(fd: libc::c_int, offset: usize, len: usize, vaddr: VirtAddr, prot: libc::c_int) {
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

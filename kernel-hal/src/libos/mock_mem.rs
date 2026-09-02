use std::os::unix::io::RawFd;

use nix::fcntl::{self, OFlag};
use nix::sys::mman::{self, MapFlags, ProtFlags};
use nix::{sys::stat::Mode, unistd};

use super::mem::PMEM_MAP_VADDR;
use crate::{MMUFlags, PhysAddr, VirtAddr};

pub struct MockMemory {
    size: usize,
    fd: RawFd,
}

impl MockMemory {
    pub fn new(size: usize) -> Self {
        let dir = tempfile::tempdir().expect("failed to create pmem directory");
        let path = dir.path().join("zcore_libos_pmem");

        let fd = fcntl::open(
            &path,
            OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
            Mode::S_IRWXU,
        )
        .expect("faild to open");
        unistd::ftruncate(fd, size as _).expect("failed to set size of shared memory!");

        let mem = Self { size, fd };
        mem.mmap(PMEM_MAP_VADDR, size, 0, MMUFlags::READ | MMUFlags::WRITE);
        mem
    }

    /// Mmap `paddr` to `vaddr` in frame file.
    pub fn mmap(&self, vaddr: VirtAddr, len: usize, paddr: PhysAddr, prot: MMUFlags) {
        assert!(paddr < self.size);
        assert!(paddr + len <= self.size);

        // workaround on macOS to write text section.
        #[cfg(target_os = "macos")]
        let prot = if prot.contains(MMUFlags::EXECUTE) {
            prot | MMUFlags::WRITE
        } else {
            prot
        };

        let prot_noexec = ProtFlags::from(prot) - ProtFlags::PROT_EXEC;
        let flags = MapFlags::MAP_SHARED | MapFlags::MAP_FIXED;
        let fd = self.fd;
        let offset = paddr as _;
        trace!(
            "mmap file: fd={}, offset={:#x}, len={:#x}, vaddr={:#x}, prot={:?}",
            fd,
            offset,
            len,
            vaddr,
            prot,
        );

        unsafe { mman::mmap(vaddr as _, len, prot_noexec, flags, fd, offset) }.unwrap_or_else(
            |err| {
                panic!(
                    "failed to mmap: fd={}, offset={:#x}, len={:#x}, vaddr={:#x}, prot={:?}: {:?}",
                    fd, offset, len, vaddr, prot, err
                )
            },
        );
        if prot.contains(MMUFlags::EXECUTE) {
            self.mprotect(vaddr, len, prot);
        }
    }

    pub fn munmap(&self, vaddr: VirtAddr, len: usize) {
        unsafe { mman::munmap(vaddr as _, len) }
            .unwrap_or_else(|err| panic!("failed to munmap: vaddr={:#x}: {:?}", vaddr, err));
    }

    pub fn mprotect(&self, vaddr: VirtAddr, len: usize, prot: MMUFlags) {
        unsafe { mman::mprotect(vaddr as _, len, prot.into()) }.unwrap_or_else(|err| {
            panic!(
                "failed to mprotect: vaddr={:#x}, prot={:?}: {:?}",
                vaddr, prot, err
            )
        });
    }

    pub fn phys_to_virt(&self, paddr: PhysAddr) -> VirtAddr {
        assert!(paddr < self.size);
        PMEM_MAP_VADDR + paddr
    }

    pub fn as_ptr<T>(&self, paddr: PhysAddr) -> *const T {
        self.phys_to_virt(paddr) as _
    }

    pub fn as_mut_ptr<T>(&self, paddr: PhysAddr) -> *mut T {
        self.phys_to_virt(paddr) as _
    }
}

impl Drop for MockMemory {
    fn drop(&mut self) {
        trace!("Drop MockMemory: fd={:?}", self.fd);
        unistd::close(self.fd).expect("failed to close shared memory file!");
    }
}

impl From<MMUFlags> for ProtFlags {
    fn from(f: MMUFlags) -> Self {
        let mut flags = Self::empty();
        if f.contains(MMUFlags::READ) {
            flags |= ProtFlags::PROT_READ;
        }
        if f.contains(MMUFlags::WRITE) {
            flags |= ProtFlags::PROT_WRITE;
        }
        if f.contains(MMUFlags::EXECUTE) {
            flags |= ProtFlags::PROT_EXEC;
        }
        flags
    }
}

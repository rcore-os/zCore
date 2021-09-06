use std::io::Error;
use std::os::unix::io::AsRawFd;

use super::mem_common::{mmap, FRAME_FILE};
use crate::{addr::is_aligned, HalResult, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

fn unmap_cont(vaddr: VirtAddr, pages: usize) -> HalResult {
    if pages == 0 {
        return Ok(());
    }
    debug_assert!(is_aligned(vaddr));
    let ret = unsafe { libc::munmap(vaddr as _, PAGE_SIZE * pages) };
    assert_eq!(ret, 0, "failed to munmap: {:?}", Error::last_os_error());
    Ok(())
}

hal_fn_impl! {
    impl mod crate::defs::vm {
        fn map_page(_vmtoken: PhysAddr, vaddr: VirtAddr, paddr: PhysAddr, flags: MMUFlags) -> HalResult {
            debug_assert!(is_aligned(vaddr));
            debug_assert!(is_aligned(paddr));
            let prot = flags.to_mmap_prot();
            mmap(FRAME_FILE.as_raw_fd(), paddr, PAGE_SIZE, vaddr, prot);
            Ok(())
        }

        fn unmap_page(_vmtoken: PhysAddr, vaddr: VirtAddr) -> HalResult {
            println!("unmap_page {:x?}",vaddr);
            unmap_cont(vaddr, 1)
        }

        fn update_page(_vmtoken: PhysAddr, vaddr: VirtAddr, _paddr: Option<PhysAddr>, flags: Option<MMUFlags>) -> HalResult {
            debug_assert!(is_aligned(vaddr));
            if let Some(flags) = flags {
                let prot = flags.to_mmap_prot();
                let ret = unsafe { libc::mprotect(vaddr as _, PAGE_SIZE, prot) };
                assert_eq!(ret, 0, "failed to mprotect: {:?}", Error::last_os_error());
            }
            Ok(())
        }

        fn current_vmtoken() -> PhysAddr {
            0
        }
    }
}

/// Page Table
pub struct PageTable;

impl PageTable {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }

    pub fn new_and_map_kernel() -> Self {
        Self
    }
}

impl PageTableTrait for PageTable {
    fn table_phys(&self) -> PhysAddr {
        0
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> HalResult {
        unmap_cont(vaddr, pages)
    }
}

trait FlagsExt {
    fn to_mmap_prot(&self) -> libc::c_int;
}

impl FlagsExt for MMUFlags {
    fn to_mmap_prot(&self) -> libc::c_int {
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

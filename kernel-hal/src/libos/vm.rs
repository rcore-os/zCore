use std::io::Error;
use std::os::unix::io::AsRawFd;

use super::mem_common::{mmap, FRAME_FILE};
use crate::{addr::is_aligned, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE};

hal_fn_impl! {
    impl mod crate::defs::vm {
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

    pub fn from_current() -> Self {
        Self
    }
}

impl GenericPageTable for PageTable {
    fn table_phys(&self) -> PhysAddr {
        0
    }

    fn map(&mut self, page: Page, paddr: PhysAddr, flags: MMUFlags) -> PagingResult {
        debug_assert!(page.size as usize == PAGE_SIZE);
        debug_assert!(is_aligned(paddr));
        mmap(
            FRAME_FILE.as_raw_fd(),
            paddr,
            PAGE_SIZE,
            page.vaddr,
            flags.into(),
        );
        Ok(())
    }

    fn unmap(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)> {
        println!("unmap_page {:x?}", vaddr);
        self.unmap_cont(vaddr, PAGE_SIZE)?;
        Ok((0, PageSize::Size4K))
    }

    fn update(
        &mut self,
        vaddr: VirtAddr,
        _paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize> {
        debug_assert!(is_aligned(vaddr));
        if let Some(flags) = flags {
            let ret = unsafe { libc::mprotect(vaddr as _, PAGE_SIZE, flags.into()) };
            assert_eq!(ret, 0, "failed to mprotect: {:?}", Error::last_os_error());
        }
        Ok(PageSize::Size4K)
    }

    fn query(&self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, MMUFlags, PageSize)> {
        debug_assert!(is_aligned(vaddr));
        unimplemented!()
    }

    fn unmap_cont(&mut self, vaddr: VirtAddr, size: usize) -> PagingResult {
        if size == 0 {
            return Ok(());
        }
        debug_assert!(is_aligned(vaddr));
        let ret = unsafe { libc::munmap(vaddr as _, size) };
        assert_eq!(ret, 0, "failed to munmap: {:?}", Error::last_os_error());
        Ok(())
    }
}

impl From<MMUFlags> for libc::c_int {
    fn from(f: MMUFlags) -> libc::c_int {
        let mut flags = 0;
        if f.contains(MMUFlags::READ) {
            flags |= libc::PROT_READ;
        }
        if f.contains(MMUFlags::WRITE) {
            flags |= libc::PROT_WRITE;
        }
        if f.contains(MMUFlags::EXECUTE) {
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
        pt.map(Page::new_aligned(VBASE, PageSize::Size4K), 0x1000, flags)
            .unwrap();
        pt.map(
            Page::new_aligned(VBASE + 0x1000, PageSize::Size4K),
            0x1000,
            flags,
        )
        .unwrap();

        unsafe {
            const MAGIC: usize = 0xdead_beaf;
            (VBASE as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x1000) as *mut usize).read(), MAGIC);
        }

        pt.unmap(VBASE + 0x1000).unwrap();
    }
}

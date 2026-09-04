use crate::{addr::is_aligned, MMUFlags, PhysAddr, VirtAddr};

/// Errors may occur during address translation.
#[derive(Debug)]
pub enum PagingError {
    NoMemory,
    NotMapped,
    AlreadyMapped,
}

/// Address translation result.
pub type PagingResult<T = ()> = Result<T, PagingError>;

/// The [`PagingError::NotMapped`] can be ignored.
pub trait IgnoreNotMappedErr {
    /// If self is `Err(PagingError::NotMapped`, ignores the error and returns
    /// `Ok(())`, otherwise remain unchanged.
    fn ignore(self) -> PagingResult;
}

impl<T> IgnoreNotMappedErr for PagingResult<T> {
    fn ignore(self) -> PagingResult {
        match self {
            Ok(_) | Err(PagingError::NotMapped) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Possible page size.
#[repr(usize)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[allow(clippy::enum_clike_unportable_variant)]
pub enum PageSize {
    Size4K = 0x1000,
    Size16K = 0x4000,
    Size64K = 0x1_0000,
    Size2M = 0x20_0000,
    Size32M = 0x200_0000,
    Size512M = 0x2000_0000,
    Size1G = 0x4000_0000,
    Size64G = 0x10_0000_0000,
}

/// A sized page.
#[derive(Debug, Copy, Clone)]
pub struct Page {
    pub vaddr: VirtAddr,
    pub size: PageSize,
}

impl PageSize {
    pub const BASE: Self = {
        #[cfg(feature = "page-16k")]
        {
            Self::Size16K
        }
        #[cfg(feature = "page-64k")]
        {
            Self::Size64K
        }
        #[cfg(not(any(feature = "page-16k", feature = "page-64k")))]
        {
            Self::Size4K
        }
    };

    pub const HUGE_L2: Self = {
        #[cfg(feature = "page-16k")]
        {
            Self::Size32M
        }
        #[cfg(feature = "page-64k")]
        {
            Self::Size512M
        }
        #[cfg(not(any(feature = "page-16k", feature = "page-64k")))]
        {
            Self::Size2M
        }
    };

    pub const HUGE_L3: Self = {
        #[cfg(feature = "page-16k")]
        {
            Self::Size64G
        }
        #[cfg(feature = "page-64k")]
        {
            Self::Size512M
        }
        #[cfg(not(any(feature = "page-16k", feature = "page-64k")))]
        {
            Self::Size1G
        }
    };

    pub const fn is_aligned(self, addr: usize) -> bool {
        self.page_offset(addr) == 0
    }

    pub const fn align_down(self, addr: usize) -> usize {
        addr & !(self as usize - 1)
    }

    pub const fn page_offset(self, addr: usize) -> usize {
        addr & (self as usize - 1)
    }

    pub const fn is_huge(self) -> bool {
        self as usize > Self::BASE as usize
    }
}

impl Page {
    pub fn new_aligned(vaddr: VirtAddr, size: PageSize) -> Self {
        debug_assert!(size.is_aligned(vaddr));
        Self { vaddr, size }
    }
}

/// A generic page table abstraction.
pub trait GenericPageTable: Sync + Send {
    /// Get the physical address of root page table.
    fn table_phys(&self) -> PhysAddr;

    /// Map the `page` to the frame of `paddr` with `flags`.
    fn map(&mut self, page: Page, paddr: PhysAddr, flags: MMUFlags) -> PagingResult;

    /// Unmap the page of `vaddr`.
    fn unmap(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)>;

    /// Change the `flags` of the page of `vaddr`.
    fn update(
        &mut self,
        vaddr: VirtAddr,
        paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize>;

    /// Query the physical address which the page of `vaddr` maps to.
    fn query(&self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, MMUFlags, PageSize)>;

    fn map_cont(
        &mut self,
        start_vaddr: VirtAddr,
        size: usize,
        start_paddr: PhysAddr,
        flags: MMUFlags,
    ) -> PagingResult {
        assert!(is_aligned(start_vaddr));
        assert!(is_aligned(start_vaddr));
        assert!(is_aligned(size));
        debug!(
            "map_cont: {:#x?} => {:#x}, flags={:?}",
            start_vaddr..start_vaddr + size,
            start_paddr,
            flags
        );
        let mut vaddr = start_vaddr;
        let mut paddr = start_paddr;
        let end_vaddr = vaddr + size;
        if flags.contains(MMUFlags::HUGE_PAGE) {
            while vaddr < end_vaddr {
                let remains = end_vaddr - vaddr;
                let page_size = if remains >= PageSize::HUGE_L3 as usize
                    && PageSize::HUGE_L3.is_aligned(vaddr)
                    && PageSize::HUGE_L3.is_aligned(paddr)
                {
                    PageSize::HUGE_L3
                } else if remains >= PageSize::HUGE_L2 as usize
                    && PageSize::HUGE_L2.is_aligned(vaddr)
                    && PageSize::HUGE_L2.is_aligned(paddr)
                {
                    PageSize::HUGE_L2
                } else {
                    PageSize::BASE
                };
                let page = Page::new_aligned(vaddr, page_size);
                self.map(page, paddr, flags)?;
                vaddr += page_size as usize;
                paddr += page_size as usize;
            }
        } else {
            while vaddr < end_vaddr {
                let page_size = PageSize::BASE;
                let page = Page::new_aligned(vaddr, page_size);
                self.map(page, paddr, flags)?;
                vaddr += page_size as usize;
                paddr += page_size as usize;
            }
        }
        Ok(())
    }

    fn unmap_cont(&mut self, start_vaddr: VirtAddr, size: usize) -> PagingResult {
        assert!(is_aligned(start_vaddr));
        assert!(is_aligned(size));
        debug!(
            "{:#x?} unmap_cont: {:#x?}",
            self.table_phys(),
            start_vaddr..start_vaddr + size
        );
        let mut vaddr = start_vaddr;
        let end_vaddr = vaddr + size;
        while vaddr < end_vaddr {
            let page_size = match self.unmap(vaddr) {
                Ok((_, s)) => {
                    assert!(s.is_aligned(vaddr));
                    s as usize
                }
                Err(PagingError::NotMapped) => PageSize::Size4K as usize,
                Err(e) => return Err(e),
            };
            vaddr += page_size;
            assert!(vaddr <= end_vaddr);
        }
        Ok(())
    }
}

use crate::{MMUFlags, PhysAddr, VirtAddr};

#[derive(Debug)]
pub enum PagingError {
    NoMemory,
    NotMapped,
    AlreadyMapped,
}

pub type PagingResult<T = ()> = Result<T, PagingError>;

pub trait IgnoreNotMappedErr {
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

#[repr(usize)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PageSize {
    Size4K = 0x1000,
    Size2M = 0x20_0000,
    Size1G = 0x4000_0000,
}

#[derive(Debug, Copy, Clone)]
pub struct Page {
    pub vaddr: VirtAddr,
    pub size: PageSize,
}

impl PageSize {
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
        matches!(self, Self::Size1G | Self::Size2M)
    }
}

impl Page {
    pub fn new_aligned(vaddr: VirtAddr, size: PageSize) -> Self {
        debug_assert!(size.is_aligned(vaddr));
        Self { vaddr, size }
    }
}

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

    fn unmap_cont(&mut self, vaddr: VirtAddr, page_size: PageSize, count: usize) -> PagingResult {
        for i in 0..count {
            self.unmap(vaddr + i * page_size as usize).ignore()?;
        }
        Ok(())
    }
}

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

/// Possible page size (4K, 2M, 1G).
#[repr(usize)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PageSize {
    Size4K = 0x1000,
    Size2M = 0x20_0000,
    Size1G = 0x4000_0000,
}

/// A 4K, 2M or 1G size page.
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

    /// Like [`unmap`](Self::unmap), but the implementation may DEFER the
    /// cross-CPU TLB shootdown, leaving only the local flush. Callers looping
    /// over a range use this and then issue ONE
    /// [`remote_flush_all`](Self::remote_flush_all) at the end — the mmu-gather
    /// pattern. A per-page synchronous shootdown is O(pages × ack-wait); with a
    /// peer CPU that cannot ack promptly (spinning on a lock with IRQs off,
    /// e.g. a page fault contending for the same address space) each page burns
    /// the full spin budget and a large `munmap` turns into an hours-long
    /// livelock. Seen in practice: glibc's malloc arena setup
    /// (mmap 128 MiB, munmap the unaligned head) from a fresh labwc thread
    /// wedged the whole desktop.
    fn unmap_no_shootdown(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)> {
        self.unmap(vaddr)
    }

    /// Like [`update`](Self::update), but may defer the cross-CPU TLB
    /// shootdown (see [`unmap_no_shootdown`](Self::unmap_no_shootdown)).
    fn update_no_shootdown(
        &mut self,
        vaddr: VirtAddr,
        paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize> {
        self.update(vaddr, paddr, flags)
    }

    /// Flush the TLB of every other CPU once, synchronously. Pairs with the
    /// `*_no_shootdown` methods above. Default is a no-op for implementations
    /// (libos, tests) whose `unmap`/`update` don't defer anything.
    fn remote_flush_all(&self) {}

    /// Begin or end a *gather window* on this address space: while one is open,
    /// [`remote_flush_all`](Self::remote_flush_all) only records that a flush is
    /// owed instead of performing it, and closing the window performs at most
    /// one.
    ///
    /// This exists for `fork`, which write-protects every mapping of the parent
    /// one at a time. Each mapping costs *two* full cross-CPU shootdowns today —
    /// one inside `VmObject::create_child`'s `range_change`, one in
    /// `VmMapping::protect_for_cow` — and each shootdown is an IPI round trip
    /// to every other CPU with an ack spin-wait. Measured, that made a
    /// copy-on-write `fork` of a *small* process 3x more expensive than the
    /// eager copy it replaced (10 956 us against 3 602 us), because a process
    /// with many small mappings pays per mapping while saving only per page.
    ///
    /// Returns whether a flush was owed at the moment the window closed, so the
    /// caller can issue exactly one.
    ///
    /// **Opening a window widens a real race** and callers must know it is not
    /// possible for them. Between write-protecting a page and flushing, another
    /// CPU can still write through a stale writable TLB entry — onto a frame the
    /// child now shares. Today that window is the few instructions inside
    /// `protect_for_cow`; gathered, it is the whole `fork`. The only caller
    /// opens one when the forking process has a single thread, which is the case
    /// where no other CPU can be executing in that address space at all.
    fn set_gather(&mut self, _on: bool) -> bool {
        false
    }

    fn map_cont(
        &mut self,
        start_vaddr: VirtAddr,
        size: usize,
        start_paddr: PhysAddr,
        flags: MMUFlags,
    ) -> PagingResult {
        assert!(is_aligned(start_vaddr));
        assert!(is_aligned(start_paddr));
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
                let page_size = if remains >= PageSize::Size1G as usize
                    && PageSize::Size1G.is_aligned(vaddr)
                    && PageSize::Size1G.is_aligned(paddr)
                {
                    PageSize::Size1G
                } else if remains >= PageSize::Size2M as usize
                    && PageSize::Size2M.is_aligned(vaddr)
                    && PageSize::Size2M.is_aligned(paddr)
                {
                    PageSize::Size2M
                } else {
                    PageSize::Size4K
                };
                let page = Page::new_aligned(vaddr, page_size);
                self.map(page, paddr, flags)?;
                vaddr += page_size as usize;
                paddr += page_size as usize;
            }
        } else {
            while vaddr < end_vaddr {
                let page_size = PageSize::Size4K;
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
        // mmu-gather: clear every PTE first (local flush only), then shoot the
        // whole range down on the other CPUs with ONE synchronous IPI round.
        // The frames are not freed until after this function returns, so the
        // single flush at the end still closes the stale-TLB window before any
        // freed frame can be reused.
        let mut any_unmapped = false;
        while vaddr < end_vaddr {
            let page_size = match self.unmap_no_shootdown(vaddr) {
                Ok((_, s)) => {
                    assert!(s.is_aligned(vaddr));
                    any_unmapped = true;
                    s as usize
                }
                Err(PagingError::NotMapped) => PageSize::Size4K as usize,
                Err(e) => return Err(e),
            };
            vaddr += page_size;
            assert!(vaddr <= end_vaddr);
        }
        if any_unmapped {
            self.remote_flush_all();
        }
        Ok(())
    }
}

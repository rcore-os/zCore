use crate::{common::vm::*, mem::PhysFrame, MMUFlags, PhysAddr, VirtAddr};
use alloc::vec::Vec;
use core::{fmt::Debug, marker::PhantomData, slice};
use lock::Mutex;

pub trait PageTableLevel: Sync + Send {
    const LEVEL: usize;
}

pub struct PageTableLevel3;
pub struct PageTableLevel4;

impl PageTableLevel for PageTableLevel3 {
    const LEVEL: usize = 3;
}

impl PageTableLevel for PageTableLevel4 {
    const LEVEL: usize = 4;
}

pub trait GenericPTE: Debug + Clone + Copy + Sync + Send {
    /// Returns the physical address mapped by this entry.
    fn addr(&self) -> PhysAddr;
    /// Returns the flags of this entry.
    fn flags(&self) -> MMUFlags;
    /// Returns whether this entry is zero.
    fn is_unused(&self) -> bool;
    /// Returns whether this entry flag indicates present.
    fn is_present(&self) -> bool;
    /// Returns whether this entry maps to a huge frame (or it's a terminal entry).
    fn is_leaf(&self) -> bool;

    /// Set flags for all types of entries.
    fn set_flags(&mut self, flags: MMUFlags, is_huge: bool);
    /// Set physical address for terminal entries.
    fn set_addr(&mut self, paddr: PhysAddr);
    /// Set physical address and flags for intermediate table entries.
    fn set_table(&mut self, paddr: PhysAddr);
    /// Set this entry to zero.
    fn clear(&mut self);
}

pub struct PageTableImpl<L: PageTableLevel, PTE: GenericPTE> {
    /// Root table frame.
    root: PhysFrame,
    /// Intermediate level table frames.
    intrm_tables: Vec<PhysFrame>,
    /// Depth of the open mmu-gather window, and whether a shootdown is owed.
    ///
    /// A counter rather than a flag so nesting cannot have one level close a
    /// window another level still needs. See `GenericPageTable::set_gather`.
    gather: usize,
    gather_pending: bool,
    /// Phantom data.
    _phantom: PhantomData<(L, PTE)>,
}

/// Private implementation.
impl<L: PageTableLevel, PTE: GenericPTE> PageTableImpl<L, PTE> {
    unsafe fn from_root(root_paddr: PhysAddr) -> Self {
        Self {
            root: PhysFrame::from_paddr(root_paddr),
            intrm_tables: Vec::new(),
            gather: 0,
            gather_pending: false,
            _phantom: PhantomData,
        }
    }

    fn alloc_intrm_table(&mut self) -> Option<PhysAddr> {
        let frame = PhysFrame::new_zero()?;
        let paddr = frame.paddr();
        self.intrm_tables.push(frame);
        Some(paddr)
    }

    fn get_entry_mut(&self, vaddr: VirtAddr) -> PagingResult<(&mut PTE, PageSize)> {
        let p3 = if L::LEVEL == 3 {
            table_of_mut::<PTE>(self.table_phys())
        } else if L::LEVEL == 4 {
            let p4 = table_of_mut::<PTE>(self.table_phys());
            let p4e = &mut p4[p4_index(vaddr)];
            next_table_mut(p4e)?
        } else {
            unreachable!()
        };

        let p3e = &mut p3[p3_index(vaddr)];
        if p3e.is_leaf() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = next_table_mut(p3e)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if p2e.is_leaf() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = next_table_mut(p2e)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok((p1e, PageSize::Size4K))
    }

    fn get_entry_mut_or_create(&mut self, page: Page) -> PagingResult<&mut PTE> {
        let vaddr = page.vaddr;
        let p3 = if L::LEVEL == 3 {
            table_of_mut::<PTE>(self.table_phys())
        } else if L::LEVEL == 4 {
            let p4 = table_of_mut::<PTE>(self.table_phys());
            let p4e = &mut p4[p4_index(vaddr)];
            next_table_mut_or_create(p4e, || self.alloc_intrm_table())?
        } else {
            unreachable!()
        };

        let p3e = &mut p3[p3_index(vaddr)];
        if page.size == PageSize::Size1G {
            return Ok(p3e);
        }

        let p2 = next_table_mut_or_create(p3e, || self.alloc_intrm_table())?;
        let p2e = &mut p2[p2_index(vaddr)];
        if page.size == PageSize::Size2M {
            return Ok(p2e);
        }

        let p1 = next_table_mut_or_create(p2e, || self.alloc_intrm_table())?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok(p1e)
    }

    fn walk(
        &self,
        table: &[PTE],
        level: usize,
        start_vaddr: usize,
        limit: usize,
        func: &impl Fn(usize, usize, usize, &PTE),
    ) {
        let mut n = 0;
        for (i, entry) in table.iter().enumerate() {
            let vaddr = start_vaddr + (i << (12 + (3 - level) * 9));
            if entry.is_present() {
                func(level, i, vaddr, entry);
                if level < 3 && !entry.is_leaf() {
                    let table_entry = next_table_mut(entry).unwrap();
                    self.walk(table_entry, level + 1, vaddr, limit, func);
                }
                n += 1;
                if n >= limit {
                    break;
                }
            }
        }
    }

    #[allow(unused)]
    fn dump(&self, limit: usize, print_fn: impl Fn(core::fmt::Arguments)) {
        static LOCK: Mutex<()> = Mutex::new(());
        let _lock = LOCK.lock();

        print_fn(format_args!("Root: {:x?}\n", self.table_phys()));
        self.walk(
            table_of(self.table_phys()),
            0,
            0,
            limit,
            &|level: usize, idx: usize, vaddr: usize, entry: &PTE| {
                for _ in 0..level {
                    print_fn(format_args!("  "));
                }
                print_fn(format_args!(
                    "[{} - {:x}], {:08x?}: {:x?}\n",
                    level, idx, vaddr, entry
                ));
            },
        );
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub(crate) unsafe fn activate(&mut self) {
        crate::vm::activate_paging(self.table_phys());
    }
}

/// Public implementation.
impl<L: PageTableLevel, PTE: GenericPTE> PageTableImpl<L, PTE> {
    /// Address-space filter for this table's shootdowns: its own root, or
    /// `None` (target everyone) when this is the kernel's table — kernel
    /// entries can carry the global bit and survive CR3 switches, so no CPU
    /// may be skipped for them. Unknown kernel token (early boot, an arch
    /// that never publishes one) disables filtering entirely: over-targeting
    /// is a wasted IPI, under-targeting is a missed invalidation.
    fn aspace_filter(&self) -> Option<usize> {
        let root = self.table_phys();
        let kernel = crate::vm::kernel_vmtoken();
        if kernel == 0 || root & !0xfff == kernel & !0xfff {
            None
        } else {
            Some(root)
        }
    }

    pub fn new() -> Self {
        let root = PhysFrame::new_zero().expect("failed to alloc frame");
        Self {
            root,
            intrm_tables: Vec::new(),
            gather: 0,
            gather_pending: false,
            _phantom: PhantomData,
        }
    }

    /// Create a new `PageTable` from current VM token. (e.g. CR3, SATP, ...)
    pub fn from_current() -> Self {
        unsafe { Self::from_root(crate::vm::current_vmtoken()) }
    }

    pub fn clone_kernel(&self) -> Self {
        let pt = Self::new();
        crate::vm::pt_clone_kernel_space(pt.table_phys(), self.table_phys());
        pt
    }
}

impl<L: PageTableLevel, PTE: GenericPTE> Default for PageTableImpl<L, PTE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L: PageTableLevel, PTE: GenericPTE> GenericPageTable for PageTableImpl<L, PTE> {
    fn table_phys(&self) -> PhysAddr {
        self.root.paddr()
    }

    fn map(&mut self, page: Page, paddr: PhysAddr, flags: MMUFlags) -> PagingResult {
        let entry = self.get_entry_mut_or_create(page)?;
        if !entry.is_unused() {
            return Err(PagingError::AlreadyMapped);
        }
        entry.set_addr(page.size.align_down(paddr));
        entry.set_flags(flags, page.size.is_huge());
        crate::vm::flush_tlb(Some(page.vaddr));
        trace!(
            "PageTable map: {:x?} -> {:x?}, flags={:?} in {:#x?}",
            page,
            paddr,
            flags,
            self.table_phys()
        );
        Ok(())
    }

    fn unmap(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)> {
        let ret = self.unmap_no_shootdown(vaddr)?;
        // Removing a mapping leaves stale TLB entries on the other CPUs that
        // still point at the old frame; once it is freed and reused this
        // corrupts the new owner. Shoot down the entry on every other online
        // CPU. Range operations use `unmap_no_shootdown` in a loop plus one
        // `remote_flush_all` instead — a synchronous shootdown per page is
        // O(pages × ack-wait) and livelocks when a peer can't ack.
        crate::common::ipi::remote_flush_tlb_aspace(Some(vaddr), self.aspace_filter());
        Ok(ret)
    }

    fn unmap_no_shootdown(&mut self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, PageSize)> {
        // Inside a gather window the paired `remote_flush_all` is swallowed, so
        // record here that one is owed. Recorded before the fallible lookup on
        // purpose: an entry that turns out to be absent owes nothing, but an
        // extra flush costs one IPI and a missed one costs silent corruption.
        self.gather_pending |= self.gather > 0;
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        let paddr = entry.addr();
        entry.clear();
        crate::vm::flush_tlb(Some(vaddr));
        trace!("PageTable unmap: {:x?} in {:#x?}", vaddr, self.table_phys());
        Ok((paddr, size))
    }

    fn update(
        &mut self,
        vaddr: VirtAddr,
        paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize> {
        let size = self.update_no_shootdown(vaddr, paddr, flags)?;
        // Reducing permissions / repointing a live mapping (e.g. COW write
        // protect) must invalidate the stale entry on the other CPUs too.
        crate::common::ipi::remote_flush_tlb_aspace(Some(vaddr), self.aspace_filter());
        Ok(size)
    }

    fn update_no_shootdown(
        &mut self,
        vaddr: VirtAddr,
        paddr: Option<PhysAddr>,
        flags: Option<MMUFlags>,
    ) -> PagingResult<PageSize> {
        // See `unmap_no_shootdown`: inside a gather window this records the
        // shootdown the swallowed `remote_flush_all` would have performed.
        self.gather_pending |= self.gather > 0;
        let (entry, size) = self.get_entry_mut(vaddr)?;
        // Symmetric with `unmap_no_shootdown`/`query`: an update must never
        // resurrect a decommitted leaf. `get_entry_mut` returns a leaf as long
        // as the intermediate P4..P1 tables are present, even if the leaf
        // itself was cleared (e.g. by a prior `unmap`). Without this guard,
        // `update(va, None, Some(RXW))` on such a cleared leaf stamps
        // PRESENT|WRITABLE onto an entry whose addr() is 0, producing a phantom
        // PTE mapping the VA to physical frame 0 — a store then lands in phys 0
        // with no fault and no VMO commit (the demand-paged anon corruption that
        // aborted mimalloc/apk with "corrupted free list entry" / SIGSEGV).
        // Returning NotMapped lets the `.ignore()` at the mprotect/range_change
        // call sites be the true no-op they assume, so the page stays unmapped
        // and re-faults cleanly into `handle_page_fault`.
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        if let Some(paddr) = paddr {
            entry.set_addr(paddr);
        }
        if let Some(flags) = flags {
            entry.set_flags(flags, size.is_huge());
        }
        crate::vm::flush_tlb(Some(vaddr));
        trace!(
            "PageTable update: {:x?}, flags={:?} in {:#x?}",
            vaddr,
            flags,
            self.table_phys()
        );
        Ok(size)
    }

    fn remote_flush_all(&self) {
        if self.gather > 0 {
            // A gather window is open: record the debt and let the caller that
            // opened it pay once. `set_gather` is the only way back out, and it
            // reports what is owed.
            //
            // Interior mutability through `&self` would be needed to record it
            // here, so the flag is set by the `&mut self` paths instead — see
            // `note_gathered_flush`, called from `unmap_no_shootdown` and
            // `update_no_shootdown`, which are the only operations that can
            // leave a stale remote entry inside a window.
            return;
        }
        crate::common::ipi::remote_flush_tlb_aspace(None, self.aspace_filter());
    }

    fn set_gather(&mut self, on: bool) -> bool {
        if on {
            self.gather += 1;
            false
        } else {
            self.gather = self.gather.saturating_sub(1);
            if self.gather > 0 {
                return false; // an outer window is still open
            }
            core::mem::take(&mut self.gather_pending)
        }
    }

    fn query(&self, vaddr: VirtAddr) -> PagingResult<(PhysAddr, MMUFlags, PageSize)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        let off = size.page_offset(vaddr);
        let ret = (entry.addr() + off, entry.flags(), size);
        trace!("PageTable query: {:x?} => {:x?}", vaddr, ret);
        Ok(ret)
    }
}

const ENTRY_COUNT: usize = 512;

const fn p4_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 27)) & (ENTRY_COUNT - 1)
}

const fn p3_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 18)) & (ENTRY_COUNT - 1)
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 9)) & (ENTRY_COUNT - 1)
}

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 12) & (ENTRY_COUNT - 1)
}

fn table_of<'a, E>(paddr: PhysAddr) -> &'a [E] {
    let ptr = crate::mem::phys_to_virt(paddr) as *const E;
    unsafe { slice::from_raw_parts(ptr, ENTRY_COUNT) }
}

fn table_of_mut<'a, E>(paddr: PhysAddr) -> &'a mut [E] {
    let ptr = crate::mem::phys_to_virt(paddr) as *mut E;
    unsafe { slice::from_raw_parts_mut(ptr, ENTRY_COUNT) }
}

fn next_table_mut<'a, E: GenericPTE>(entry: &E) -> PagingResult<&'a mut [E]> {
    if !entry.is_present() {
        Err(PagingError::NotMapped)
    } else {
        debug_assert!(!entry.is_leaf());
        Ok(table_of_mut(entry.addr()))
    }
}

fn next_table_mut_or_create<'a, E: GenericPTE>(
    entry: &mut E,
    mut allocator: impl FnMut() -> Option<PhysAddr>,
) -> PagingResult<&'a mut [E]> {
    if entry.is_unused() {
        let paddr = allocator().ok_or(PagingError::NoMemory)?;
        entry.set_table(paddr);
        Ok(table_of_mut(paddr))
    } else {
        next_table_mut(entry)
    }
}

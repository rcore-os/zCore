use crate::sync::Mutex;
use crate::{
    common::vm::*, mem::PhysFrame, MMUFlags, PhysAddr, VirtAddr, PAGE_SIZE, PAGE_SIZE_LOG2,
};
use alloc::vec::Vec;
use core::{fmt::Debug, marker::PhantomData, slice};

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
    /// Phantom data.
    _phantom: PhantomData<(L, PTE)>,
}

/// Private implementation.
impl<L: PageTableLevel, PTE: GenericPTE> PageTableImpl<L, PTE> {
    unsafe fn from_root(root_paddr: PhysAddr) -> Self {
        Self {
            root: unsafe { PhysFrame::from_paddr(root_paddr) },
            intrm_tables: Vec::new(),
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
            let p4e = &mut p4[root_index(vaddr, 4)];
            next_table_mut(p4e)?
        } else {
            unreachable!()
        };

        let p3e = &mut p3[if L::LEVEL == 3 {
            root_index(vaddr, 3)
        } else {
            p3_index(vaddr)
        }];
        if p3e.is_leaf() {
            return Ok((p3e, PageSize::HUGE_L3));
        }

        let p2 = next_table_mut(p3e)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if p2e.is_leaf() {
            return Ok((p2e, PageSize::HUGE_L2));
        }

        let p1 = next_table_mut(p2e)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok((p1e, PageSize::BASE))
    }

    fn get_entry_mut_or_create(&mut self, page: Page) -> PagingResult<&mut PTE> {
        let vaddr = page.vaddr;
        let p3 = if L::LEVEL == 3 {
            table_of_mut::<PTE>(self.table_phys())
        } else if L::LEVEL == 4 {
            let p4 = table_of_mut::<PTE>(self.table_phys());
            let p4e = &mut p4[root_index(vaddr, 4)];
            next_table_mut_or_create(p4e, || self.alloc_intrm_table())?
        } else {
            unreachable!()
        };

        let p3e = &mut p3[if L::LEVEL == 3 {
            root_index(vaddr, 3)
        } else {
            p3_index(vaddr)
        }];
        if page.size == PageSize::HUGE_L3 {
            return Ok(p3e);
        }

        let p2 = next_table_mut_or_create(p3e, || self.alloc_intrm_table())?;
        let p2e = &mut p2[p2_index(vaddr)];
        if page.size == PageSize::HUGE_L2 {
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
            let vaddr = start_vaddr + (i << (PAGE_SIZE_LOG2 + (L::LEVEL - 1 - level) * INDEX_BITS));
            if entry.is_present() {
                func(level, i, vaddr, entry);
                if level < L::LEVEL - 1 && !entry.is_leaf() {
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
    pub fn new() -> Self {
        let root = PhysFrame::new_zero().expect("failed to alloc frame");
        Self {
            root,
            intrm_tables: Vec::new(),
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
        let (entry, size) = self.get_entry_mut(vaddr)?;
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

const ENTRY_COUNT: usize = PAGE_SIZE / core::mem::size_of::<usize>();
const INDEX_BITS: usize = ENTRY_COUNT.trailing_zeros() as usize;

/// Translatable virtual address width, in bits.
///
/// Only used to narrow the *root* table index below. Configurations whose root
/// still consumes a full `INDEX_BITS` (4K/4-level, RISC-V Sv39/Sv48) are
/// unaffected by the clamp in [`root_index_bits`].
const VA_BITS: usize = 48;

/// Bit-shift of the root table's index for a table with `level` levels.
const fn root_shift(level: usize) -> usize {
    PAGE_SIZE_LOG2 + (level - 1) * INDEX_BITS
}

/// Number of index bits the *root* table actually uses.
///
/// The lower levels always consume `INDEX_BITS`, but the root only covers
/// whatever is left of the virtual address, which can be fewer. With a 16K
/// granule and a 48-bit VA, for example, the walk is `14 + 3*11` and the root
/// (level 0) is selected by VA[47] alone — a single bit. Masking the root index
/// with the full `INDEX_BITS` there would place entries at index 2047 while the
/// hardware page-table walker reads index 1, so every mapping would silently
/// resolve to a zero descriptor.
const fn root_index_bits(level: usize) -> usize {
    let shift = root_shift(level);
    let remaining = VA_BITS - shift;
    if remaining < INDEX_BITS {
        remaining
    } else {
        INDEX_BITS
    }
}

/// Index of `vaddr` into the root table of a table with `level` levels.
const fn root_index(vaddr: usize, level: usize) -> usize {
    (vaddr >> root_shift(level)) & ((1 << root_index_bits(level)) - 1)
}

// The level-4 (root) index is computed by `root_index`, which additionally
// clamps the mask to the bits the hardware walker actually decodes.

const fn p3_index(vaddr: usize) -> usize {
    (vaddr >> (PAGE_SIZE_LOG2 + 2 * INDEX_BITS)) & (ENTRY_COUNT - 1)
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> (PAGE_SIZE_LOG2 + INDEX_BITS)) & (ENTRY_COUNT - 1)
}

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> PAGE_SIZE_LOG2) & (ENTRY_COUNT - 1)
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

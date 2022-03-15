//! Definition of phyical, virtual addresses and helper functions.

use crate::PAGE_SIZE;

/// Physical address.
pub type PhysAddr = usize;

/// Virtual address.
pub type VirtAddr = usize;

/// Device address.
pub type DevVAddr = usize;

/// Returns the address of the nearest page that is not larger than the given address.
pub const fn align_down(addr: usize) -> usize {
    addr & !(PAGE_SIZE - 1)
}

/// Returns the address of the nearest page whose address is not smaller than the given one.
pub const fn align_up(addr: usize) -> usize {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Returns true if the given address is aligned with the page, else false.
pub const fn is_aligned(addr: usize) -> bool {
    page_offset(addr) == 0
}

/// The page number of the page where the given address resides.
pub const fn page_count(size: usize) -> usize {
    align_up(size) / PAGE_SIZE
}

/// Returns the offset address of given address within a page.
pub const fn page_offset(addr: usize) -> usize {
    addr & (PAGE_SIZE - 1)
}

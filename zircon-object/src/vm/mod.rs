//! Objects for Virtual Memory Management.

mod consts;
mod stream;
mod vmar;
mod vmo;

pub use self::{stream::*, vmar::*, vmo::*};
use super::{ZxError, ZxResult};
use alloc::sync::Arc;
pub use kernel_hal::{CachePolicy, MMUFlags};
use lazy_static::*;

/// Physical Address
pub type PhysAddr = usize;

/// Virtual Address
pub type VirtAddr = usize;

/// Device Address
pub type DevVAddr = usize;

/// Size of a page
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_LOG2: usize = 12;

pub fn page_aligned(x: usize) -> bool {
    x % PAGE_SIZE == 0
}

pub fn check_aligned(x: usize, align: usize) -> bool {
    x % align == 0
}

/// How many pages the `size` needs.
/// To avoid overflow and pass more unit tests, use wrapping add
pub fn pages(size: usize) -> usize {
    size.wrapping_add(PAGE_SIZE - 1) / PAGE_SIZE
}

pub fn ceil(x: usize, align: usize) -> usize {
    x.wrapping_add(align - 1) / align
}

pub fn roundup_pages(size: usize) -> usize {
    if page_aligned(size) {
        size
    } else {
        pages(size) * PAGE_SIZE
    }
}

pub fn round_down_pages(size: usize) -> usize {
    size / PAGE_SIZE * PAGE_SIZE
}

lazy_static! {
    pub static ref KERNEL_ASPACE: Arc<VmAddressRegion> = VmAddressRegion::new_kernel();
}

pub fn kernel_allocate_physical(
    size: usize,
    paddr: PhysAddr,
    mmu_flags: MMUFlags,
    cache_policy: CachePolicy,
) -> ZxResult<VirtAddr> {
    if !page_aligned(paddr) {
        return Err(ZxError::INVALID_ARGS);
    }
    let size = roundup_pages(size);
    let vmo = VmObject::new_physical(paddr, pages(size));
    vmo.set_cache_policy(cache_policy)?;
    let flags = mmu_flags - MMUFlags::CACHE_1 - MMUFlags::CACHE_2;
    KERNEL_ASPACE.map(None, vmo, 0, size, flags)
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_round_pages() {
        assert_eq!(roundup_pages(0), 0);
        assert_eq!(roundup_pages(core::usize::MAX), 0);
        assert_eq!(
            roundup_pages(core::usize::MAX - PAGE_SIZE + 1),
            core::usize::MAX - PAGE_SIZE + 1
        );
        assert_eq!(roundup_pages(PAGE_SIZE * 3 - 1), PAGE_SIZE * 3);
    }
}

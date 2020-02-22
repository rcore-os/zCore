//! Objects for Virtual Memory Management.

mod vmar;
mod vmo;

pub use self::{vmar::*, vmo::*};
pub use kernel_hal::MMUFlags;

/// Physical Address
pub type PhysAddr = usize;

/// Virtual Address
pub type VirtAddr = usize;

/// Size of a page
pub const PAGE_SIZE: usize = 0x1000;

pub fn page_aligned(x: usize) -> bool {
    x % PAGE_SIZE == 0
}

/// How many pages the `size` needs.
pub fn pages(size: usize) -> usize {
    (size + PAGE_SIZE - 1) / PAGE_SIZE
}

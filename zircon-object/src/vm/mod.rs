mod vmar;
mod vmo;

pub use self::{vmar::*, vmo::*};
pub use crate::hal::MMUFlags;

/// Physical Address
pub type PhysAddr = usize;

/// Virtual Address
pub type VirtAddr = usize;

pub const PAGE_SIZE: usize = 0x1000;

fn page_aligned(x: usize) -> bool {
    x % PAGE_SIZE == 0
}

pub fn pages(size: usize) -> usize {
    (size + PAGE_SIZE - 1) / PAGE_SIZE
}

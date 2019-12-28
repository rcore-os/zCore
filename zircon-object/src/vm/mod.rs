pub mod vmar;
pub mod vmo;

/// Physical Address
pub type PhysAddr = usize;

/// Virtual Address
pub type VirtAddr = usize;

pub const PAGE_SIZE: usize = 0x1000;

fn page_aligned(x: usize) -> bool {
    x % 0x1000 == 0
}

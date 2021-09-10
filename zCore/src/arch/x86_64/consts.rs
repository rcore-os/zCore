// x86_64

pub const MEMORY_OFFSET: usize = 0;
pub const KERNEL_OFFSET: usize = 0xffff_ff00_0000_0000;
pub const PHYSICAL_MEMORY_OFFSET: usize = 0xffff_8000_0000_0000;
pub const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB
pub const PAGE_SIZE: usize = 1 << 12;

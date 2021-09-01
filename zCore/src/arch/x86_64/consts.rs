// x86_64

pub const MEMORY_OFFSET: usize = 0;
pub const KERNEL_OFFSET: usize = 0xffffff00_00000000;
pub const PHYSICAL_MEMORY_OFFSET: usize = 0xffff8000_00000000;
pub const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB
pub const PAGE_SIZE: usize = 1 << 12;

pub const KERNEL_PM4: usize = (KERNEL_OFFSET >> 39) & 0o777;
pub const PHYSICAL_MEMORY_PM4: usize = (PHYSICAL_MEMORY_OFFSET >> 39) & 0o777;

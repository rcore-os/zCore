// RISCV

cfg_if! {
    if #[cfg(feature = "board-qemu")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_8000_0000;
        pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
    } else if #[cfg(feature = "board-d1")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_C000_0000;
        pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;
    }
}

pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - PHYS_MEMORY_BASE;
pub const KERNEL_HEAP_SIZE: usize = 80 * 1024 * 1024; // 80 MB
pub const STACK_PAGES_PER_HART: usize = 32;
pub const MAX_HART_NUM: usize = 8;

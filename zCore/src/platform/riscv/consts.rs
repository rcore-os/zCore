// RISCV

cfg_if::cfg_if! {
    if #[cfg(feature = "board_qemu")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_8000_0000;
        pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
        pub const PHYS_MEMORY_END: usize = 0x8800_0000; // TODO: get memory end from device tree
    } else if #[cfg(feature = "board_d1")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_C000_0000;
        pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;
        pub const PHYS_MEMORY_END: usize = 0x6000_0000; // 512M
    }
}

pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - PHYS_MEMORY_BASE;

pub const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MB

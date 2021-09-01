// RISCV

#[cfg(feature = "board_qemu")]
pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_8000_0000;
#[cfg(feature = "board_qemu")]
pub const MEMORY_OFFSET: usize = 0x8000_0000;
#[cfg(feature = "board_qemu")]
pub const MEMORY_END: usize = 0x8800_0000; // TODO: get memory end from device tree

#[cfg(feature = "board_d1")]
pub const KERNEL_OFFSET: usize = 0xFFFFFFFF_C0000000;
#[cfg(feature = "board_d1")]
pub const MEMORY_OFFSET: usize = 0x40000000;
#[cfg(feature = "board_d1")]
pub const MEMORY_END: usize = 0x60000000; // 512M

pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - MEMORY_OFFSET;

pub const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MB
pub const PAGE_SIZE: usize = 1 << 12;

pub const KERNEL_L2: usize = (KERNEL_OFFSET >> 30) & 0o777;
pub const PHYSICAL_MEMORY_L2: usize = (PHYSICAL_MEMORY_OFFSET >> 30) & 0o777;

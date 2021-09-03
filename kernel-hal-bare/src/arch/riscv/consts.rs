// RISCV
// Linear mapping

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

pub const MAX_DTB_SIZE: usize = 0x2000;

pub const MMIO_MTIMECMP0: *mut u64 = 0x0200_4000usize as *mut u64;
pub const MMIO_MTIME: *const u64 = 0x0200_BFF8 as *const u64;

#[cfg(feature = "board_qemu")]
pub const UART_BASE: usize = 0x10000000;
#[cfg(feature = "board_qemu")]
pub const UART0_INT_NUM: u32 = 10;
#[cfg(feature = "board_qemu")]
pub const PLIC_PRIORITY: usize = 0x0c000000;
#[cfg(feature = "board_qemu")]
pub const PLIC_PENDING: usize = 0x0c001000;
#[cfg(feature = "board_qemu")]
pub const PLIC_INT_ENABLE: usize = 0x0c002080;
#[cfg(feature = "board_qemu")]
pub const PLIC_THRESHOLD: usize = 0x0c201000;
#[cfg(feature = "board_qemu")]
pub const PLIC_CLAIM: usize = 0x0c201004;

#[cfg(feature = "board_d1")]
pub const UART_BASE: usize = 0x02500000;
#[cfg(feature = "board_d1")]
pub const UART0_INT_NUM: u32 = 18;
#[cfg(feature = "board_d1")]
pub const PLIC_PRIORITY: usize = 0x1000_0000;
#[cfg(feature = "board_d1")]
pub const PLIC_PENDING: usize = 0x1000_1000;
#[cfg(feature = "board_d1")]
pub const PLIC_INT_ENABLE: usize = 0x1000_2080;
#[cfg(feature = "board_d1")]
pub const PLIC_THRESHOLD: usize = 0x1020_1000;
#[cfg(feature = "board_d1")]
pub const PLIC_CLAIM: usize = 0x1020_1004;

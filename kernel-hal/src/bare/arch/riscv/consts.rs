cfg_if! {
    if #[cfg(feature = "board_qemu")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_8000_0000;
        pub const MEMORY_OFFSET: usize = 0x8000_0000;
        pub const MEMORY_END: usize = 0x8800_0000; // TODO: get memory end from device tree

        pub const UART_BASE: usize = 0x1000_0000;
        pub const PLIC_BASE: usize = 0x0C00_0000;

        pub const UART0_INT_NUM: u32 = 10;
    } else if #[cfg(feature = "board_d1")] {
        pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_C000_0000;
        pub const MEMORY_OFFSET: usize = 0x4000_0000;
        pub const MEMORY_END: usize = 0x6000_0000; // 512M

        pub const UART_BASE: usize = 0x0250_0000;
        pub const PLIC_BASE: usize = 0x1000_0000;

        pub const UART0_INT_NUM: u32 = 18;
    }
}

pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - MEMORY_OFFSET;

pub const PLIC_PRIORITY: usize = PLIC_BASE + 0x0;
pub const PLIC_PENDING: usize = PLIC_BASE + 0x1000;
pub const PLIC_INT_ENABLE: usize = PLIC_BASE + 0x2080;
pub const PLIC_THRESHOLD: usize = PLIC_BASE + 0x20_1000;
pub const PLIC_CLAIM: usize = PLIC_BASE + 0x20_1004;

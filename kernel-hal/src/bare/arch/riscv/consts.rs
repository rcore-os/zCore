cfg_if! {
    if #[cfg(feature = "board_qemu")] {
        pub const UART_BASE: usize = 0x1000_0000;
        pub const PLIC_BASE: usize = 0x0C00_0000;
        pub const UART0_INT_NUM: usize = 10;
    } else if #[cfg(feature = "board_d1")] {
        pub const UART_BASE: usize = 0x0250_0000;
        pub const PLIC_BASE: usize = 0x1000_0000;
        pub const UART0_INT_NUM: usize = 18;
    }
}

pub const PLIC_PRIORITY: usize = PLIC_BASE + 0x0;
pub const PLIC_PENDING: usize = PLIC_BASE + 0x1000;
pub const PLIC_INT_ENABLE: usize = PLIC_BASE + 0x2080;
pub const PLIC_THRESHOLD: usize = PLIC_BASE + 0x20_1000;
pub const PLIC_CLAIM: usize = PLIC_BASE + 0x20_1004;

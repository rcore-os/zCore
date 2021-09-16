mod uart_16550;
pub use uart_16550::Uart16550Mmio;

#[cfg(target_arch = "x86_64")]
pub use uart_16550::Uart16550Pio;

//! Uart device driver.

mod buffered;
mod uart_16550;
#[cfg(feature = "board-d1")]
mod uart_allwinner;
#[cfg(target_arch = "aarch64")]
mod uart_pl011;
mod uart_u740;

pub use buffered::BufferedUart;
pub use uart_16550::Uart16550Mmio;
pub use uart_u740::UartU740Mmio;

#[cfg(target_arch = "x86_64")]
pub use uart_16550::Uart16550Pmio;
#[cfg(feature = "board-d1")]
pub use uart_allwinner::UartAllwinner;
#[cfg(target_arch = "aarch64")]
pub use uart_pl011::Pl011Uart;

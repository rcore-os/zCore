//! Uart device driver.

mod buffered;
mod uart_16550;

pub use buffered::BufferedUart;
pub use uart_16550::Uart16550Mmio;

#[cfg(target_arch = "x86_64")]
pub use uart_16550::Uart16550Pmio;

#[cfg(target_arch = "aarch64")]
mod uart_pl011;

#[cfg(target_arch = "aarch64")]
pub use uart_pl011::Pl011Uart;

#[cfg(feature = "allwinner")]
mod uart_allwinner;

#[cfg(feature = "allwinner")]
pub use uart_allwinner::UartAllwinner;

#[cfg(feature = "fu740")]
mod uart_u740;

#[cfg(feature = "fu740")]
pub use uart_u740::UartU740Mmio;

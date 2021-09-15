mod uart_16550;
pub use uart_16550::Uart16550;

#[cfg(feature = "virtio")]
mod virtio_console;
#[cfg(feature = "virtio")]
pub use virtio_console::VirtIoConsole;

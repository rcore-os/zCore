use crate::drivers;
use zcore_drivers::uart::Pl011Uart;
use zcore_drivers::Device;
use alloc::sync::Arc;
use crate::imp::config::UART_ADDR;
use zcore_drivers::uart::BufferedUart;

pub fn init_early() {
    let uart = Arc::new(Pl011Uart::new(UART_ADDR));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
}

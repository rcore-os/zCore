use alloc::sync::Arc;

use zcore_drivers::scheme::Scheme;
use zcore_drivers::{mock::MockUart, Device};

use crate::drivers;

pub(super) fn init() {
    let uart = Arc::new(MockUart::new());
    drivers::add_device(Device::Uart(uart.clone()));
    MockUart::start_irq_serve(move || uart.handle_irq(0));
}

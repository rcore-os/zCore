use alloc::boxed::Box;

use zcore_drivers::mock::MockUart;

use crate::drivers::UART;

pub(super) fn init() {
    UART.init_by(Box::new(MockUart::new()));
}

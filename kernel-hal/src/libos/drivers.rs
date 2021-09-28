use alloc::sync::Arc;

use zcore_drivers::mock::MockUart;

use crate::drivers::UART;

pub(super) fn init() {
    UART.init_once_by(Arc::new(MockUart::new()));
    MockUart::start_irq_serve(|| UART.handle_irq(0));
}

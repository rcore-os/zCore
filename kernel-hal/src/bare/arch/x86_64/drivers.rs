use alloc::boxed::Box;

use zcore_drivers::scheme::EventListener;
use zcore_drivers::uart::Uart16550Pio;

use crate::drivers::UART;

pub(super) fn init() {
    UART.init_by(Box::new(EventListener::new(Uart16550Pio::new(0x3F8))));
}

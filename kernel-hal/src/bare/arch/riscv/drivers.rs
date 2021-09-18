use alloc::boxed::Box;

use zcore_drivers::scheme::EventListener;
use zcore_drivers::uart::Uart16550Mmio;

use crate::{drivers::UART, mem::phys_to_virt};

pub(super) fn init() {
    UART.init_by(Box::new(EventListener::new(unsafe {
        Uart16550Mmio::<u8>::new(phys_to_virt(super::consts::UART_BASE))
    })));
}

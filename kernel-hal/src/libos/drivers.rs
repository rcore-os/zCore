use alloc::sync::Arc;

use zcore_drivers::mock::uart::MockUart;
use zcore_drivers::{scheme::Scheme, Device};

pub(super) fn init() {
    let uart = Arc::new(MockUart::new());
    crate::drivers::add_device(Device::Uart(uart.clone()));
    MockUart::start_irq_serve(move || uart.handle_irq(0));

    #[cfg(feature = "graphic")]
    {
        use zcore_drivers::mock::display::MockDisplay;
        let display = Arc::new(MockDisplay::new(1280, 720));
        crate::drivers::add_device(Device::Display(display.clone()));
        crate::console::init_graphic_console(display);
    }
}

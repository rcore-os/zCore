use core::fmt::{Arguments, Write};

use spin::Mutex;
use zcore_drivers::scheme::{Scheme, UartScheme};
use zcore_drivers::{io::Pio, uart::Uart16550};

pub(super) static COM1: Mutex<Uart16550<Pio<u8>>> = Mutex::new(Uart16550::<Pio<u8>>::new(0x3F8));

pub(super) fn init() {
    COM1.lock().init().unwrap();
}

pub(super) fn handle_irq() {
    if let Some(c) = COM1.lock().try_recv().unwrap() {
        crate::serial::serial_put(c);
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::serial {
        fn serial_write_fmt(fmt: Arguments) {
            COM1.lock().write_fmt(fmt).unwrap();
        }
    }
}

use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::fmt::{Arguments, Write};

use spin::Mutex;
use uart_16550::SerialPort;

lazy_static::lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

pub(super) static COM1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });

pub(super) fn init() {
    COM1.lock().init();
}

hal_fn_impl! {
    impl mod crate::hal_fn::serial {
        fn serial_write_fmt(fmt: Arguments) {
            COM1.lock().write_fmt(fmt).unwrap();
        }
    }
}

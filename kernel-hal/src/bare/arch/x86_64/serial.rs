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
    impl mod crate::defs::serial {
        fn serial_put(x: u8) {
            let x = if x == b'\r' { b'\n' } else { x };
            STDIN.lock().push_back(x);
            STDIN_CALLBACK.lock().retain(|f| !f());
        }

        fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
            STDIN_CALLBACK.lock().push(callback);
        }

        fn serial_read(buf: &mut [u8]) -> usize {
            let mut stdin = STDIN.lock();
            let len = stdin.len().min(buf.len());
            for c in &mut buf[..len] {
                *c = stdin.pop_front().unwrap();
            }
            len
        }

        fn serial_write_fmt(fmt: Arguments) {
            COM1.lock().write_fmt(fmt).unwrap();
            // if let Some(console) = CONSOLE.lock().as_mut() {
            //     console.write_fmt(fmt).unwrap();
            // }
        }
    }
}

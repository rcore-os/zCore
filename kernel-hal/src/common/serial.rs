use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::fmt::{Arguments, Result, Write};

use spin::Mutex;

use crate::drivers::UART;

lazy_static::lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = UART.try_get() {
            uart.write_str(s).unwrap();
        }
        Ok(())
    }
}

pub(crate) fn handle_irq() {
    if let Some(uart) = UART.try_get() {
        if let Some(c) = uart.try_recv().unwrap() {
            let c = if c == b'\r' { b'\n' } else { c };
            STDIN.lock().push_back(c);
            STDIN_CALLBACK.lock().retain(|f| !f());
        }
    }
}

/// Register a callback of serial readable event.
pub fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
    STDIN_CALLBACK.lock().push(callback);
}

/// Read a string from serial buffer.
pub fn serial_read(buf: &mut [u8]) -> usize {
    let mut stdin = STDIN.lock();
    let len = stdin.len().min(buf.len());
    for c in &mut buf[..len] {
        *c = stdin.pop_front().unwrap();
    }
    len
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SerialWriter.write_fmt(fmt).unwrap();
}

/// Print a string to serial.
pub fn serial_write(s: &str) {
    serial_write_fmt(format_args!("{}", s));
}

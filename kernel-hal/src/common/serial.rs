use alloc::{boxed::Box, collections::VecDeque, vec::Vec};

use spin::Mutex;

lazy_static::lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

/// Put a char to serial buffer.
pub fn serial_put(x: u8) {
    let x = if x == b'\r' { b'\n' } else { x };
    STDIN.lock().push_back(x);
    STDIN_CALLBACK.lock().retain(|f| !f());
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

/// Print a string to serial.
pub fn serial_write(s: &str) {
    crate::serial::serial_write_fmt(format_args!("{}", s));
}

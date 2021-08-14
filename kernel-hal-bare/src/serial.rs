use spin::Mutex;
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};

lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

/// Put a char by serial interrupt handler.
pub fn serial_put(mut x: u8) {
    if x == b'\r' {
        x = b'\n';
    }
    STDIN.lock().push_back(x);
    STDIN_CALLBACK.lock().retain(|f| !f());
}

#[export_name = "hal_serial_set_callback"]
pub fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
    STDIN_CALLBACK.lock().push(callback);
}

#[export_name = "hal_serial_read"]
pub fn serial_read(buf: &mut [u8]) -> usize {
    let mut stdin = STDIN.lock();
    let len = stdin.len().min(buf.len());
    for c in &mut buf[..len] {
        *c = stdin.pop_front().unwrap();
    }
    len
}

#[export_name = "hal_serial_write"]
pub fn serial_write(s: &str) {
    crate::arch::putfmt(format_args!("{}", s));
}

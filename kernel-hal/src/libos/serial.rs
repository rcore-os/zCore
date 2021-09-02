use lazy_static::lazy_static;
use std::collections::VecDeque;
use std::sync::Mutex;

lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

/// Put a char by serial interrupt handler.
pub fn serial_put(x: u8) {
    STDIN.lock().unwrap().push_back(x);
    STDIN_CALLBACK.lock().unwrap().retain(|f| !f());
}

pub fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
    STDIN_CALLBACK.lock().unwrap().push(callback);
}

pub fn serial_read(buf: &mut [u8]) -> usize {
    let mut stdin = STDIN.lock().unwrap();
    let len = stdin.len().min(buf.len());
    for c in &mut buf[..len] {
        *c = stdin.pop_front().unwrap();
    }
    len
}

/// Output a char to console.
pub fn serial_write(s: &str) {
    eprint!("{}", s);
}

use alloc::boxed::Box;

/// Put a char by serial interrupt handler.
pub fn serial_put(_x: u8) {
    unimplemented!()
}

/// Register a callback of serial readable event.
pub fn serial_set_callback(_callback: Box<dyn Fn() -> bool + Send + Sync>) {
    unimplemented!()
}

/// Read a string from console.
pub fn serial_read(_buf: &mut [u8]) -> usize {
    unimplemented!()
}

/// Output a string to console.
pub fn serial_write(_s: &str) {
    unimplemented!()
}

use core::fmt::{Arguments, Result, Write};

use crate::drivers;

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = drivers::uart::first() {
            uart.write_str(s).unwrap();
        } else {
            crate::hal_fn::serial::serial_write_early(s);
        }
        Ok(())
    }
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SerialWriter.write_fmt(fmt).unwrap();
}

/// Print a string to serial.
pub fn serial_write(s: &str) {
    serial_write_fmt(format_args!("{}", s));
}

/// Read buffer data from serial.
pub async fn serial_read(buf: &mut [u8]) -> usize {
    super::future::SerialReadFuture::new(buf).await
}

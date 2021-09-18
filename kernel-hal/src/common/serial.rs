use core::fmt::{Arguments, Result, Write};

use crate::drivers::UART;

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = UART.try_get() {
            uart.write_str(s).unwrap();
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

/// Get a char from serial.
pub async fn serial_read(buf: &mut [u8]) -> usize {
    super::future::SerialFuture::new(buf).await
}

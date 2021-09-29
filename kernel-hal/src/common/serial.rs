use alloc::sync::Arc;
use core::fmt::{Arguments, Result, Write};

use zcore_drivers::scheme::{IrqHandler, UartScheme};

use crate::drivers::UART;
use crate::utils::init_once::InitOnce;

struct SerialWriter(&'static InitOnce<Arc<dyn UartScheme>>);

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = self.0.try_get() {
            uart.write_str(s).unwrap();
        } else {
            crate::hal_fn::serial::serial_write_early(s);
        }
        Ok(())
    }
}

pub fn subscribe_event(handler: IrqHandler, once: bool) {
    UART.subscribe(handler, once);
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SerialWriter(&UART).write_fmt(fmt).unwrap();
}

/// Print a string to serial.
pub fn serial_write(s: &str) {
    serial_write_fmt(format_args!("{}", s));
}

/// Try to get a char from serial.
pub fn serial_try_read() -> Option<u8> {
    UART.try_recv().unwrap_or(None)
}

/// Read buffer data from serial.
pub async fn serial_read(buf: &mut [u8]) -> usize {
    super::future::SerialFuture::new(buf).await
}

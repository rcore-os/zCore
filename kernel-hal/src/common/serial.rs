use alloc::{boxed::Box, collections::VecDeque};
use core::fmt::{Arguments, Result, Write};

use spin::Mutex;
use zcore_drivers::scheme::IrqHandler;
use zcore_drivers::utils::EventListener;

use crate::drivers::UART;

const BUF_CAPACITY: usize = 4096;

struct BufferedSerial {
    buf: Mutex<VecDeque<u8>>,
    listener: Mutex<EventListener>,
}

impl BufferedSerial {
    fn new() -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(BUF_CAPACITY)),
            listener: Mutex::new(EventListener::new()),
        }
    }

    fn handle_irq(&self) {
        while let Some(c) = UART.try_recv().unwrap_or(None) {
            let c = if c == b'\r' { b'\n' } else { c };
            self.buf.lock().push_back(c);
        }
        if self.buf.lock().len() > 0 {
            self.listener.lock().trigger();
        }
    }
}

lazy_static! {
    static ref SERIAL: BufferedSerial = BufferedSerial::new();
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = UART.try_get() {
            uart.write_str(s).unwrap();
        } else {
            crate::hal_fn::serial::serial_write_early(s);
        }
        Ok(())
    }
}

pub(crate) fn init_listener() {
    let mut listener = EventListener::new();
    listener.subscribe(Box::new(|| SERIAL.handle_irq()), false);
    UART.bind_listener(listener);
}

pub fn subscribe_event(handler: IrqHandler, once: bool) {
    SERIAL.listener.lock().subscribe(handler, once);
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SerialWriter.write_fmt(fmt).unwrap();
}

/// Print a string to serial.
pub fn serial_write(s: &str) {
    serial_write_fmt(format_args!("{}", s));
}

/// Try to get a char from serial.
pub fn serial_try_read() -> Option<u8> {
    SERIAL.buf.lock().pop_front()
}

/// Read buffer data from serial.
pub async fn serial_read(buf: &mut [u8]) -> usize {
    super::future::SerialFuture::new(buf).await
}

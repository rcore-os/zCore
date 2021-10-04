use core::fmt::{Arguments, Result, Write};

use crate::drivers;

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = drivers::uart::first() {
            uart.write_str(s).unwrap();
        } else {
            crate::hal_fn::console::console_write_early(s);
        }
        Ok(())
    }
}

cfg_if! {
    if #[cfg(feature = "graphic")] {
        use crate::utils::init_once::InitOnce;
        use alloc::sync::Arc;
        use spin::Mutex;
        use zcore_drivers::{scheme::DisplayScheme, utils::GraphicConsole};

        static GRAPHIC_CONSOLE: InitOnce<Mutex<GraphicConsole>> = InitOnce::new();

        #[allow(dead_code)]
        pub(crate) fn init_graphic_console(display: Arc<dyn DisplayScheme>) {
            GRAPHIC_CONSOLE.init_once_by(Mutex::new(GraphicConsole::new(display)));
        }
    }
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SerialWriter.write_fmt(fmt).unwrap();
}

/// Print format string and its arguments to serial.
pub fn serial_write(s: &str) {
    SerialWriter.write_str(s).unwrap();
}

/// Print format string and its arguments to graphic console.
#[allow(unused_variables)]
pub fn graphic_console_write_fmt(fmt: Arguments) {
    #[cfg(feature = "graphic")]
    if let Some(cons) = GRAPHIC_CONSOLE.try_get() {
        cons.lock().write_fmt(fmt).unwrap();
    }
}

/// Print format string and its arguments to graphic console.
#[allow(unused_variables)]
pub fn graphic_console_write(s: &str) {
    #[cfg(feature = "graphic")]
    if let Some(cons) = GRAPHIC_CONSOLE.try_get() {
        cons.lock().write_str(s).unwrap();
    }
}

/// Print format string and its arguments to serial and graphic console (if exists).
pub fn console_write_fmt(fmt: Arguments) {
    serial_write_fmt(fmt);
    graphic_console_write_fmt(fmt);
}

/// Print a string to serial and graphic console (if exists).
pub fn console_write(s: &str) {
    serial_write(s);
    graphic_console_write(s);
}

/// Read buffer data from console (serial).
pub async fn console_read(buf: &mut [u8]) -> usize {
    super::future::SerialReadFuture::new(buf).await
}

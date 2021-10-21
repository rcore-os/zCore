use crate::drivers;
use core::fmt::{Arguments, Result, Write};
use spin::Mutex;

struct SerialWriter;

static SERIAL_WRITER: Mutex<SerialWriter> = Mutex::new(SerialWriter);

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = drivers::all_uart().first() {
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
        use zcore_drivers::{scheme::DisplayScheme, utils::GraphicConsole};

        static GRAPHIC_CONSOLE: InitOnce<Mutex<GraphicConsole>> = InitOnce::new();
        static CONSOLE_WIN_SIZE: InitOnce<ConsoleWinSize> = InitOnce::new();

        pub(crate) fn init_graphic_console(display: Arc<dyn DisplayScheme>) {
            let info = display.info();
            let cons = GraphicConsole::new(display);
            let winsz = ConsoleWinSize {
                ws_row: cons.rows() as u16,
                ws_col: cons.columns() as u16,
                ws_xpixel: info.width as u16,
                ws_ypixel: info.height as u16,
            };
            CONSOLE_WIN_SIZE.init_once_by(winsz);
            GRAPHIC_CONSOLE.init_once_by(Mutex::new(cons));
        }
    }
}

/// Print format string and its arguments to serial.
pub fn serial_write_fmt(fmt: Arguments) {
    SERIAL_WRITER.lock().write_fmt(fmt).unwrap();
}

/// Print format string and its arguments to serial.
pub fn serial_write(s: &str) {
    SERIAL_WRITER.lock().write_str(s).unwrap();
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

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ConsoleWinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// Returns the size information of the console, see [`ConsoleWinSize`].
pub fn console_win_size() -> ConsoleWinSize {
    #[cfg(feature = "graphic")]
    if let Some(&winsz) = CONSOLE_WIN_SIZE.try_get() {
        return winsz;
    }
    ConsoleWinSize::default()
}

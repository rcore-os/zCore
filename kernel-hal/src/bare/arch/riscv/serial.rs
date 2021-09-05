use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::fmt::{Arguments, Result, Write};

use spin::Mutex;

use super::{sbi, uart};

lazy_static::lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

struct SbiConsole;
struct UartConsole;

impl Write for SbiConsole {
    fn write_str(&mut self, s: &str) -> Result {
        for ch in s.chars() {
            sbi::console_putchar(ch as u8 as usize);
        }
        Ok(())
    }
}

impl Write for UartConsole {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(ref mut uart) = uart::UART.lock().get_mut() {
            //每次都创建一个新的Uart ? 内存位置始终相同
            write!(uart, "{}", s)
        } else {
            SbiConsole.write_str(s)
        }
    }
}

pub(super) fn sbi_print_fmt(fmt: Arguments) {
    SbiConsole.write_fmt(fmt).unwrap();
}

pub(super) fn uart_print_fmt(fmt: Arguments) {
    UartConsole.write_fmt(fmt).unwrap();
}

hal_fn_impl! {
    impl mod crate::defs::serial {
        fn serial_put(x: u8) {
            if (x == b'\r') || (x == b'\n') {
                STDIN.lock().push_back(b'\n');
                STDIN.lock().push_back(b'\r');
            }else{
                STDIN.lock().push_back(x);
            }
            STDIN_CALLBACK.lock().retain(|f| !f());
        }

        fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
            STDIN_CALLBACK.lock().push(callback);
        }

        fn serial_read(buf: &mut [u8]) -> usize {
            let mut stdin = STDIN.lock();
            let len = stdin.len().min(buf.len());
            for c in &mut buf[..len] {
                *c = stdin.pop_front().unwrap();
            }
            len
        }

        fn serial_write_fmt(fmt: Arguments) {
            uart_print_fmt(fmt);
        }
    }
}

macro_rules! sbi_print {
	($($arg:tt)*) => ({
        crate::serial::sbi_print_fmt(format_args!($($arg)*));
	});
}

macro_rules! sbi_println {
	() => (sbi_print!("\n"));
	($($arg:tt)*) => (sbi_print!("{}\n", format_args!($($arg)*)));
}

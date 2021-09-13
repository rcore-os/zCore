use core::fmt::{Arguments, Result, Write};

use super::{sbi, uart};

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
        if let Some(uart) = uart::UART.try_get() {
            //每次都创建一个新的Uart ? 内存位置始终相同
            write!(uart.lock(), "{}", s)
        } else {
            SbiConsole.write_str(s)
        }
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::serial {
        fn serial_write_fmt(fmt: Arguments) {
            UartConsole.write_fmt(fmt).unwrap();
        }
    }
}

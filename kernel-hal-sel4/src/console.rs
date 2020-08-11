use crate::sys;
use core::fmt::{self, Write};

struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut buffer = [0u8; 4];
        sys::locked(|| {
            for c in s.chars() {
                for code_point in c.encode_utf8(&mut buffer).as_bytes().iter() {
                    unsafe {
                        sys::l4bridge_putchar(*code_point);
                    }
                }
            }
        });
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    Console.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?));
    }
}

#[macro_export]
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    }
}
use core::fmt::{Arguments, Result, Write};

use spin::Mutex;
use zcore_drivers::scheme::{Scheme, UartScheme};
use zcore_drivers::{io::Mmio, uart::Uart16550};

use crate::{mem::phys_to_virt, utils::init_once::InitOnce};

pub(super) static UART: InitOnce<Mutex<&'static mut Uart16550<Mmio<u8>>>> = InitOnce::new();

pub(super) fn init() {
    UART.init(|| {
        let uart = unsafe { Uart16550::<Mmio<u8>>::new(phys_to_virt(super::consts::UART_BASE)) };
        uart.init().unwrap();
        Mutex::new(uart)
    });
}

pub(super) fn handle_irq() {
    if let Some(uart) = UART.try_get() {
        if let Some(c) = uart.lock().try_recv().unwrap() {
            crate::serial::serial_put(c);
        }
    }
}

struct SbiWriter;

impl Write for SbiWriter {
    fn write_str(&mut self, s: &str) -> Result {
        for ch in s.chars() {
            super::sbi::console_putchar(ch as usize);
        }
        Ok(())
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::serial {
        fn serial_write_fmt(fmt: Arguments) {
            if let Some(uart) = UART.try_get() {
                uart.lock().write_fmt(fmt).unwrap();
            } else {
                SbiWriter.write_fmt(fmt).unwrap();
            }
        }
    }
}

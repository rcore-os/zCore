//! PL011 UART.
use tock_registers::interfaces::{Readable, Writeable};
use tock_registers::register_structs;
use tock_registers::registers::{ReadOnly, ReadWrite};
use crate::imp::config::UART_ADDR;
use zcore_drivers::scheme::{UartScheme, EventScheme, Scheme};
use zcore_drivers::DeviceResult;
use zcore_drivers::utils::EventHandler;

register_structs! {
    Pl011UartRegs {
        /// Data Register.
        (0x00 => dr: ReadWrite<u32>),
        (0x04 => _reserved0),
        /// Flag Register.
        (0x18 => fr: ReadOnly<u32>),
        (0x1c => @END),
    }
}



pub struct Pl011Uart {
    base_vaddr: usize,
}

impl Pl011Uart {
    pub fn new(base_vaddr: usize) -> Self {
        Self { base_vaddr }
    }

    const fn regs(&self) -> &Pl011UartRegs {
        unsafe { &*(self.base_vaddr as *const _) }
    }

    fn putchar(&self, c: u8) {
        while self.regs().fr.get() & (1 << 5) != 0 {}
        self.regs().dr.set(c as u32);
    }

    fn getchar(&self) -> Option<u8> {
        if self.regs().fr.get() & (1 << 4) == 0 {
            Some(self.regs().dr.get() as u8)
        } else {
            None
        }
    }
}

pub fn console_putchar(c: u8) {
    let uart = Pl011Uart::new(UART_ADDR);
    uart.putchar(c);
}

pub fn console_getchar() -> Option<u8> {
    let uart = Pl011Uart::new(UART_ADDR);
    uart.getchar()
}

impl Scheme for Pl011Uart {
    fn name(&self) -> &str {
        "Pl011 ARM series uart"
    }

    fn handle_irq(&self, _irq_num: usize) {
        unimplemented!()
    }
}

impl EventScheme for Pl011Uart {
    type Event = ();

    fn trigger(&self, _event: Self::Event) {

    }

    fn subscribe(&self, _handler: EventHandler<Self::Event>, _once: bool) {

    }
}

impl UartScheme for Pl011Uart {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        Ok(self.getchar())
    }

    fn send(&self, ch: u8) -> DeviceResult {
        Ok(self.putchar(ch))
    }

    fn write_str(&self, s: &str) -> DeviceResult {
        for c in s.bytes() {
            self.send(c)?;
        }
        Ok(())
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::console {
        fn console_write_early(s: &str) {
            for c in s.bytes() {
                console_putchar(c as u8);
            }
        }
    }
}
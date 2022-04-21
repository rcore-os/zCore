//! PL011 UART.
use crate::scheme::{impl_event_scheme, Scheme, UartScheme};
use crate::utils::EventListener;
use crate::DeviceResult;
use tock_registers::interfaces::{Readable, Writeable};
use tock_registers::register_structs;
use tock_registers::registers::{ReadOnly, ReadWrite, WriteOnly};

register_structs! {
    Pl011UartRegs {
        /// Data Register.
        (0x00 => dr: ReadWrite<u32>),
        (0x04 => _reserved0),
        /// Flag Register.
        (0x18 => fr: ReadOnly<u32>),
        /// Interrupt control register.
        (0x2c => lcr: ReadWrite<u32>),
        (0x38 => imsr: WriteOnly<u32>),
        (0x44 => icr: WriteOnly<u32>),
        (0x5c => @END),
    }
}

pub struct Pl011Uart {
    base_vaddr: usize,
    listener: EventListener,
}

impl Pl011Uart {
    pub fn new(base_vaddr: usize) -> Self {
        Self {
            base_vaddr,
            listener: EventListener::new(),
        }
    }

    pub fn init(&self) {
        // Disable FIFOs (use character mode instead)
        let mut lcr = self.regs().lcr.get() as u32;
        lcr = lcr & !(1 << 4);
        self.regs().lcr.set(lcr);
        // Enable IRQs
        self.regs().imsr.set(1 << 4);
        // Clear pending interrupts
        self.regs().icr.set(0x7ff);
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

impl Scheme for Pl011Uart {
    fn name(&self) -> &str {
        "Pl011 ARM series uart"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.listener.trigger(())
    }
}

impl_event_scheme!(Pl011Uart);

impl UartScheme for Pl011Uart {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        Ok(self.getchar())
    }

    fn send(&self, ch: u8) -> DeviceResult {
        self.putchar(ch);
        Ok(())
    }

    fn write_str(&self, s: &str) -> DeviceResult {
        for c in s.bytes() {
            self.send(c)?;
        }
        Ok(())
    }
}

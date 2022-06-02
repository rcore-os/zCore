//! PL011 UART.
use crate::scheme::{impl_event_scheme, Scheme, UartScheme};
use crate::utils::EventListener;
use crate::DeviceResult;
use bitflags::*;
use core::ptr;

bitflags! {
    /// UARTFR
    struct UartFrFlags: u16 {
        const TXFE = 1 << 7;
        const RXFF = 1 << 6;
        const TXFF = 1 << 5;
        const RXFE = 1 << 4;
        const BUSY = 1 << 3;
    }
}

bitflags! {
    /// UARTCR
    struct UartCrFlags: u16 {
        const RXE = 1 << 9;
        const TXE = 1 << 8;
        const UARTEN = 1 << 0;
    }
}

bitflags! {
    // UARTIMSC
    struct UartImscFlags: u16 {
        const RTIM = 1 << 6;
        const TXIM = 1 << 5;
        const RXIM = 1 << 4;
    }
}

bitflags! {
    // UARTICR
    struct UartIcrFlags: u16 {
        const RTIC = 1 << 6;
        const TXIC = 1 << 5;
        const RXIC = 1 << 4;
    }
}

bitflags! {
    //UARTMIS
    struct UartMisFlags: u16 {
        const TXMIS = 1 << 5;
        const RXMIS = 1 << 4;
    }
}

bitflags! {
    //UARTLCR_H
    struct UartLcrhFlags: u16 {
        const FEN = 1 << 4;
    }
}

#[allow(dead_code)]
pub struct Pl011Uart {
    inner: Pl011Inner,
    listener: EventListener,
}

impl Pl011Uart {
    pub fn new(base: usize) -> Self {
        Self {
            inner: {
                let inner = Pl011Inner::new(base);
                inner.init();
                inner
            },
            listener: EventListener::new(),
        }
    }

    fn getchar(&self) -> Option<u8> {
        self.inner.getchar()
    }

    fn putchar(&self, data: u8) {
        self.inner.putchar(data);
    }
}

struct Pl011Inner {
    base: usize,
    data_reg: u8,
    flag_reg: u8,
    line_ctrl_reg: u8,
    ctrl_reg: u8,
    intr_mask_setclr_reg: u8,
    intr_clr_reg: u8,
}

impl Pl011Inner {
    pub fn new(base: usize) -> Pl011Inner {
        Pl011Inner {
            base,
            data_reg: 0x00,
            flag_reg: 0x18,
            line_ctrl_reg: 0x2c,
            ctrl_reg: 0x30,
            intr_mask_setclr_reg: 0x38,
            intr_clr_reg: 0x44,
        }
    }

    fn read_reg(&self, register: u8) -> u16 {
        unsafe { ptr::read_volatile((self.base + register as usize) as *mut u16) }
    }

    fn write_reg(&self, register: u8, data: u16) {
        unsafe {
            ptr::write_volatile((self.base + register as usize) as *mut u16, data);
        }
    }

    fn init(&self) {
        // Enable RX, TX, UART
        let flags = UartCrFlags::RXE | UartCrFlags::TXE | UartCrFlags::UARTEN;
        self.write_reg(self.ctrl_reg, flags.bits());

        // Disable FIFOs (use character mode instead)
        let mut flags = UartLcrhFlags::from_bits_truncate(self.read_reg(self.line_ctrl_reg));
        flags.remove(UartLcrhFlags::FEN);
        self.write_reg(self.line_ctrl_reg, flags.bits());

        // Enable IRQs
        let flags = UartImscFlags::RXIM;
        self.write_reg(self.intr_mask_setclr_reg, flags.bits);

        // Clear pending interrupts
        self.write_reg(self.intr_clr_reg, 0x7ff);
    }

    fn line_sts(&self) -> UartFrFlags {
        UartFrFlags::from_bits_truncate(self.read_reg(self.flag_reg))
    }

    fn getchar(&self) -> Option<u8> {
        if self.line_sts().contains(UartFrFlags::RXFF) {
            Some(self.read_reg(self.data_reg) as u8)
        } else {
            None
        }
    }

    fn putchar(&self, data: u8) {
        while !self.line_sts().contains(UartFrFlags::TXFE) {}
        self.write_reg(self.data_reg, data as u16);
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

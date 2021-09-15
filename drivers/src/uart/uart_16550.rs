use core::{convert::TryInto, fmt};

use bitflags::bitflags;

use crate::io::{Io, Mmio, ReadOnly};
use crate::scheme::{Scheme, UartScheme};
use crate::DeviceResult;

bitflags! {
    /// Interrupt enable flags
    struct IntEnFlags: u8 {
        const RECEIVED = 1;
        const SENT = 1 << 1;
        const ERRORED = 1 << 2;
        const STATUS_CHANGE = 1 << 3;
        // 4 to 7 are unused
    }
}

bitflags! {
    /// Line status flags
    struct LineStsFlags: u8 {
        const INPUT_FULL = 1;
        // 1 to 4 unknown
        const OUTPUT_EMPTY = 1 << 5;
        // 6 and 7 unknown
    }
}

#[repr(C)]
pub struct Uart16550<T: Io> {
    /// Data register, read to receive, write to send
    data: T,
    /// Interrupt enable
    int_en: T,
    /// FIFO control
    fifo_ctrl: T,
    /// Line control
    line_ctrl: T,
    /// Modem control
    modem_ctrl: T,
    /// Line status
    line_sts: ReadOnly<T>,
    /// Modem status
    modem_sts: ReadOnly<T>,
}

#[cfg(target_arch = "x86_64")]
impl Uart16550<crate::io::Pio<u8>> {
    pub const fn new(base: u16) -> Self {
        use crate::io::Pio;
        Self {
            data: Pio::new(base),
            int_en: Pio::new(base + 1),
            fifo_ctrl: Pio::new(base + 2),
            line_ctrl: Pio::new(base + 3),
            modem_ctrl: Pio::new(base + 4),
            line_sts: ReadOnly::new(Pio::new(base + 5)),
            modem_sts: ReadOnly::new(Pio::new(base + 6)),
        }
    }
}

impl Uart16550<Mmio<u8>> {
    pub unsafe fn new(base: usize) -> &'static mut Self {
        Mmio::<u8>::from_base(base)
    }
}

impl Uart16550<Mmio<u32>> {
    pub unsafe fn new(base: usize) -> &'static mut Self {
        Mmio::<u32>::from_base(base)
    }
}

impl<T: Io> Uart16550<T>
where
    T::Value: From<u8> + TryInto<u8>,
{
    fn line_sts(&self) -> LineStsFlags {
        LineStsFlags::from_bits_truncate(
            (self.line_sts.read() & 0xFF.into()).try_into().unwrap_or(0),
        )
    }
}

impl<T: Io> Scheme for Uart16550<T>
where
    T::Value: From<u8> + TryInto<u8>,
{
    fn init(&mut self) -> DeviceResult {
        // Disable interrupts
        self.int_en.write(0x00.into());

        // Enable DLAB
        self.line_ctrl.write(0x80.into());

        // Set maximum speed to 38400 bps by configuring DLL and DLM
        self.data.write(0x03.into());
        self.int_en.write(0x00.into());

        // Disable DLAB and set data word length to 8 bits
        self.line_ctrl.write(0x03.into());

        // Enable FIFO, clear TX/RX queues and
        // set interrupt watermark at 14 bytes
        self.fifo_ctrl.write(0xC7.into());

        // Mark data terminal ready, signal request to send
        // and enable auxilliary output #2 (used as interrupt line for CPU)
        self.modem_ctrl.write(0x0B.into());

        // Enable interrupts
        self.int_en.write(0x01.into());

        Ok(())
    }
}

impl<T: Io> UartScheme for Uart16550<T>
where
    T::Value: From<u8> + TryInto<u8>,
{
    fn try_recv(&mut self) -> DeviceResult<Option<u8>> {
        if self.line_sts().contains(LineStsFlags::INPUT_FULL) {
            Ok(Some(
                (self.data.read() & 0xFF.into()).try_into().unwrap_or(0),
            ))
        } else {
            Ok(None)
        }
    }

    fn send(&mut self, ch: u8) -> DeviceResult {
        while !self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY) {}
        self.data.write(ch.into());
        Ok(())
    }
}

impl<T: Io> fmt::Write for Uart16550<T>
where
    T::Value: From<u8> + TryInto<u8>,
{
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            match b {
                8 | 0x7F => {
                    self.send(8).unwrap();
                    self.send(b' ').unwrap();
                    self.send(8).unwrap();
                }
                b'\n' => {
                    self.send(b'\r').unwrap();
                    self.send(b'\n').unwrap();
                }
                _ => {
                    self.send(b).unwrap();
                }
            }
        }
        Ok(())
    }
}

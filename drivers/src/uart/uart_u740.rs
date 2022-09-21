use bitflags::bitflags;
use core::convert::TryInto;
use core::ops::{BitAnd, BitOr, Not};
use lock::Mutex;

use crate::io::{Io, Mmio, ReadOnly};
use crate::scheme::{impl_event_scheme, Scheme, UartScheme};
use crate::utils::EventListener;
use crate::DeviceResult;

bitflags! {
    /// TXDATA fields
    struct TXDATAFlags: u32 {
        const TXFULL = 1 << 31;
    }
}

bitflags! {
    /// RXDATA fields
    struct RXDATAFlags: u32 {
        const RXEMPTY = 1 << 31;
    }
}

bitflags! {
    /// TXCTRL fields
    struct TXCTRLFlags: u32 {
        const TXEN = 1;
        const NSTOP = 1 << 1;
    }
}

bitflags! {
    /// RXCTRL fields
    struct RXCTRLFlags: u32 {
        const RXEN = 1;
    }
}

bitflags! {
    /// IE fields
    struct IEFlags: u32 {
        const TXWM = 1;
        const RXWM = 1 << 1;
    }
}

#[repr(C)]
struct UartU740Inner<T: Io> {
    /// Transmit data register
    tx_data: T,
    /// Receive data register
    rx_data: ReadOnly<T>,
    /// Transmit control register
    tx_ctrl: T,
    /// Receive control register
    rx_ctrl: T,
    /// UART interrupt enable
    ie: T,
    /// UART interrupt pending
    ip: ReadOnly<T>,
    /// Baud rate divisor
    div: T,
}

impl<T: Io> UartU740Inner<T>
where
    T::Value: From<u8> + TryInto<u8> + From<u32> + TryInto<u32>,
{
    fn init(&mut self) {
        // Enable transmit and set 1 stop bit and interrupt watermark
        self.tx_ctrl
            .write((self.tx_ctrl.read().try_into().unwrap_or(0) | TXCTRLFlags::TXEN.bits()).into());

        // Enable receive and set interrupt watermark
        self.rx_ctrl
            .write((self.rx_ctrl.read().try_into().unwrap_or(0) | RXCTRLFlags::RXEN.bits()).into());

        // Enable TX & RX interrupt
        self.ie.write(
            (self.ie.read().try_into().unwrap_or(0) | IEFlags::TXWM.bits() | IEFlags::RXWM.bits())
                .into(),
        );
    }

    fn try_recv(&mut self) -> DeviceResult<Option<u8>> {
        let ch = self.rx_data.read();
        if RXDATAFlags::from_bits_truncate(ch.try_into().unwrap_or(0))
            .contains(RXDATAFlags::RXEMPTY)
        {
            Ok(None)
        } else {
            Ok(Some(ch.try_into().unwrap_or(0) & 0xFF))
        }
    }

    fn send(&mut self, ch: u8) -> DeviceResult {
        let mut status;
        loop {
            status = self.tx_data.read();
            if !TXDATAFlags::from_bits_truncate(status.try_into().unwrap_or(0))
                .contains(TXDATAFlags::TXFULL)
            {
                break;
            }
        }
        self.tx_data.write(ch.into());
        Ok(())
    }

    fn write_str(&mut self, s: &str) -> DeviceResult {
        for b in s.bytes() {
            match b {
                b'\n' => {
                    self.send(b'\r')?;
                    self.send(b'\n')?;
                }
                _ => {
                    self.send(b)?;
                }
            }
        }
        Ok(())
    }
}

/// MMIO driver for UART 16550
pub struct UartU740Mmio<V: 'static>
where
    V: Copy + BitAnd<Output = V> + BitOr<Output = V> + Not<Output = V>,
{
    inner: Mutex<&'static mut UartU740Inner<Mmio<V>>>,
    listener: EventListener,
}

impl_event_scheme!(UartU740Mmio<V>
where
    V: Copy
        + BitAnd<Output = V>
        + BitOr<Output = V>
        + Not<Output = V>
        + From<u8>
        + TryInto<u8>
        + Send
);

impl<V> Scheme for UartU740Mmio<V>
where
    V: Copy + BitAnd<Output = V> + BitOr<Output = V> + Not<Output = V> + Send,
{
    fn name(&self) -> &str {
        "uart-u740-mmio"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.listener.trigger(());
    }
}

impl<V> UartScheme for UartU740Mmio<V>
where
    V: Copy
        + BitAnd<Output = V>
        + BitOr<Output = V>
        + Not<Output = V>
        + From<u8>
        + TryInto<u8>
        + From<u32>
        + TryInto<u32>
        + Send,
{
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        self.inner.lock().try_recv()
    }

    fn send(&self, ch: u8) -> DeviceResult {
        self.inner.lock().send(ch)
    }

    fn write_str(&self, s: &str) -> DeviceResult {
        self.inner.lock().write_str(s)
    }
}

impl<V> UartU740Mmio<V>
where
    V: Copy
        + BitAnd<Output = V>
        + BitOr<Output = V>
        + Not<Output = V>
        + From<u8>
        + TryInto<u8>
        + From<u32>
        + TryInto<u32>
        + Send,
{
    unsafe fn new_common(base: usize) -> Self {
        let uart: &mut UartU740Inner<Mmio<V>> = Mmio::<V>::from_base_as(base);
        uart.init();
        Self {
            inner: Mutex::new(uart),
            listener: EventListener::new(),
        }
    }
}

impl UartU740Mmio<u32> {
    /// # Safety
    ///
    /// This function is unsafe because `base_addr` may be an arbitrary address.
    pub unsafe fn new(base: usize) -> Self {
        Self::new_common(base)
    }
}

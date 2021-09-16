use core::fmt::{Result, Write};

use spin::Mutex;
use virtio_drivers::{VirtIOConsole as InnerDriver, VirtIOHeader};

use crate::scheme::{Scheme, UartScheme};
use crate::DeviceResult;

pub struct VirtIoConsole<'a> {
    inner: Mutex<InnerDriver<'a>>,
}

impl<'a> VirtIoConsole<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        Ok(Self {
            inner: Mutex::new(InnerDriver::new(header)?),
        })
    }
}

impl<'a> Scheme for VirtIoConsole<'a> {}

impl<'a> UartScheme for VirtIoConsole<'a> {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        Ok(self.inner.lock().recv(true)?)
    }

    fn send(&self, ch: u8) -> DeviceResult {
        self.inner.lock().send(ch)?;
        Ok(())
    }
}

impl<'a> Write for VirtIoConsole<'a> {
    fn write_str(&mut self, s: &str) -> Result {
        for b in s.bytes() {
            self.send(b).unwrap()
        }
        Ok(())
    }
}

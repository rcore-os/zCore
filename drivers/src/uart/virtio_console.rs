use crate::scheme::{Scheme, UartScheme};
use crate::DeviceResult;

use virtio_drivers::{VirtIOConsole as InnerDriver, VirtIOHeader};

pub struct VirtIoConsole<'a> {
    inner: InnerDriver<'a>,
}

impl<'a> VirtIoConsole<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        Ok(Self {
            inner: InnerDriver::new(header)?,
        })
    }
}

impl<'a> Scheme for VirtIoConsole<'a> {}

impl<'a> UartScheme for VirtIoConsole<'a> {
    fn try_recv(&mut self) -> DeviceResult<Option<u8>> {
        Ok(self.inner.recv(true)?)
    }

    fn send(&mut self, ch: u8) -> DeviceResult {
        self.inner.send(ch)?;
        Ok(())
    }
}

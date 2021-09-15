use crate::scheme::{BlockScheme, Scheme};
use crate::DeviceResult;

use virtio_drivers::{VirtIOBlk as InnerDriver, VirtIOHeader};

pub struct VirtIoBlk<'a> {
    inner: InnerDriver<'a>,
}

impl<'a> VirtIoBlk<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        Ok(Self {
            inner: InnerDriver::new(header)?,
        })
    }
}

impl<'a> Scheme for VirtIoBlk<'a> {}

impl<'a> BlockScheme for VirtIoBlk<'a> {
    fn read_block(&mut self, block_id: usize, buf: &mut [u8]) -> DeviceResult {
        self.inner.read_block(block_id, buf)?;
        Ok(())
    }

    fn write_block(&mut self, block_id: usize, buf: &[u8]) -> DeviceResult {
        self.inner.write_block(block_id, buf)?;
        Ok(())
    }

    fn flush(&mut self) -> DeviceResult {
        Ok(())
    }
}

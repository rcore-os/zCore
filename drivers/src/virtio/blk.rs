use lock::Mutex;
use virtio_drivers::{VirtIOBlk as InnerDriver, VirtIOHeader};

use crate::scheme::{BlockScheme, Scheme};
use crate::DeviceResult;

pub struct VirtIoBlk<'a> {
    inner: Mutex<InnerDriver<'a>>,
    capacity: usize,
}

impl<'a> VirtIoBlk<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        let capacity = unsafe {
            let config = &*(header.config_space() as *const u64);
            *config as usize
        };
        Ok(Self {
            inner: Mutex::new(InnerDriver::new(header)?),
            capacity,
        })
    }
}

impl<'a> Scheme for VirtIoBlk<'a> {
    fn name(&self) -> &str {
        "virtio-blk"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.inner.lock().ack_interrupt();
    }
}

/// Sector size assumed by the underlying `virtio-drivers` crate, whose
/// `VirtIOBlk::{read,write}_block` hard-assert `buf.len() == 512` and only
/// ever transfer one sector per call.
const SECTOR_SIZE: usize = 512;

impl<'a> BlockScheme for VirtIoBlk<'a> {
    // `block_id` indexes 512-byte sectors; `buf.len()` may be any multiple of
    // 512 (the `BlockScheme` contract documented on the trait, and what the
    // cache/read-ahead layer in `linux-object`'s `BlockByteDevice` relies on
    // for multi-sector transfers). The vendored virtio-drivers driver only
    // ever moves exactly one sector per call and `assert!`s on anything else,
    // so a multi-sector request has to be split here rather than forwarded
    // whole — forwarding it as-is panics the kernel the first time a caller
    // asks for more than 512 bytes (e.g. `CachedDevice`'s 64 KiB read-ahead).
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DeviceResult {
        if buf.len() == SECTOR_SIZE {
            return self.inner.lock().read_block(block_id, buf).map_err(Into::into);
        }
        if buf.is_empty() || !buf.len().is_multiple_of(SECTOR_SIZE) {
            return Err(crate::DeviceError::InvalidParam);
        }
        let mut inner = self.inner.lock();
        for (i, chunk) in buf.chunks_mut(SECTOR_SIZE).enumerate() {
            inner.read_block(block_id + i, chunk)?;
        }
        Ok(())
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> DeviceResult {
        if buf.len() == SECTOR_SIZE {
            return self.inner.lock().write_block(block_id, buf).map_err(Into::into);
        }
        if buf.is_empty() || !buf.len().is_multiple_of(SECTOR_SIZE) {
            return Err(crate::DeviceError::InvalidParam);
        }
        let mut inner = self.inner.lock();
        for (i, chunk) in buf.chunks(SECTOR_SIZE).enumerate() {
            inner.write_block(block_id + i, chunk)?;
        }
        Ok(())
    }

    fn flush(&self) -> DeviceResult {
        Ok(())
    }

    fn block_count(&self) -> usize {
        self.capacity
    }
}

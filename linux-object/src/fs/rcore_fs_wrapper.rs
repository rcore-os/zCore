//! Device wrappers that implement `rcore_fs::dev::Device`, which can loaded
//! file systems on (e.g. `rcore_fs_sfs::SimpleFileSystem::open()`).

use alloc::sync::Arc;

extern crate rcore_fs;

use kernel_hal::drivers::scheme::BlockScheme;
use lock::RwLock;
use rcore_fs::dev::{BlockDevice, DevError, Device, Result};

/// A naive LRU cache layer for `BlockDevice`, re-exported from `rcore-fs`.
pub use rcore_fs::dev::block_cache::BlockCache;

/// Memory buffer for device.
pub struct MemBuf(RwLock<&'static mut [u8]>);

impl MemBuf {
    /// create a [`MemBuf`] struct.
    pub fn new(buf: &'static mut [u8]) -> Self {
        MemBuf(RwLock::new(buf))
    }
}

impl Device for MemBuf {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let slice = self.0.read();
        let len = buf.len().min(slice.len() - offset);
        buf[..len].copy_from_slice(&slice[offset..offset + len]);
        Ok(len)
    }
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let mut slice = self.0.write();
        let len = buf.len().min(slice.len() - offset);
        slice[offset..offset + len].copy_from_slice(&buf[..len]);
        Ok(len)
    }
    fn sync(&self) -> Result<()> {
        Ok(())
    }
}

/// Block device implements [`BlockScheme`].
pub struct Block(Arc<dyn BlockScheme>);

impl Block {
    /// create a [`Block`] struct.
    pub fn new(block: Arc<dyn BlockScheme>) -> Self {
        Self(block)
    }
}

impl BlockDevice for Block {
    const BLOCK_SIZE_LOG2: u8 = 9; // 512

    fn read_at(&self, block_id: usize, buf: &mut [u8]) -> Result<()> {
        self.0.read_block(block_id, buf).map_err(|_| DevError)
    }

    fn write_at(&self, block_id: usize, buf: &[u8]) -> Result<()> {
        self.0.write_block(block_id, buf).map_err(|_| DevError)
    }

    fn sync(&self) -> Result<()> {
        self.0.flush().map_err(|_| DevError)
    }
}

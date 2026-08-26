//! Block device abstraction used by the driver and mkfs.

use crate::Result;
use alloc::sync::Arc;

/// Byte-addressable backing store (whole partition or image file).
pub trait BlockDevice: Send + Sync {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()>;
    fn sync(&self) -> Result<()>;
    /// Device size in bytes.
    fn size(&self) -> u64;
}

impl BlockDevice for Arc<dyn BlockDevice> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        (**self).write_at(offset, buf)
    }
    fn sync(&self) -> Result<()> {
        (**self).sync()
    }
    fn size(&self) -> u64 {
        (**self).size()
    }
}

/// `std::fs::File`-backed device for tests and host-side image generation.
#[cfg(feature = "std")]
pub struct FileDevice {
    file: std::sync::Mutex<std::fs::File>,
    size: u64,
}

#[cfg(feature = "std")]
impl FileDevice {
    pub fn open(file: std::fs::File) -> std::io::Result<Self> {
        let size = file.metadata()?.len();
        Ok(Self {
            file: std::sync::Mutex::new(file),
            size,
        })
    }
}

#[cfg(feature = "std")]
impl BlockDevice for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))
            .map_err(|_| crate::Error::Io)?;
        f.read_exact(buf).map_err(|_| crate::Error::Io)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))
            .map_err(|_| crate::Error::Io)?;
        f.write_all(buf).map_err(|_| crate::Error::Io)
    }

    fn sync(&self) -> Result<()> {
        self.file
            .lock()
            .unwrap()
            .sync_data()
            .map_err(|_| crate::Error::Io)
    }

    fn size(&self) -> u64 {
        self.size
    }
}

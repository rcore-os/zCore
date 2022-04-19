use alloc::sync::Arc;
use core::any::Any;
use rcore_fs::vfs::{make_rdev, FileType, FsError, INode, Metadata, PollStatus, Result, Timespec};
use rcore_fs_devfs::DevFS;
use zcore_drivers::{scheme::UartScheme, DeviceError};

/// Uart device.
pub struct UartDev {
    index: usize,
    port: Arc<dyn UartScheme>,
    inode_id: usize,
}

impl UartDev {
    pub fn new(index: usize, port: Arc<dyn UartScheme>) -> Self {
        Self {
            index,
            port,
            inode_id: DevFS::new_inode_id(),
        }
    }
}

impl INode for UartDev {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        info!(
            "uart read_at: offset={:#x} buf_len={:#x}",
            offset,
            buf.len()
        );

        let mut len = 0;
        for b in buf.iter_mut() {
            match self.port.try_recv() {
                Ok(Some(b_)) => {
                    *b = b_;
                    len += 1;
                }
                Ok(None) => break,
                Err(e) => return Err(convert_error(e)),
            }
        }
        Ok(len)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        info!(
            "uart write_at: offset={:#x} buf_len={:#x}",
            offset,
            buf.len()
        );

        for b in buf {
            self.port.send(*b).map_err(convert_error)?;
        }
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            // TOKNOW and TODO
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o600, // owner read & write
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(4, self.index),
        })
    }

    #[allow(unsafe_code)]
    fn io_control(&self, _cmd: u32, _data: usize) -> Result<usize> {
        warn!("uart ioctl unimplemented");
        Ok(0)
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

fn convert_error(e: DeviceError) -> FsError {
    match e {
        DeviceError::NotSupported => FsError::NotSupported,
        DeviceError::NotReady => FsError::Busy,
        DeviceError::InvalidParam => FsError::InvalidParam,
        DeviceError::BufferTooSmall
        | DeviceError::DmaError
        | DeviceError::IoError
        | DeviceError::AlreadyExists
        | DeviceError::NoResources => FsError::DeviceError,
    }
}

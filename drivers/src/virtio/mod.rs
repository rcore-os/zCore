//! Packaging of [`virtio-drivers` library](https://github.com/rcore-os/virtio-drivers).

mod blk;
mod console;
mod gpu;
mod input;

pub use blk::VirtIoBlk;
pub use console::VirtIoConsole;
pub use gpu::VirtIoGpu;
pub use input::VirtIoInput;
pub use virtio_drivers::VirtIOHeader;

use crate::DeviceError;
use core::convert::From;
use virtio_drivers::Error;

impl From<Error> for DeviceError {
    fn from(err: Error) -> Self {
        match err {
            Error::BufferTooSmall => Self::BufferTooSmall,
            Error::NotReady => Self::NotReady,
            Error::InvalidParam => Self::InvalidParam,
            Error::DmaError => Self::DmaError,
            Error::AlreadyUsed => Self::AlreadyExists,
            Error::IoError => Self::IoError,
        }
    }
}

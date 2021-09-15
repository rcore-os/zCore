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
            _ => Self::IoError,
        }
    }
}

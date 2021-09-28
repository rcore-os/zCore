use crate::HalError;
use core::convert::From;
use zcore_drivers::DeviceError;

pub use zcore_drivers::scheme::{IrqHandler, IrqPolarity, IrqTriggerMode};

impl From<DeviceError> for HalError {
    fn from(err: DeviceError) -> Self {
        warn!("{:?}", err);
        Self
    }
}

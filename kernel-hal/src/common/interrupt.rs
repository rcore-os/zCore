use crate::HalError;
use core::convert::From;
use zcore_drivers::DeviceError;

pub use zcore_drivers::scheme::{IrqHandler, IrqPolarity, IrqTriggerMode};

impl From<DeviceError> for HalError {
    fn from(_err: DeviceError) -> Self {
        Self
    }
}

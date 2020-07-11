//! Objects for Virtual Machine Monitor (hypervisor).

mod guest;
mod page_table;

pub use guest::Guest;
pub(crate) use page_table::VmmPageTable;
pub use rvm::TrapKind;

use super::ZxError;
use rvm::RvmError;

impl From<RvmError> for ZxError {
    fn from(e: RvmError) -> Self {
        match e {
            RvmError::Internal => Self::INTERNAL,
            RvmError::NotSupported => Self::NOT_SUPPORTED,
            RvmError::NoMemory => Self::NO_MEMORY,
            RvmError::InvalidParam => Self::INVALID_ARGS,
            RvmError::OutOfRange => Self::OUT_OF_RANGE,
            RvmError::BadState => Self::BAD_STATE,
            RvmError::NotFound => Self::NOT_FOUND,
        }
    }
}

impl From<ZxError> for RvmError {
    fn from(e: ZxError) -> Self {
        match e {
            ZxError::INTERNAL => Self::Internal,
            ZxError::NOT_SUPPORTED => Self::NotSupported,
            ZxError::NO_MEMORY => Self::NoMemory,
            ZxError::INVALID_ARGS => Self::InvalidParam,
            ZxError::OUT_OF_RANGE => Self::OutOfRange,
            ZxError::BAD_STATE => Self::BadState,
            ZxError::NOT_FOUND => Self::NotFound,
            _ => Self::BadState,
        }
    }
}

use num_enum::TryFromPrimitive;
use core::convert::TryFrom;

#[derive(Copy, Clone, Debug, TryFromPrimitive)]
#[repr(i32)]
pub enum KernelError {
    OutOfCap = 1,
    OutOfMemory = 2,
    Retry = 3,
    VmRegionOverlap = 4,
    MisalignedAddress = 5,
    MissingPagingParents = 6,
    RetypeFailed = 7,
    ResumeFailed = 8,
    TcbFailure = 9,
    PriorityFailure = 10,
    IpcIgnored = 11,
    IpcFailure = 12,
    Unknown = 13,
    BadTimerPeriod = 14,
    BadUserAddress = 15,
    InvalidPhysicalAddress = 16,
}

impl KernelError {
    pub fn from_code(other: i32) -> KernelError {
        match KernelError::try_from(other) {
            Ok(x) => x,
            Err(_) => KernelError::Unknown,
        }
    }
}

pub type KernelResult<T> = Result<T, KernelError>;

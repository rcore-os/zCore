#[derive(Copy, Clone, Debug)]
#[repr(i32)]
pub enum KernelError {
    OutOfCap = 1,
    OutOfMemory = 2,
    Retry = 3,
    VmRegionOverlap = 4,
    MisalignedAddress = 5,
    MissingPagingParents = 6,
}

pub type KernelResult<T> = Result<T, KernelError>;

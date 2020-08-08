#[derive(Copy, Clone, Debug)]
#[repr(i32)]
pub enum KernelError {
    OutOfCap = 1,
    OutOfMemory = 2,
    Retry = 3,
}

pub type KernelResult<T> = Result<T, KernelError>;

#[derive(Debug)]
#[repr(i32)]
pub enum KernelError {
    OutOfCap = 1,
    OutOfMemory = 2,
}

pub type KernelResult<T> = Result<T, KernelError>;

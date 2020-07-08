use super::user::*;
use core::slice;

type VecResult<T> = core::result::Result<T, VecError>;

const MAX_LENGTH: usize = 0x1000;

#[repr(C)]
pub struct IoVec<T, P: Policy> {
    ptr: UserPtr<T, P>,
    len: usize,
}

pub type InIoVec<T> = IoVec<T, In>;
pub type OutIoVec<T> = IoVec<T, Out>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VecError {
    PtrErr(Error),
    LengthErr,
}

impl From<Error> for VecError {
    fn from(err: Error) -> VecError {
        VecError::PtrErr(err)
    }
}

impl<T, P: Policy> IoVec<T, P> {
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_user_ptr(&self) -> UserPtr<T, P> {
        unimplemented!()
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    pub fn check(&self) -> VecResult<()> {
        self.ptr.check()?;
        if self.len > MAX_LENGTH {
            return Err(VecError::LengthErr);
        }
        Ok(())
    }

    pub fn as_slice(&self) -> VecResult<&[T]> {
        self.check()?;
        let slice = unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) };
        Ok(slice)
    }
}

impl<T, P: Write> IoVec<T, P> {
    pub fn as_mut_slice(&self) -> VecResult<&mut [T]> {
        self.check()?;
        let slice = unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) };
        Ok(slice)
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }
}

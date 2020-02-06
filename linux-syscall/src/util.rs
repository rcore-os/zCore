pub use kernel_hal::user::*;
use {crate::error::*, alloc::vec::Vec};

impl From<Error> for SysError {
    fn from(e: Error) -> Self {
        match e {
            Error::InvalidUtf8 => SysError::EINVAL,
            Error::InvalidPointer => SysError::EFAULT,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct IoVec<P: Policy> {
    /// Starting address
    base: UserPtr<u8, P>,
    /// Number of bytes to transfer
    len: usize,
}

/// A valid IoVecs request from user
#[derive(Debug)]
pub struct IoVecs<P: Policy> {
    vec: Vec<IoVec<P>>,
}

impl<P: Policy> IoVecs<P> {
    pub fn new(ptr: UserInPtr<IoVec<P>>, count: usize) -> LxResult<Self> {
        Ok(IoVecs {
            vec: ptr.read_array(count)?,
        })
    }

    pub fn total_len(&self) -> usize {
        self.vec.iter().map(|vec| vec.len).sum()
    }
}

impl<P: Read> IoVecs<P> {
    pub fn read_to_vec(&self) -> LxResult<Vec<u8>> {
        let mut buf = Vec::new();
        for vec in self.vec.iter() {
            buf.extend(vec.base.read_array(vec.len)?);
        }
        Ok(buf)
    }
}

impl<P: Write> IoVecs<P> {
    pub fn write_from_buf(&mut self, mut buf: &[u8]) -> LxResult<usize> {
        let buf_len = buf.len();
        for vec in self.vec.iter_mut() {
            let copy_len = vec.len.min(buf.len());
            if copy_len == 0 {
                continue;
            }
            vec.base.write_array(&buf[..copy_len])?;
            buf = &buf[copy_len..];
        }
        Ok(buf_len - buf.len())
    }
}

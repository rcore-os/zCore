use super::*;
use crate::error::SysResult;
//use crate::fs::*;
use crate::util::UserInPtr;
use alloc::vec::Vec;

impl Syscall {
    pub fn sys_writev(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVec>,
        iov_count: usize,
    ) -> SysResult<usize> {
        info!(
            "writev: fd: {}, iov: {:?}, count: {}",
            fd, iov_ptr, iov_count
        );
        let mut buf = Vec::new();
        for vec in iov_ptr.read_array(iov_count)? {
            buf.extend(vec.base.read_array(vec.len)?);
        }
        hal::serial_write(core::str::from_utf8(&buf).unwrap());
        Ok(buf.len())
        //        let proc = self.process();
        //        let file_like = proc.get_file_like(fd)?;
        //        let len = file_like.write(&buf)?;
        //        Ok(len)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct IoVec {
    /// Starting address
    base: UserInPtr<u8>,
    /// Number of bytes to transfer
    len: usize,
}

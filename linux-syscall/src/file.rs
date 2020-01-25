use super::*;
use crate::fs::*;

impl Syscall {
    pub fn sys_read(&self, fd: FileDesc, mut base: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("read: fd: {}, base: {:?}, len: {:#x}", fd, base, len);
        let proc = self.process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; len];
        let len = file_like.read(&mut buf)?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    pub fn sys_write(&self, fd: FileDesc, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("write: fd: {}, base: {:?}, len: {:#x}", fd, base, len);
        let proc = self.process();
        let buf = base.read_array(len)?;
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write(&buf)?;
        Ok(len)
    }

    pub fn sys_pread(
        &self,
        fd: FileDesc,
        mut base: UserOutPtr<u8>,
        len: usize,
        offset: usize,
    ) -> SysResult {
        info!(
            "pread: fd: {}, base: {:?}, len: {}, offset: {}",
            fd, base, len, offset
        );
        let proc = self.process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; len];
        let len = file_like.read_at(offset, &mut buf)?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    pub fn sys_pwrite(
        &self,
        fd: FileDesc,
        base: UserInPtr<u8>,
        len: usize,
        offset: usize,
    ) -> SysResult {
        info!(
            "pwrite: fd: {}, base: {:?}, len: {}, offset: {}",
            fd, base, len, offset
        );
        let proc = self.process();
        let buf = base.read_array(len)?;
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write_at(offset, &buf)?;
        Ok(len)
    }

    pub fn sys_readv(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVec<Out>>,
        iov_count: usize,
    ) -> SysResult {
        info!(
            "readv: fd: {}, iov: {:?}, count: {}",
            fd, iov_ptr, iov_count
        );
        let mut iovs = IoVecs::new(iov_ptr, iov_count)?;
        let proc = self.process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; iovs.total_len()];
        let len = file_like.read(&mut buf)?;
        iovs.write_from_buf(&buf)?;
        Ok(len)
    }

    pub fn sys_writev(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVec<In>>,
        iov_count: usize,
    ) -> SysResult {
        info!(
            "writev: fd: {}, iov: {:?}, count: {}",
            fd, iov_ptr, iov_count
        );
        let iovs = IoVecs::new(iov_ptr, iov_count)?;
        let buf = iovs.read_to_vec()?;
        let proc = self.process();
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write(&buf)?;
        Ok(len)
    }
}

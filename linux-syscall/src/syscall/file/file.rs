//! File operations
//!
//! - read, pread, readv
//! - write, pwrite, writev
//! - lseek
//! - truncate, ftruncate
//! - sendfile, copy_file_range
//! - sync, fsync, fdatasync
//! - ioctl, fcntl
//! - access, faccessat

use super::*;

impl Syscall {
    pub fn sys_read(&self, fd: FileDesc, mut base: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("read: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        let proc = self.lock_linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; len];
        let len = file_like.read(&mut buf)?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    pub fn sys_write(&self, fd: FileDesc, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("write: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        let proc = self.lock_linux_process();
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
        offset: u64,
    ) -> SysResult {
        info!(
            "pread: fd={:?}, base={:?}, len={}, offset={}",
            fd, base, len, offset
        );
        let proc = self.lock_linux_process();
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
        offset: u64,
    ) -> SysResult {
        info!(
            "pwrite: fd={:?}, base={:?}, len={}, offset={}",
            fd, base, len, offset
        );
        let proc = self.lock_linux_process();
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
        info!("readv: fd={:?}, iov={:?}, count={}", fd, iov_ptr, iov_count);
        let mut iovs = IoVecs::new(iov_ptr, iov_count)?;
        let proc = self.lock_linux_process();
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
            "writev: fd={:?}, iov={:?}, count={}",
            fd, iov_ptr, iov_count
        );
        let iovs = IoVecs::new(iov_ptr, iov_count)?;
        let buf = iovs.read_to_vec()?;
        let proc = self.lock_linux_process();
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write(&buf)?;
        Ok(len)
    }

    pub fn sys_lseek(&self, fd: FileDesc, offset: i64, whence: u8) -> SysResult {
        const SEEK_SET: u8 = 0;
        const SEEK_CUR: u8 = 1;
        const SEEK_END: u8 = 2;

        let pos = match whence {
            SEEK_SET => SeekFrom::Start(offset as u64),
            SEEK_END => SeekFrom::End(offset),
            SEEK_CUR => SeekFrom::Current(offset),
            _ => return Err(SysError::EINVAL),
        };
        info!("lseek: fd={:?}, pos={:?}", fd, pos);

        let proc = self.lock_linux_process();
        let file = proc.get_file(fd)?;
        let offset = file.seek(pos)?;
        Ok(offset as usize)
    }

    pub fn sys_truncate(&self, path: UserInPtr<u8>, len: usize) -> SysResult {
        let path = path.read_cstring()?;
        info!("truncate: path={:?}, len={}", path, len);
        let proc = self.lock_linux_process();
        proc.lookup_inode(&path)?.resize(len)?;
        Ok(0)
    }

    pub fn sys_ftruncate(&self, fd: FileDesc, len: usize) -> SysResult {
        info!("ftruncate: fd={:?}, len={}", fd, len);
        let proc = self.lock_linux_process();
        proc.get_file(fd)?.set_len(len as u64)?;
        Ok(0)
    }

    pub fn sys_sendfile(
        &self,
        out_fd: FileDesc,
        in_fd: FileDesc,
        offset_ptr: UserInOutPtr<u64>,
        count: usize,
    ) -> SysResult {
        self.sys_copy_file_range(in_fd, offset_ptr, out_fd, 0.into(), count, 0)
    }

    pub fn sys_copy_file_range(
        &self,
        in_fd: FileDesc,
        mut in_offset: UserInOutPtr<u64>,
        out_fd: FileDesc,
        mut out_offset: UserInOutPtr<u64>,
        count: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "copy_file_range: in={:?}, out={:?}, in_offset={:?}, out_offset={:?}, count={}, flags={}",
            in_fd, out_fd, in_offset, out_offset, count, flags
        );
        let proc = self.lock_linux_process();
        let in_file = proc.get_file(in_fd)?;
        let out_file = proc.get_file(out_fd)?;
        let mut buffer = [0u8; 1024];

        // for in_offset and out_offset
        // null means update file offset
        // non-null means update {in,out}_offset instead

        let mut read_offset = if !in_offset.is_null() {
            in_offset.read()?
        } else {
            in_file.seek(SeekFrom::Current(0))?
        };

        let orig_out_file_offset = out_file.seek(SeekFrom::Current(0))?;
        let write_offset = if !out_offset.is_null() {
            let offset = out_offset.read()?;
            out_file.seek(SeekFrom::Start(offset))?
        } else {
            0
        };

        // read from specified offset and write new offset back
        let mut bytes_read = 0;
        let mut total_written = 0;
        while bytes_read < count {
            let len = buffer.len().min(count - bytes_read);
            let read_len = in_file.read_at(read_offset, &mut buffer[..len])?;
            if read_len == 0 {
                break;
            }
            bytes_read += read_len;
            read_offset += read_len as u64;

            let mut bytes_written = 0;
            let mut rlen = read_len;
            while bytes_written < read_len {
                let write_len = out_file.write(&buffer[bytes_written..(bytes_written + rlen)])?;
                if write_len == 0 {
                    info!(
                        "copy_file_range:END_ERR in={:?}, out={:?}, in_offset={:?}, out_offset={:?}, count={} = bytes_read {}, bytes_written {}, write_len {}",
                        in_fd, out_fd, in_offset, out_offset, count, bytes_read, bytes_written, write_len
                    );
                    return Err(SysError::EBADF);
                }
                bytes_written += write_len;
                rlen -= write_len;
            }
            total_written += bytes_written;
        }

        if !in_offset.is_null() {
            in_offset.write(read_offset)?;
        } else {
            in_file.seek(SeekFrom::Current(bytes_read as i64))?;
        }
        out_offset.write_if_not_null(write_offset + total_written as u64)?;
        if !out_offset.is_null() {
            out_file.seek(SeekFrom::Start(orig_out_file_offset))?;
        }
        Ok(total_written)
    }

    pub fn sys_sync(&self) -> SysResult {
        info!("sync:");
        let proc = self.lock_linux_process();
        proc.root_inode().fs().sync()?;
        Ok(0)
    }

    pub fn sys_fsync(&self, fd: FileDesc) -> SysResult {
        info!("fsync: fd={:?}", fd);
        let proc = self.lock_linux_process();
        proc.get_file(fd)?.sync_all()?;
        Ok(0)
    }

    pub fn sys_fdatasync(&self, fd: FileDesc) -> SysResult {
        info!("fdatasync: fd={:?}", fd);
        let proc = self.lock_linux_process();
        proc.get_file(fd)?.sync_data()?;
        Ok(0)
    }
    pub fn sys_ioctl(
        &self,
        fd: FileDesc,
        request: usize,
        arg1: usize,
        arg2: usize,
        arg3: usize,
    ) -> SysResult {
        info!(
            "ioctl: fd={:?}, request={:#x}, args=[{:#x}, {:#x}, {:#x}]",
            fd, request, arg1, arg2, arg3
        );
        let proc = self.lock_linux_process();
        let file_like = proc.get_file_like(fd)?;
        file_like.ioctl(request, arg1, arg2, arg3)
    }

    pub fn sys_fcntl(&self, fd: FileDesc, cmd: usize, arg: usize) -> SysResult {
        info!("fcntl: fd={:?}, cmd={:x}, arg={}", fd, cmd, arg);
        let proc = self.lock_linux_process();
        let file_like = proc.get_file_like(fd)?;
        file_like.fcntl(cmd, arg)
    }

    pub fn sys_access(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        self.sys_faccessat(FileDesc::CWD, path, mode, 0)
    }

    pub fn sys_faccessat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mode: usize,
        flags: usize,
    ) -> SysResult {
        // TODO: check permissions based on uid/gid
        let path = path.read_cstring()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "faccessat: dirfd={:?}, path={:?}, mode={:#o}, flags={:?}",
            dirfd, path, mode, flags
        );
        let proc = self.lock_linux_process();
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let _inode = proc.lookup_inode_at(dirfd, &path, follow)?;
        Ok(0)
    }
}

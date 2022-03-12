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
use linux_object::time::TimeSpec;

impl Syscall<'_> {
    /// Read from a file descriptor
    ///
    /// read() attempts to read up to count bytes from file descriptor fd into the buffer starting at buf.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `base` - pointer to the buffer to fill with read contents.
    /// * `len` - number of bytes to read.
    ///
    /// # return value
    ///
    /// On success, the number of bytes read is returned(zero indicates end of file).  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub async fn sys_read(&self, fd: FileDesc, mut base: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("read: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; len];
        let len = file_like.read(&mut buf).await?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    /// Write to a file descriptor
    ///
    /// write() writes up to count bytes from the buffer starting at buf to the file referred to by the file descriptor fd.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `base` - pointer to the buffer to write.
    /// * `len` - number of bytes to write.
    ///
    /// # return value
    ///
    /// On success, the number of bytes written is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_write(&self, fd: FileDesc, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("write: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        let proc = self.linux_process();
        let buf = base.read_array(len)?;
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write(&buf)?;
        Ok(len)
    }

    /// Read from a file descriptor at a given offset
    ///
    ///  pread() reads up to count bytes from file descriptor fd at offset offset (from the start of the file) into the buffer starting at buf.  The file offset is not changed.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `base` - pointer to the buffer to read.
    /// * `len` - number of bytes to read.
    /// * `offset` - offset from the start of the file.
    ///
    /// # return value
    ///
    /// On success, the number of bytes read is returned(zero indicates end of file).  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub async fn sys_pread(
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
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; len];
        let len = file_like.read_at(offset, &mut buf).await?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    /// Write to a file descriptor at a given offset
    ///
    /// pwrite() writes up to count bytes from the buffer starting at buf to the file descriptor fd at offset offset.  The file offset is not changed.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `base` - pointer to the buffer to write.
    /// * `len` - number of bytes to write.
    /// * `offset` - offset from the start of the file.
    ///
    /// # return value
    ///
    /// On success, the number of bytes written is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
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
        let proc = self.linux_process();
        let buf = base.read_array(len)?;
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write_at(offset, &buf)?;
        Ok(len)
    }

    /// Read data into multiple buffers
    ///
    /// The readv() system call reads iovcnt buffers from the file associated with the file descriptor fd into the buffers described by iov ("scatter input").
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `iov_ptr` - pointer to the io vecter buffer to read.
    /// * `iov_count` - number of vecter entitiy to read.
    ///
    /// # return value
    ///
    /// On success, the number of bytes read is returned(zero indicates end of file).  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub async fn sys_readv(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecOut>,
        iov_count: usize,
    ) -> SysResult {
        info!("readv: fd={:?}, iov={:?}, count={}", fd, iov_ptr, iov_count);
        let mut iovs = iov_ptr.read_iovecs(iov_count)?;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; iovs.total_len()];
        let len = file_like.read(&mut buf).await?;
        iovs.write_from_buf(&buf)?;
        Ok(len)
    }

    /// Write data into multiple buffers
    ///
    /// The writev() system call writes iovcnt buffers of data described by iov to the file associated with the file descriptor fd ("gather output").
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `iov_ptr` - pointer to the io vecter buffer to write.
    /// * `iov_count` - number of vecter entitiy to write.
    ///
    /// # return value
    ///
    /// On success, the number of bytes written is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_writev(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecIn>,
        iov_count: usize,
    ) -> SysResult {
        info!(
            "writev: fd={:?}, iov={:?}, count={}",
            fd, iov_ptr, iov_count
        );
        let iovs = iov_ptr.read_iovecs(iov_count)?;
        let buf = iovs.read_to_vec()?;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let len = file_like.write(&buf)?;
        Ok(len)
    }

    /// Reposition read/write file offset
    ///
    /// lseek() repositions the file offset of the open file description associated with the file descriptor fd to the argument offset according to the directive *whence*.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `offset` - the bytes of offset.
    /// * `whence` - the type of offset.
    ///     1. SEEK_SET - The file offset is set to offset bytes.
    ///     2. SEEK_CUR - The file offset is set to its current location plus offset bytes.
    ///     3. SEEK_END - The file offset is set to the size of the file plus offset bytes.
    ///
    /// # return value
    ///
    /// On success, returns the resulting offset location as measured in bytes from the beginning of the file.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_lseek(&self, fd: FileDesc, offset: i64, whence: u8) -> SysResult {
        const SEEK_SET: u8 = 0;
        const SEEK_CUR: u8 = 1;
        const SEEK_END: u8 = 2;

        let pos = match whence {
            SEEK_SET => SeekFrom::Start(offset as u64),
            SEEK_END => SeekFrom::End(offset),
            SEEK_CUR => SeekFrom::Current(offset),
            _ => return Err(LxError::EINVAL),
        };
        info!("lseek: fd={:?}, pos={:?}", fd, pos);

        let proc = self.linux_process();
        let file = proc.get_file(fd)?;
        let offset = file.seek(pos)?;
        Ok(offset as usize)
    }

    /// Truncate a file to a specified length
    ///
    /// The truncate() functions cause the regular file named by path to be truncated to a size of precisely length bytes.
    ///
    /// # argument
    ///
    /// * `path` - path to the file.
    /// * `len` - a size of precisely length bytes.
    ///
    /// # return value
    ///
    /// On success,  zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_truncate(&self, path: UserInPtr<u8>, len: usize) -> SysResult {
        let path = path.read_cstring()?;
        info!("truncate: path={:?}, len={}", path, len);
        let proc = self.linux_process();
        proc.lookup_inode(&path)?.resize(len)?;
        Ok(0)
    }

    /// Truncate a file to a specified length
    ///
    /// The ftruncate() functions cause the regular file referenced by fd to be truncated to a size of precisely length bytes.
    ///
    /// # argument
    ///
    /// * `fd` - file descriptor.
    /// * `len` - a size of precisely length bytes.
    ///
    /// # return value
    ///
    /// On success,  zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_ftruncate(&self, fd: FileDesc, len: usize) -> SysResult {
        info!("ftruncate: fd={:?}, len={}", fd, len);
        let proc = self.linux_process();
        proc.get_file(fd)?.set_len(len as u64)?;
        Ok(0)
    }

    /// Transfer data between file descriptors
    ///
    /// sendfile() copies data between one file descriptor and another.
    ///
    /// # argument
    ///
    /// * `out_fd` - should be a descriptor opened for writing.
    /// * `in_fd` - should be a file descriptor opened for reading.
    /// * `offset_ptr` - will be set to the offset of the byte following the last byte that was read.
    /// * `count` - is the number of bytes to copy between the file descriptors.
    ///
    /// # return value
    ///
    /// On success, the number of bytes written to out_fd is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub async fn sys_sendfile(
        &self,
        out_fd: FileDesc,
        in_fd: FileDesc,
        offset_ptr: UserInOutPtr<u64>,
        count: usize,
    ) -> SysResult {
        self.sys_copy_file_range(in_fd, offset_ptr, out_fd, 0.into(), count, 0)
            .await
    }

    /// Copy a range of data from one file to another
    ///
    /// The copy_file_range() system call performs an in-kernel copy
    /// between two file descriptors without the additional cost of
    /// transferring data from the kernel to user space and then back
    /// into the kernel.
    ///
    /// # argument
    ///
    /// * `in_fd` - the source file descriptor.
    /// * `in_offset` - point to a buffer that specifies the starting offset
    /// * `out_fd` - the target file descriptor.
    /// * `out_offset` - point to a buffer that specifies the starting offset
    /// * `count` - is the number of bytes to copy between the file descriptors.
    /// * `flags` - is provided to allow for future extensions and currently must be set to 0.
    ///
    /// # return value
    ///
    /// On success, the number of bytes written to out_fd is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub async fn sys_copy_file_range(
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
        let proc = self.linux_process();
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
            let read_len = in_file.read_at(read_offset, &mut buffer[..len]).await?;
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
                    return Err(LxError::EBADF);
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

    /// Commit filesystem caches to disk
    ///
    /// sync() causes all pending modifications to filesystem metadata
    /// and cached file data to be written to the underlying filesystems.
    ///
    /// # return value
    ///
    /// On success, returns 0.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_sync(&self) -> SysResult {
        info!("sync:");
        let proc = self.linux_process();
        proc.root_inode().fs().sync()?;
        Ok(0)
    }

    /// Synchronize a file's in-core state with storage device
    ///
    /// fsync() transfers ("flushes") all modified in-core data of (i.e.,
    /// modified buffer cache pages for) the file referred to by the file
    /// descriptor fd to the disk device (or other permanent storage
    /// device) so that all changed information can be retrieved even if
    /// the system crashes or is rebooted.
    ///
    ///
    /// # argument
    ///
    /// * `fd` - The specified file descriptor.
    ///
    /// # return value
    ///
    /// On success, returns 0.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_fsync(&self, fd: FileDesc) -> SysResult {
        info!("fsync: fd={:?}", fd);
        let proc = self.linux_process();
        proc.get_file(fd)?.sync_all()?;
        Ok(0)
    }

    /// Synchronize a file's in-core state with storage device
    ///
    /// fdatasync() is similar to fsync(), but does not flush modified
    /// metadata unless that metadata is needed in order to allow a
    /// subsequent data retrieval to be correctly handled.
    ///
    /// # argument
    ///
    /// * `fd` - The specified file descriptor.
    ///
    /// # return value
    ///
    /// On success, returns 0.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_fdatasync(&self, fd: FileDesc) -> SysResult {
        info!("fdatasync: fd={:?}", fd);
        let proc = self.linux_process();
        proc.get_file(fd)?.sync_data()?;
        Ok(0)
    }

    /// Control device
    ///
    /// The ioctl() system call manipulates the underlying device
    /// parameters of special files.
    ///
    /// # argument
    ///
    /// * `fd` -  must be an open file descriptor.
    /// * `request` - is a device-dependent request code.
    ///
    /// # return value
    ///
    /// Usually, on success zero is returned.  A few ioctl() requests use
    /// the return value as an output parameter and return a nonnegative
    /// value on success.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
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
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        file_like.ioctl(request, arg1, arg2, arg3)
    }

    /// Manipulate file descriptor
    ///
    /// fcntl() performs one of the operations described below on the
    /// open file descriptor fd.  The operation is determined by cmd.
    ///
    /// # argument
    ///
    /// * `fd` -  must be an open file descriptor.
    /// * `cmd` - kind of operation.
    /// * `arg` - optional required is determined by cmd.
    ///
    /// # return value
    ///
    /// Usually, on success zero is returned.  A few fcntl() requests use
    /// the return value as an output parameter on success.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_fcntl(&self, fd: FileDesc, cmd: usize, arg: usize) -> SysResult {
        info!("fcntl: fd={:?}, cmd={:x}, arg={}", fd, cmd, arg);
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;

        if let Ok(cmd) = FcntlCmd::try_from(cmd) {
            match cmd {
                FcntlCmd::GETFD => Ok(file_like.flags().close_on_exec() as usize),
                FcntlCmd::SETFD => {
                    let mut flags = file_like.flags();
                    if (arg & 1) != 0 {
                        flags |= OpenFlags::CLOEXEC;
                    } else {
                        flags -= OpenFlags::CLOEXEC;
                    }
                    file_like.set_flags(flags)?;
                    Ok(0)
                }
                FcntlCmd::GETFL => Ok(file_like.flags().bits()),
                FcntlCmd::SETFL => {
                    file_like.set_flags(OpenFlags::from_bits_truncate(arg))?;
                    Ok(0)
                }
                FcntlCmd::DUPFD | FcntlCmd::DUPFD_CLOEXEC => {
                    let new_fd = proc.get_free_fd_from(arg);
                    self.sys_dup2(fd, new_fd)?;
                    let dup = proc.get_file_like(new_fd)?;
                    let mut flags = dup.flags();
                    if cmd == FcntlCmd::DUPFD_CLOEXEC {
                        flags |= OpenFlags::CLOEXEC;
                    } else {
                        flags -= OpenFlags::CLOEXEC;
                    }
                    dup.set_flags(flags)?;
                    Ok(new_fd.into())
                }
                _ => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    /// Check user's permissions for a file
    ///
    /// access() checks whether the calling process can access the file pathname.  If pathname is a symbolic link, it is dereferenced.
    ///
    /// # argument
    ///
    /// * `path` - path to the file.
    /// * `mode` - specifies the accessibility check(s) to be performed.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_access(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        self.sys_faccessat(FileDesc::CWD, path, mode, 0)
    }

    /// Check user's permissions for a file
    /// TODO: check permissions based on uid/gid
    ///
    /// faccessat() operates in exactly the same way as access().
    ///
    /// # argument
    ///
    /// * `dirfd` - specifies a file descriptor as the starting point of the relative path.Ignore this when *path* is an absolute path.
    /// * `path` - path to the file.
    /// * `mode` - specifies the accessibility check(s) to be performed.
    /// * `flags` - is constructed by ORing together zero or more of the following values:AT_EACCESS、AT_SYMLINK_NOFOLLOW.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
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
        let proc = self.linux_process();
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let _inode = proc.lookup_inode_at(dirfd, &path, follow)?;
        Ok(0)
    }

    /// Change file timestamps with nanosecond precision
    ///
    /// utimensat() update the timestamps of a file with nanosecond precision
    ///
    /// # argument
    ///
    /// * `dirfd` - specifies a file descriptor.
    /// * `pathname` - path to the file.
    /// * `times` - The timestamp to be specified.
    /// * `flags` - type of control.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - Invalid value in *flags*.
    ///
    pub fn sys_utimensat(
        &mut self,
        dirfd: FileDesc,
        pathname: UserInPtr<u8>,
        times: UserInOutPtr<[TimeSpec; 2]>,
        flags: usize,
    ) -> SysResult {
        info!(
            "utimensat(raw): dirfd: {:?}, pathname: {:?}, times: {:?}, flags: {:#x}",
            dirfd, pathname, times, flags
        );
        const UTIME_NOW: usize = 0x3fffffff;
        const UTIME_OMIT: usize = 0x3ffffffe;
        let proc = self.linux_process();
        let mut times = if times.is_null() {
            let epoch = TimeSpec::now();
            [epoch, epoch]
        } else {
            let times = times.read()?;
            [times[0], times[1]]
        };
        let inode = if pathname.is_null() {
            let fd = dirfd;
            info!("futimens: fd: {:?}, times: {:?}", fd, times);
            proc.get_file(fd)?.inode()
        } else {
            let pathname = pathname.read_cstring()?;
            info!(
                "utimensat: dirfd: {:?}, pathname: {:?}, times: {:?}, flags: {:#x}",
                dirfd, pathname, times, flags
            );
            let follow = if flags == 0 {
                true
            } else if flags == AtFlags::SYMLINK_NOFOLLOW.bits() {
                false
            } else {
                return Err(LxError::EINVAL);
            };
            proc.lookup_inode_at(dirfd, &pathname[..], follow)?
        };
        let mut metadata = inode.metadata()?;
        if times[0].nsec != UTIME_OMIT {
            if times[0].nsec == UTIME_NOW {
                times[0] = TimeSpec::now();
            }
            metadata.atime = rcore_fs::vfs::Timespec {
                sec: times[0].sec as i64,
                nsec: times[0].nsec as i32,
            };
        }
        if times[1].nsec != UTIME_OMIT {
            if times[1].nsec == UTIME_NOW {
                times[1] = TimeSpec::now();
            }
            metadata.mtime = rcore_fs::vfs::Timespec {
                sec: times[1].sec as i64,
                nsec: times[1].nsec as i32,
            };
        }
        inode.set_metadata(&metadata)?;
        Ok(0)
    }
}

const F_LINUX_SPECIFIC_BASE: usize = 1024;

numeric_enum_macro::numeric_enum! {
    #[repr(usize)]
    #[allow(non_camel_case_types)]
    #[derive(Eq, PartialEq, Debug, Copy, Clone)]
    /// fcntl flags
    pub enum FcntlCmd {
        /// dup
        DUPFD = 0,
        /// get close_on_exec
        GETFD = 1,
        /// set/clear close_on_exec
        SETFD = 2,
        /// get file->f_flags
        GETFL = 3,
        /// set file->f_flags
        SETFL = 4,
        /// Get record locking info.
        GETLK = 5,
        /// Set record locking info (non-blocking).
        SETLK = 6,
        /// Set record locking info (blocking).
        SETLKW = 7,
        /// like F_DUPFD, but additionally set the close-on-exec flag
        DUPFD_CLOEXEC = F_LINUX_SPECIFIC_BASE + 6,
    }
}

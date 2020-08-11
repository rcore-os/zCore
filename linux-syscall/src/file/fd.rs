//! File descriptor operations
//!
//! - open(at)
//! - close
//! - dup2
//! - pipe

use super::*;
use alloc::string::String;

impl Syscall<'_> {
    /// Opens or creates a file, depending on the flags passed to the call. Returns an integer with the file descriptor.
    pub fn sys_open(&self, path: UserInPtr<u8>, flags: usize, mode: usize) -> SysResult {
        self.sys_openat(FileDesc::CWD, path, flags, mode)
    }

    /// open file relative to directory file descriptor
    pub fn sys_openat(
        &self,
        dir_fd: FileDesc,
        path: UserInPtr<u8>,
        flags: usize,
        mode: usize,
    ) -> SysResult {
        let proc = self.linux_process();
        let path = path.read_cstring()?;
        let flags = OpenFlags::from_bits_truncate(flags);
        info!(
            "openat: dir_fd={:?}, path={:?}, flags={:?}, mode={:#o}",
            dir_fd, path, flags, mode
        );

        let inode = if flags.contains(OpenFlags::CREATE) {
            let (dir_path, file_name) = split_path(&path);
            // relative to cwd
            let dir_inode = proc.lookup_inode_at(dir_fd, dir_path, true)?;
            match dir_inode.find(file_name) {
                Ok(file_inode) => {
                    if flags.contains(OpenFlags::EXCLUSIVE) {
                        return Err(LxError::EEXIST);
                    }
                    file_inode
                }
                Err(FsError::EntryNotFound) => {
                    dir_inode.create(file_name, FileType::File, mode as u32)?
                }
                Err(e) => return Err(LxError::from(e)),
            }
        } else {
            proc.lookup_inode_at(dir_fd, &path, true)?
        };

        let file = File::new(inode, flags.to_options(), path);
        let fd = proc.add_file(file)?;
        Ok(fd.into())
    }

    /// Closes a file descriptor, so that it no longer refers to any file and may be reused.
    pub fn sys_close(&self, fd: FileDesc) -> SysResult {
        info!("close: fd={:?}", fd);
        let proc = self.linux_process();
        proc.close_file(fd)?;
        Ok(0)
    }

    /// create a copy of the file descriptor oldfd.
    pub fn sys_dup2(&self, fd1: FileDesc, fd2: FileDesc) -> SysResult {
        info!("dup2: from {:?} to {:?}", fd1, fd2);
        let proc = self.linux_process();
        // close fd2 first if it is opened
        let _ = proc.close_file(fd2);
        let file_like = proc.get_file_like(fd1)?;
        proc.add_file_at(fd2, file_like);
        Ok(fd2.into())
    }

    /// Creates a pipe, a unidirectional data channel that can be used for interprocess communication.
    pub fn sys_pipe(&self, mut fds: UserOutPtr<[i32; 2]>) -> SysResult {
        info!("pipe: fds={:?}", fds);

        let proc = self.linux_process();
        let (read, write) = Pipe::create_pair();

        let read_fd = proc.add_file(File::new(
            Arc::new(read),
            OpenOptions {
                read: true,
                write: false,
                append: false,
                nonblock: false,
            },
            String::from("pipe_r:[]"),
        ))?;

        let write_fd = proc.add_file(File::new(
            Arc::new(write),
            OpenOptions {
                read: false,
                write: true,
                append: false,
                nonblock: false,
            },
            String::from("pipe_w:[]"),
        ))?;
        fds.write([read_fd.into(), write_fd.into()])?;

        info!(
            "pipe: created rfd={:?} wfd={:?} fds={:?}",
            read_fd, write_fd, fds
        );

        Ok(0)
    }
}

bitflags! {
    struct OpenFlags: usize {
        /// read only
        const RDONLY = 0;
        /// write only
        const WRONLY = 1;
        /// read write
        const RDWR = 2;
        /// create file if it does not exist
        const CREATE = 1 << 6;
        /// error if CREATE and the file exists
        const EXCLUSIVE = 1 << 7;
        /// truncate file upon open
        const TRUNCATE = 1 << 9;
        /// append on each write
        const APPEND = 1 << 10;
    }
}

impl OpenFlags {
    /// check if the OpenFlags is readable
    fn readable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::RDONLY.bits() || b == Self::RDWR.bits()
    }
    /// check if the OpenFlags is writable
    fn writable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::WRONLY.bits() || b == Self::RDWR.bits()
    }
    /// convert OpenFlags to OpenOptions
    fn to_options(self) -> OpenOptions {
        OpenOptions {
            read: self.readable(),
            write: self.writable(),
            append: self.contains(Self::APPEND),
            nonblock: false,
        }
    }
}

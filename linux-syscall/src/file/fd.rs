//! File descriptor operations
//!
//! - open(at)
//! - close
//! - dup2
//! - pipe

use super::*;
use alloc::string::String;

impl Syscall<'_> {
    /// Open or create a file, depending on the flags passed to the call. Returns an integer with the file descriptor.
    /// - path - specify the file to open or create.
    /// - flags - specify the access mode, creation and state of the file.
    /// - mode - specify the file mode bits to be applied when a new file is created.
    pub fn sys_open(&self, path: UserInPtr<u8>, flags: usize, mode: usize) -> SysResult {
        self.sys_openat(FileDesc::CWD, path, flags, mode)
    }

    /// Open file relative to directory file descriptor.
    /// - dir_fd - the directory where `path` is located.
    /// - path - specify the file to open or create.
    /// - flags - specify the access mode, creation and state of the file.
    /// - mode - specify the file mode bits to be applied when a new file is created.
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

        let file = File::new(inode, flags, path);
        let fd = proc.add_file(file)?;
        Ok(fd.into())
    }

    /// Close a file descriptor, so that it no longer refers to any file and may be reused.
    /// - fd - the file descriptor to be closed.
    pub fn sys_close(&self, fd: FileDesc) -> SysResult {
        info!("close: fd={:?}", fd);
        let proc = self.linux_process();
        proc.close_file(fd)?;
        Ok(0)
    }

    /// Create a copy of the file descriptor `fd1`.
    /// - fd1 - the file descriptor to be copied.
    /// - fd2 - the file descriptor to be allocated.
    pub fn sys_dup2(&self, fd1: FileDesc, fd2: FileDesc) -> SysResult {
        info!("dup2: from {:?} to {:?}", fd1, fd2);
        let proc = self.linux_process();
        // close fd2 first if it is opened
        let _ = proc.close_file(fd2);
        let file_like = proc.get_file_like(fd1)?.dup();
        let fd2 = proc.add_file_at(fd2, file_like)?;
        Ok(fd2.into())
    }

    /// create a copy of the file descriptor `fd1`, and uses the lowest-numbered unused descriptor for the new descriptor.
    /// - fd1 - the file descriptor to be copied.
    pub fn sys_dup(&self, fd1: FileDesc) -> SysResult {
        info!("dup: from {:?}", fd1);
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd1)?.dup();
        let fd2 = proc.add_file(file_like)?;
        Ok(fd2.into())
    }

    /// Create a pipe, a unidirectional data channel that can be used for interprocess communication.
    /// - fds - used to return two file descriptors referring to the ends of the pipe.
    /// fds\[0], read end of the pipe. fds\[1], write end of the pipe.
    pub fn sys_pipe(&self, fds: UserOutPtr<[i32; 2]>) -> SysResult {
        self.sys_pipe2(fds, 0)
    }

    /// Create a pipe, a unidirectional data channel that can be used for interprocess communication.
    /// - fds - used to return two file descriptors referring to the ends of the pipe.
    /// fds\[0], read end of the pipe. fds\[1], write end of the pipe.
    /// - flags - specify creation and state of the pipe.
    pub fn sys_pipe2(&self, mut fds: UserOutPtr<[i32; 2]>, flags: usize) -> SysResult {
        info!("pipe2: fds={:?}, flags: {:#x}", fds, flags);

        let proc = self.linux_process();
        let (read, write) = Pipe::create_pair();

        let base_flags =
            OpenFlags::from_bits_truncate(flags) & (OpenFlags::NON_BLOCK | OpenFlags::CLOEXEC);
        let read_fd = proc.add_file(File::new(
            Arc::new(read),
            base_flags | OpenFlags::RDONLY,
            String::from("pipe_r:[]"),
        ))?;

        let write_fd = proc.add_file(File::new(
            Arc::new(write),
            base_flags | OpenFlags::WRONLY,
            String::from("pipe_w:[]"),
        ))?;
        fds.write([read_fd.into(), write_fd.into()])?;

        info!(
            "pipe2: created rfd={:?} wfd={:?} fds={:?}",
            read_fd, write_fd, fds
        );

        Ok(0)
    }

    /// Apply or remove an advisory lock on an open file.
    ///
    /// TODO: handle operation
    /// - fd - the file descriptor of the open file.
    /// - operation - place or remove a lock.
    pub fn sys_flock(&mut self, fd: FileDesc, operation: usize) -> SysResult {
        bitflags! {
            struct Operation: u8 {
                const LOCK_SH = 1;
                const LOCK_EX = 2;
                const LOCK_NB = 4;
                const LOCK_UN = 8;
            }
        }
        let operation = Operation::from_bits(operation as u8).unwrap();
        info!("flock: fd: {:?}, operation: {:?}", fd, operation);
        let proc = self.linux_process();

        proc.get_file(fd)?;
        Ok(0)
    }
}

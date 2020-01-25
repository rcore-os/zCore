//! Linux Process

use crate::error::*;
use crate::fs::*;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use spin::{Mutex, MutexGuard};
use zircon_object::task::Process;

pub trait ProcessExt {
    fn lock_linux(&self) -> MutexGuard<'_, LinuxProcess>;
}

impl ProcessExt for Process {
    fn lock_linux(&self) -> MutexGuard<'_, LinuxProcess> {
        self.ext()
            .downcast_ref::<Mutex<LinuxProcess>>()
            .unwrap()
            .lock()
    }
}

/// Linux specific process information.
pub struct LinuxProcess {
    /// Current Working Directory
    pub cwd: String,
    pub exec_path: String,
    pub files: BTreeMap<FileDesc, Arc<dyn FileLike>>,
}

impl LinuxProcess {
    /// Create a new process.
    pub fn new() -> Self {
        let stdin = File::new(
            STDIN.clone(),
            OpenOptions {
                read: true,
                write: false,
                append: false,
                nonblock: false,
            },
            String::from("stdin"),
        ) as Arc<dyn FileLike>;
        let stdout = File::new(
            STDOUT.clone(),
            OpenOptions {
                read: false,
                write: true,
                append: false,
                nonblock: false,
            },
            String::from("stdout"),
        ) as Arc<dyn FileLike>;
        let mut files = BTreeMap::new();
        files.insert(0.into(), stdin);
        files.insert(1.into(), stdout.clone());
        files.insert(2.into(), stdout);

        LinuxProcess {
            cwd: String::from(""),
            exec_path: String::new(),
            files,
        }
    }

    /// Add a file-like, return its file descriptor.
    pub fn add_file_like(&mut self, file: Arc<dyn FileLike>) -> FileDesc {
        let fd = self.get_free_fd();
        self.files.insert(fd, file);
        fd
    }

    /// Get the `File` with given `fd`.
    pub fn get_file(&self, fd: FileDesc) -> LxResult<Arc<File>> {
        let file = self
            .get_file_like(fd)?
            .downcast_arc::<File>()
            .map_err(|_| SysError::EBADF)?;
        Ok(file)
    }

    /// Get the `FileLike` with given `fd`.
    pub fn get_file_like(&self, fd: FileDesc) -> LxResult<Arc<dyn FileLike>> {
        self.files.get(&fd).cloned().ok_or(SysError::EBADF)
    }

    /// Close file descriptor `fd`.
    pub fn close_file(&mut self, fd: FileDesc) -> LxResult<()> {
        self.files.remove(&fd).map(|_| ()).ok_or(SysError::EBADF)
    }

    fn get_free_fd(&self) -> FileDesc {
        (0usize..)
            .map(|i| i.into())
            .find(|fd| !self.files.contains_key(fd))
            .unwrap()
    }
}

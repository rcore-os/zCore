//! Linux Process

use crate::error::*;
use crate::fs::*;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::AtomicI32;
use kernel_hal::VirtAddr;
use rcore_fs::vfs::{FileSystem, INode};
use spin::{Mutex, MutexGuard};
use zircon_object::signal::Futex;
use zircon_object::task::{Job, Process};
use zircon_object::ZxResult;

pub trait ProcessExt {
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>>;
    fn lock_linux(&self) -> MutexGuard<'_, LinuxProcess>;
}

impl ProcessExt for Process {
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>> {
        let linux_proc = Mutex::new(LinuxProcess::new(rootfs));
        Process::create_with_ext(job, "root", linux_proc)
    }

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
    files: BTreeMap<FileDesc, Arc<dyn FileLike>>,
    futexes: BTreeMap<VirtAddr, Arc<Futex>>,
    root_inode: Arc<dyn INode>,
    parent: Weak<Process>,
    #[allow(dead_code)]
    children: Vec<Weak<Process>>,
}

impl LinuxProcess {
    /// Create a new process.
    pub fn new(rootfs: Arc<dyn FileSystem>) -> Self {
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
            cwd: String::from("/"),
            exec_path: String::new(),
            files,
            futexes: Default::default(),
            root_inode: create_root_fs(rootfs),
            parent: Weak::default(),
            children: Vec::new(),
        }
    }

    /// Get futex object.
    #[allow(unsafe_code)]
    pub fn get_futex(&mut self, uaddr: VirtAddr) -> Arc<Futex> {
        self.futexes
            .entry(uaddr)
            .or_insert_with(|| {
                // FIXME: check address
                let value = unsafe { &*(uaddr as *const AtomicI32) };
                Futex::new(value)
            })
            .clone()
    }

    /// Add a file to the file descriptor table.
    pub fn add_file(&mut self, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let fd = self.get_free_fd();
        self.files.insert(fd, file);
        Ok(fd)
    }

    /// Add a file to the file descriptor table at given `fd`.
    pub fn add_file_at(&mut self, fd: FileDesc, file: Arc<dyn FileLike>) {
        self.files.insert(fd, file);
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

    /// Get root INode of the process.
    pub fn root_inode(&self) -> &Arc<dyn INode> {
        &self.root_inode
    }

    /// Get parent process.
    pub fn parent(&self) -> Option<Arc<Process>> {
        self.parent.upgrade()
    }
}

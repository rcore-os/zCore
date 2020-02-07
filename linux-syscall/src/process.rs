//! Linux Process

use crate::error::*;
use crate::fs::*;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::mem::drop;
use core::sync::atomic::AtomicI32;
use kernel_hal::VirtAddr;
use rcore_fs::vfs::{FileSystem, INode};
use spin::{Mutex, MutexGuard};
use zircon_object::object::{KernelObject, KoID, Signal};
use zircon_object::signal::Futex;
use zircon_object::task::{Job, Process, Status};
use zircon_object::ZxResult;

pub trait ProcessExt {
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>>;
    fn lock_linux(&self) -> MutexGuard<'_, LinuxProcess>;
    fn vfork_from(parent: &Arc<Self>) -> ZxResult<Arc<Self>>;
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

    /// [Vfork] the process.
    ///
    /// [Vfork]: http://man7.org/linux/man-pages/man2/vfork.2.html
    fn vfork_from(parent: &Arc<Self>) -> ZxResult<Arc<Self>> {
        let mut linux_parent = parent.lock_linux();
        let new_linux_proc = Mutex::new(LinuxProcess {
            cwd: linux_parent.cwd.clone(),
            exec_path: linux_parent.exec_path.clone(),
            files: linux_parent.files.clone(),
            futexes: BTreeMap::new(),
            root_inode: linux_parent.root_inode.clone(),
            parent: Arc::downgrade(parent),
            children: BTreeMap::new(),
        });
        let new_proc = Process::create_with_ext(&parent.job(), "", new_linux_proc)?;
        linux_parent
            .children
            .insert(new_proc.id(), new_proc.clone());

        // notify parent on terminated
        let parent = parent.clone();
        new_proc.add_signal_callback(Box::new(move |signal| {
            if signal.contains(Signal::PROCESS_TERMINATED) {
                parent.signal_set(Signal::SIGCHLD);
            }
            false
        }));
        Ok(new_proc)
    }
}

/// Wait for state changes in a child of the calling process, and obtain information about
/// the child whose state has changed.
///
/// A state change is considered to be:
/// - the child terminated.
/// - the child was stopped by a signal. TODO
/// - the child was resumed by a signal. TODO
pub async fn wait_child(proc: &Arc<Process>, pid: KoID, nonblock: bool) -> LxResult<ExitCode> {
    loop {
        let mut linux_proc = proc.lock_linux();
        let child = linux_proc.children.get(&pid).ok_or(SysError::ECHILD)?;
        if let Status::Exited(code) = child.status() {
            linux_proc.children.remove(&pid);
            return Ok(code as ExitCode);
        }
        if nonblock {
            return Err(SysError::EAGAIN);
        }
        let child: Arc<dyn KernelObject> = child.clone();
        drop(linux_proc);
        child.wait_signal_async(Signal::PROCESS_TERMINATED).await;
    }
}

/// Wait for state changes in a child of the calling process.
pub async fn wait_child_any(proc: &Arc<Process>, nonblock: bool) -> LxResult<(KoID, ExitCode)> {
    loop {
        let mut linux_proc = proc.lock_linux();
        if linux_proc.children.is_empty() {
            return Err(SysError::ECHILD);
        }
        for (&pid, child) in linux_proc.children.iter() {
            if let Status::Exited(code) = child.status() {
                linux_proc.children.remove(&pid);
                return Ok((pid, code as ExitCode));
            }
        }
        drop(linux_proc);
        if nonblock {
            return Err(SysError::EAGAIN);
        }
        let proc: Arc<dyn KernelObject> = proc.clone();
        proc.wait_signal_async(Signal::SIGCHLD).await;
    }
}

/// Linux specific process information.
pub struct LinuxProcess {
    /// Current Working Directory
    pub cwd: String,
    /// Execute path
    pub exec_path: String,
    /// Opened files
    files: BTreeMap<FileDesc, Arc<dyn FileLike>>,
    /// Futexes
    futexes: BTreeMap<VirtAddr, Arc<Futex>>,
    /// The root INode of file system
    root_inode: Arc<dyn INode>,
    /// Parent process
    parent: Weak<Process>,
    /// Child processes
    children: BTreeMap<KoID, Arc<Process>>,
}

pub type ExitCode = i32;

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
            children: BTreeMap::new(),
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

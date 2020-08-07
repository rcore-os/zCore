//! Linux Process

use crate::error::*;
use crate::fs::*;
use alloc::vec::Vec;
use alloc::{
    boxed::Box,
    string::String,
    sync::{Arc, Weak},
};
use core::sync::atomic::AtomicI32;
use hashbrown::HashMap;
use kernel_hal::VirtAddr;
use rcore_fs::vfs::{FileSystem, INode};
use spin::Mutex;
use zircon_object::{
    object::{KernelObject, KoID, Signal},
    signal::Futex,
    task::{Job, Process, Status},
    ZxResult,
};

pub trait ProcessExt {
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>>;
    fn linux(&self) -> &LinuxProcess;
    fn fork_from(parent: &Arc<Self>, vfork: bool) -> ZxResult<Arc<Self>>;
}

impl ProcessExt for Process {
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>> {
        let linux_proc = LinuxProcess::new(rootfs);
        Process::create_with_ext(job, "root", linux_proc)
    }

    fn linux(&self) -> &LinuxProcess {
        self.ext().downcast_ref::<LinuxProcess>().unwrap()
    }

    /// [Fork] the process.
    ///
    /// [Fork]: http://man7.org/linux/man-pages/man2/fork.2.html
    fn fork_from(parent: &Arc<Self>, vfork: bool) -> ZxResult<Arc<Self>> {
        let linux_parent = parent.linux();
        let mut linux_parent_inner = linux_parent.inner.lock();
        let new_linux_proc = LinuxProcess {
            root_inode: linux_parent.root_inode.clone(),
            parent: Arc::downgrade(parent),
            inner: Mutex::new(LinuxProcessInner {
                execute_path: linux_parent_inner.execute_path.clone(),
                current_working_directory: linux_parent_inner.current_working_directory.clone(),
                files: linux_parent_inner.files.clone(),
                ..Default::default()
            }),
        };
        let new_proc = Process::create_with_ext(&parent.job(), "", new_linux_proc)?;
        linux_parent_inner
            .children
            .insert(new_proc.id(), new_proc.clone());
        if !vfork {
            new_proc.vmar().fork_from(&parent.vmar())?;
        }

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
        let mut inner = proc.linux().inner.lock();
        let child = inner.children.get(&pid).ok_or(LxError::ECHILD)?;
        if let Status::Exited(code) = child.status() {
            inner.children.remove(&pid);
            return Ok(code as ExitCode);
        }
        if nonblock {
            return Err(LxError::EAGAIN);
        }
        let child: Arc<dyn KernelObject> = child.clone();
        drop(inner);
        child.wait_signal(Signal::PROCESS_TERMINATED).await;
    }
}

/// Wait for state changes in a child of the calling process.
pub async fn wait_child_any(proc: &Arc<Process>, nonblock: bool) -> LxResult<(KoID, ExitCode)> {
    loop {
        let mut inner = proc.linux().inner.lock();
        if inner.children.is_empty() {
            return Err(LxError::ECHILD);
        }
        for (&pid, child) in inner.children.iter() {
            if let Status::Exited(code) = child.status() {
                inner.children.remove(&pid);
                return Ok((pid, code as ExitCode));
            }
        }
        drop(inner);
        if nonblock {
            return Err(LxError::EAGAIN);
        }
        let proc: Arc<dyn KernelObject> = proc.clone();
        proc.wait_signal(Signal::SIGCHLD).await;
    }
}

/// Linux specific process information.
pub struct LinuxProcess {
    /// The root INode of file system
    root_inode: Arc<dyn INode>,
    /// Parent process
    parent: Weak<Process>,
    /// Inner
    inner: Mutex<LinuxProcessInner>,
}

#[derive(Default)]
struct LinuxProcessInner {
    /// Execute path
    execute_path: String,
    /// Current Working Directory
    ///
    /// Omit leading '/'.
    current_working_directory: String,
    /// Opened files
    files: HashMap<FileDesc, Arc<dyn FileLike>>,
    /// Futexes
    futexes: HashMap<VirtAddr, Arc<Futex>>,
    /// Child processes
    children: HashMap<KoID, Arc<Process>>,
}

pub type ExitCode = i32;

impl LinuxProcess {
    /// Create a new process.
    pub fn new(rootfs: Arc<dyn FileSystem>) -> Self {
        let stdin = File::new(
            STDIN.clone(), // FIXME: stdin
            OpenOptions {
                read: true,
                write: false,
                append: false,
                nonblock: false,
            },
            String::from("/dev/stdin"),
        ) as Arc<dyn FileLike>;
        let stdout = File::new(
            STDOUT.clone(), // TODO: open from '/dev/stdout'
            OpenOptions {
                read: false,
                write: true,
                append: false,
                nonblock: false,
            },
            String::from("/dev/stdout"),
        ) as Arc<dyn FileLike>;
        let mut files = HashMap::new();
        files.insert(0.into(), stdin);
        files.insert(1.into(), stdout.clone());
        files.insert(2.into(), stdout);

        LinuxProcess {
            root_inode: create_root_fs(rootfs),
            parent: Weak::default(),
            inner: Mutex::new(LinuxProcessInner {
                files,
                ..Default::default()
            }),
        }
    }

    /// Get futex object.
    #[allow(unsafe_code)]
    pub fn get_futex(&self, uaddr: VirtAddr) -> Arc<Futex> {
        let mut inner = self.inner.lock();
        inner
            .futexes
            .entry(uaddr)
            .or_insert_with(|| {
                // FIXME: check address
                let value = unsafe { &*(uaddr as *const AtomicI32) };
                Futex::new(value)
            })
            .clone()
    }

    /// Add a file to the file descriptor table.
    pub fn add_file(&self, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let mut inner = self.inner.lock();
        let fd = inner.get_free_fd();
        inner.files.insert(fd, file);
        Ok(fd)
    }

    /// Add a file to the file descriptor table at given `fd`.
    pub fn add_file_at(&self, fd: FileDesc, file: Arc<dyn FileLike>) {
        let mut inner = self.inner.lock();
        inner.files.insert(fd, file);
    }

    /// Get the `File` with given `fd`.
    pub fn get_file(&self, fd: FileDesc) -> LxResult<Arc<File>> {
        let file = self
            .get_file_like(fd)?
            .downcast_arc::<File>()
            .map_err(|_| LxError::EBADF)?;
        Ok(file)
    }

    /// Get the `FileLike` with given `fd`.
    pub fn get_file_like(&self, fd: FileDesc) -> LxResult<Arc<dyn FileLike>> {
        let inner = self.inner.lock();
        inner.files.get(&fd).cloned().ok_or(LxError::EBADF)
    }

    /// Close file descriptor `fd`.
    pub fn close_file(&self, fd: FileDesc) -> LxResult {
        let mut inner = self.inner.lock();
        inner.files.remove(&fd).map(|_| ()).ok_or(LxError::EBADF)
    }

    /// Get root INode of the process.
    pub fn root_inode(&self) -> &Arc<dyn INode> {
        &self.root_inode
    }

    /// Get parent process.
    pub fn parent(&self) -> Option<Arc<Process>> {
        self.parent.upgrade()
    }

    /// Get current working directory.
    pub fn current_working_directory(&self) -> String {
        String::from("/") + &self.inner.lock().current_working_directory
    }

    /// Change working directory.
    pub fn change_directory(&self, path: &str) {
        if path.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        let cwd = match path.as_bytes()[0] {
            b'/' => String::new(),
            _ => inner.current_working_directory.clone(),
        };
        let mut cwd_vec: Vec<_> = cwd.split('/').filter(|x| !x.is_empty()).collect();
        for seg in path.split('/') {
            match seg {
                ".." => {
                    cwd_vec.pop();
                }
                "." | "" => {} // nothing to do here.
                _ => cwd_vec.push(seg),
            }
        }
        inner.current_working_directory = cwd_vec.join("/");
    }

    /// Get execute path.
    pub fn execute_path(&self) -> String {
        self.inner.lock().execute_path.clone()
    }

    /// Set execute path.
    pub fn set_execute_path(&self, path: &str) {
        self.inner.lock().execute_path = String::from(path);
    }
}

impl LinuxProcessInner {
    fn get_free_fd(&self) -> FileDesc {
        (0usize..)
            .map(|i| i.into())
            .find(|fd| !self.files.contains_key(fd))
            .unwrap()
    }
}

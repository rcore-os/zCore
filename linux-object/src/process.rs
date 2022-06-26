//! Linux Process

use crate::{
    error::{LxError, LxResult},
    fs::{File, FileDesc, FileLike, OpenFlags, STDIN, STDOUT},
    ipc::*,
    net::Socket,
    signal::{Signal as LinuxSignal, SignalAction},
};
use alloc::{
    boxed::Box,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::AtomicI32;
use hashbrown::HashMap;
use kernel_hal::VirtAddr;
use lock::{Mutex, MutexGuard};
use rcore_fs::vfs::{FileSystem, INode};

use zircon_object::{
    object::{KernelObject, KoID, Signal},
    signal::Futex,
    task::{Job, Process, Status},
    ZxResult,
};

pub use rcore_fs::vfs::FsInfo;

/// Process extension for linux
pub trait ProcessExt {
    /// create Linux process
    fn create_linux(job: &Arc<Job>, rootfs: Arc<dyn FileSystem>) -> ZxResult<Arc<Self>>;
    /// get linux process
    fn linux(&self) -> &LinuxProcess;
    /// fork from current linux process
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
                signal_actions: linux_parent_inner.signal_actions.clone(),
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
                info!("Received signal: {:?}", signal);
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
        child.signal_clear(Signal::PROCESS_TERMINATED);
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
            warn!("Nonblock flag isn't supported");
            // return Err(LxError::EAGAIN);
        }
        let proc: Arc<dyn KernelObject> = proc.clone();
        proc.signal_clear(Signal::SIGCHLD);
        //等待进程结束信号
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

/// Linux process mut inner data
#[derive(Default)]
struct LinuxProcessInner {
    /// Execute path
    execute_path: String,
    /// Current Working Directory
    ///
    /// Omit leading '/'.
    current_working_directory: String,
    /// file open number limit
    file_limit: RLimit,
    /// Opened files
    files: HashMap<FileDesc, Arc<dyn FileLike>>,
    /// Semaphore
    semaphores: SemProc,
    /// Share Memory
    shm_identifiers: ShmProc,
    /// Futexes
    futexes: HashMap<VirtAddr, Arc<Futex>>,
    /// Child processes
    children: HashMap<KoID, Arc<Process>>,
    /// Signal actions
    signal_actions: SignalActions,
    /// Sockets
    sockets: HashMap<usize, Arc<Mutex<dyn Socket>>>,
}

#[derive(Clone)]
struct SignalActions {
    table: [SignalAction; LinuxSignal::RTMAX + 1],
}

impl Default for SignalActions {
    fn default() -> Self {
        Self {
            table: [SignalAction::default(); LinuxSignal::RTMAX + 1],
        }
    }
}

/// resource limit
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RLimit {
    /// soft limit
    pub cur: u64,
    /// hard limit
    pub max: u64,
}

impl Default for RLimit {
    fn default() -> Self {
        RLimit {
            cur: 1024,
            max: 1024,
        }
    }
}

/// The type of process exit code.
pub type ExitCode = i32;

impl LinuxProcess {
    /// Create a new process.
    pub fn new(rootfs: Arc<dyn FileSystem>) -> Self {
        let stdin = File::new(
            STDIN.clone(), // FIXME: stdin
            OpenFlags::RDONLY,
            String::from("/dev/stdin"),
        ) as Arc<dyn FileLike>;
        let stdout = File::new(
            STDOUT.clone(), // TODO: open from '/dev/stdout'
            OpenFlags::WRONLY,
            String::from("/dev/stdout"),
        ) as Arc<dyn FileLike>;
        let stderr = File::new(
            STDOUT.clone(), // TODO: open from '/dev/stderr'
            OpenFlags::WRONLY,
            String::from("/dev/stderr"),
        ) as Arc<dyn FileLike>;
        let mut files = HashMap::new();
        files.insert(0.into(), stdin);
        files.insert(1.into(), stdout);
        files.insert(2.into(), stderr);

        LinuxProcess {
            root_inode: crate::fs::create_root_fs(rootfs), //Arc::clone(&ROOT_INODE),访问磁盘可能更快？
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
                let value = unsafe { &*(uaddr as *const AtomicI32) };
                Futex::new(value)
            })
            .clone()
    }

    /// Get lowest free fd
    pub fn get_free_fd(&self) -> FileDesc {
        self.inner.lock().get_free_fd()
    }

    /// get the lowest available fd great than or equal to `start`.
    pub fn get_free_fd_from(&self, start: usize) -> FileDesc {
        self.inner.lock().get_free_fd_from(start)
    }

    /// Add a file to the file descriptor table.
    pub fn add_file(&self, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let inner = self.inner.lock();
        let fd = inner.get_free_fd();
        self.insert_file(inner, fd, file)
    }

    /// Add a file to the file descriptor table at given `fd`.
    pub fn add_file_at(&self, fd: FileDesc, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let inner = self.inner.lock();
        self.insert_file(inner, fd, file)
    }

    /// insert a file and fd into the file descriptor table
    fn insert_file(
        &self,
        mut inner: MutexGuard<LinuxProcessInner>,
        fd: FileDesc,
        file: Arc<dyn FileLike>,
    ) -> LxResult<FileDesc> {
        if inner.files.len() < inner.file_limit.cur as usize {
            inner.files.insert(fd, file);
            Ok(fd)
        } else {
            Err(LxError::EMFILE)
        }
    }

    /// get and set file limit number
    pub fn file_limit(&self, new_limit: Option<RLimit>) -> RLimit {
        let mut inner = self.inner.lock();
        let old = inner.file_limit;
        if let Some(limit) = new_limit {
            inner.file_limit = limit;
        }
        old
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
        trace!("get_file_like: {:#x?}", inner.files);
        inner.files.get(&fd).cloned().ok_or(LxError::EBADF)
    }

    /// get all files
    pub fn get_files(&self) -> LxResult<HashMap<FileDesc, Arc<dyn FileLike>>> {
        let inner = self.inner.lock();
        Ok(inner.files.clone())
    }

    /// Close file descriptor `fd`.
    pub fn close_file(&self, fd: FileDesc) -> LxResult {
        let mut inner = self.inner.lock();
        inner.files.remove(&fd).map(|_| ()).ok_or(LxError::EBADF)
    }

    /// Add a socket to the socket set at given `SocketHandle`.
    pub fn add_socket(&self, socket: Arc<Mutex<dyn Socket>>) -> LxResult<usize> {
        let inner = self.inner.lock();
        let fd = inner.get_free_socket_fd();
        self.insert_socket(inner, fd, socket)
        // unimplemented!()
    }

    /// insert a file and fd into the file descriptor table
    fn insert_socket(
        &self,
        mut inner: MutexGuard<LinuxProcessInner>,
        fd: usize,
        socket: Arc<Mutex<dyn Socket>>,
    ) -> LxResult<usize> {
        if inner.sockets.len() < inner.file_limit.cur as usize {
            inner.sockets.insert(fd, socket);
            Ok(fd)
        } else {
            Err(LxError::EMFILE)
        }
    }

    /// Get the `Socket` with given `fd`.
    pub fn get_socket(&self, fd: usize) -> LxResult<Arc<Mutex<dyn Socket>>> {
        // unimplemented!()
        let inner = self.inner.lock();
        let socket = inner.sockets.get(&fd).cloned().ok_or(LxError::EBADF);
        socket
    }

    /// Close file descriptor `fd`.
    pub fn close_socket(&self, fd: usize) -> LxResult {
        let mut inner = self.inner.lock();
        inner.sockets.remove(&fd).map(|_| ()).ok_or(LxError::EBADF)
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

    /// Get signal action.
    pub fn signal_action(&self, signal: LinuxSignal) -> SignalAction {
        self.inner.lock().signal_actions.table[signal as u8 as usize]
    }

    /// Set signal action.
    pub fn set_signal_action(&self, signal: LinuxSignal, action: SignalAction) {
        self.inner.lock().signal_actions.table[signal as u8 as usize] = action;
    }

    /// Close file that FD_CLOEXEC is set
    pub fn remove_cloexec_files(&self) {
        let mut inner = self.inner.lock();
        let close_fds = inner
            .files
            .iter()
            .filter_map(|(fd, file_like)| {
                if let Ok(file) = file_like.clone().downcast_arc::<File>() {
                    if file.flags().close_on_exec() {
                        Some(*fd)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for fd in close_fds {
            inner.files.remove(&fd).map(|_| ()).unwrap();
        }
    }

    /// Insert a `SemArray` and return its ID
    pub fn semaphores_add(&self, array: Arc<SemArray>) -> usize {
        self.inner.lock().semaphores.add(array)
    }

    /// Get an semaphore set by `id`
    pub fn semaphores_get(&self, id: usize) -> Option<Arc<SemArray>> {
        self.inner.lock().semaphores.get(id)
    }

    /// Add an undo operation
    pub fn semaphores_add_undo(&self, id: usize, num: u16, op: i16) {
        self.inner.lock().semaphores.add_undo(id, num, op)
    }

    /// Remove an `SemArray` by ID
    pub fn semaphores_remove(&self, id: usize) {
        self.inner.lock().semaphores.remove(id)
    }

    /// get ShmId from Virtual Addr
    pub fn shm_get_id(&self, id: usize) -> Option<usize> {
        self.inner.lock().shm_identifiers.get_id(id)
    }

    /// get the ShmIdentifier from shm_identifiers
    pub fn shm_get(&self, id: usize) -> Option<ShmIdentifier> {
        self.inner.lock().shm_identifiers.get(id)
    }

    /// Delete the ShmIdentifier from shm_identifiers
    pub fn shm_pop(&self, id: usize) {
        self.inner.lock().shm_identifiers.pop(id)
    }

    /// Insert the `SharedGuard` and return its ID
    pub fn shm_add(&self, shared_guard: Arc<Mutex<ShmGuard>>) -> usize {
        self.inner.lock().shm_identifiers.add(shared_guard)
    }

    /// Set Virtual Addr for shared memory
    pub fn shm_set(&self, id: usize, shm_id: ShmIdentifier) {
        self.inner.lock().shm_identifiers.set(id, shm_id)
    }
}

impl LinuxProcessInner {
    fn get_free_fd(&self) -> FileDesc {
        self.get_free_fd_from(0)
    }

    fn get_free_fd_from(&self, start: usize) -> FileDesc {
        (start..)
            .map(|i| i.into())
            .find(|fd| !self.files.contains_key(fd))
            .unwrap()
    }

    fn get_free_socket_fd(&self) -> usize {
        (SOCKET_FD..)
            .find(|fd| !self.sockets.contains_key(fd))
            .unwrap()
    }
}

// Temp , TODO warp a struct impl into/from with FileDesc and SocketHandle
const SOCKET_FD: usize = 10000;

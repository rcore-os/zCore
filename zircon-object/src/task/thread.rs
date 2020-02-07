use {
    self::thread_state::*,
    super::process::Process,
    super::*,
    crate::object::*,
    alloc::{boxed::Box, string::String, sync::Arc},
    core::any::Any,
    spin::Mutex,
};

mod thread_state;

/// Runnable / computation entity
///
/// ## SYNOPSIS
///
/// TODO
///
/// ## DESCRIPTION
///
/// The thread object is the construct that represents a time-shared CPU execution
/// context. Thread objects live associated to a particular
/// [Process Object](crate::task::Process) which provides the memory and the handles to other
/// objects necessary for I/O and computation.
///
/// ### Lifetime
/// Threads are created by calling [`Thread::create()`], but only start executing
/// when either [`Thread::start()`] or [`Process::start()`] are called. Both syscalls
/// take as an argument the entrypoint of the initial routine to execute.
///
/// The thread passed to [`Process::start()`] should be the first thread to start execution
/// on a process.
///
/// A thread terminates execution:
/// - by calling [`Thread::exit()`]
/// - when the parent process terminates
/// - by calling [`Task::kill()`]
/// - after generating an exception for which there is no handler or the handler
/// decides to terminate the thread.
///
/// Returning from the entrypoint routine does not terminate execution. The last
/// action of the entrypoint should be to call [`Thread::exit()`].
///
/// Closing the last handle to a thread does not terminate execution. In order to
/// forcefully kill a thread for which there is no available handle, use
/// `KernelObject::get_child()` to obtain a handle to the thread. This method is strongly
/// discouraged. Killing a thread that is executing might leave the process in a
/// corrupt state.
///
/// Fuchsia native threads are always *detached*. That is, there is no *join()* operation
/// needed to do a clean termination. However, some runtimes above the kernel, such as
/// C11 or POSIX might require threads to be joined.
///
/// ### Signals
/// Threads provide the following signals:
/// - [`THREAD_TERMINATED`]
/// - [`THREAD_SUSPENDED`]
/// - [`THREAD_RUNNING`]
///
/// When a thread is started [`THREAD_RUNNING`] is asserted. When it is suspended
/// [`THREAD_RUNNING`] is deasserted, and [`THREAD_SUSPENDED`] is asserted. When
/// the thread is resumed [`THREAD_SUSPENDED`] is deasserted and
/// [`THREAD_RUNNING`] is asserted. When a thread terminates both
/// [`THREAD_RUNNING`] and [`THREAD_SUSPENDED`] are deasserted and
/// [`THREAD_TERMINATED`] is asserted.
///
/// Note that signals are OR'd into the state maintained by the
/// `KernelObject::wait_signal_async()` family of functions thus
/// you may see any combination of requested signals when they return.
///
/// [`Thread::create()`]: Thread::create
/// [`Thread::exit()`]: Thread::exit
/// [`Process::exit()`]: crate::task::Process::exit
/// [`THREAD_TERMINATED`]: crate::object::Signal::THREAD_TERMINATED
/// [`THREAD_SUSPENDED`]: crate::object::Signal::THREAD_SUSPENDED
/// [`THREAD_RUNNING`]: crate::object::Signal::THREAD_RUNNING
pub struct Thread {
    base: KObjectBase,
    #[allow(dead_code)]
    name: String,
    proc: Arc<Process>,
    ext: Box<dyn Any + Send + Sync>,
    inner: Mutex<ThreadInner>,
}

impl_kobject!(Thread);

#[derive(Default)]
struct ThreadInner {
    /// HAL thread handle
    ///
    /// Should be `None` before start or after terminated.
    hal_thread: Option<kernel_hal::Thread>,

    /// Thread state
    ///
    /// Only be `Some` on suspended.
    state: Option<ThreadState>,
}

impl Thread {
    /// Create a new thread.
    pub fn create(proc: &Arc<Process>, name: &str, _options: u32) -> ZxResult<Arc<Self>> {
        Self::create_with_ext(proc, name, ())
    }

    /// Create a new thread with extension info.
    pub fn create_with_ext(
        proc: &Arc<Process>,
        name: &str,
        ext: impl Any + Send + Sync,
    ) -> ZxResult<Arc<Self>> {
        // TODO: options
        let thread = Arc::new(Thread {
            base: KObjectBase::new(),
            name: String::from(name),
            proc: proc.clone(),
            ext: Box::new(ext),
            inner: Mutex::new(ThreadInner::default()),
        });
        proc.add_thread(thread.clone());
        Ok(thread)
    }

    /// Get the process.
    pub fn proc(&self) -> &Arc<Process> {
        &self.proc
    }

    /// Get the extension.
    pub fn ext(&self) -> &Box<dyn Any + Send + Sync> {
        &self.ext
    }

    /// Start execution on the thread.
    pub fn start(
        self: &Arc<Self>,
        entry: usize,
        stack: usize,
        arg1: usize,
        arg2: usize,
    ) -> ZxResult<()> {
        let regs = GeneralRegs::new_fn(entry, stack, arg1, arg2);
        self.start_with_regs(regs)
    }

    /// Start execution with given registers.
    pub fn start_with_regs(self: &Arc<Self>, regs: GeneralRegs) -> ZxResult<()> {
        let mut inner = self.inner.lock();
        if inner.hal_thread.is_some() {
            return Err(ZxError::BAD_STATE);
        }
        let hal_thread = kernel_hal::Thread::spawn(self.clone(), regs);
        inner.hal_thread = Some(hal_thread);
        self.base.signal_set(Signal::THREAD_RUNNING);
        Ok(())
    }

    /// Terminate the current running thread.
    /// TODO: move to CurrentThread
    pub fn exit(&self) {
        self.proc().remove_thread(self.base.id);
        self.base.signal_set(Signal::THREAD_TERMINATED);
    }

    /// Read one aspect of thread state.
    pub fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize> {
        let inner = self.inner.lock();
        let state = inner.state.as_ref().ok_or(ZxError::BAD_STATE)?;
        let len = state.read(kind, buf)?;
        Ok(len)
    }

    /// Write one aspect of thread state.
    pub fn write_state(&self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult<()> {
        let mut inner = self.inner.lock();
        let state = inner.state.as_mut().ok_or(ZxError::BAD_STATE)?;
        state.write(kind, buf)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::job::Job;
    use super::*;
    use std::sync::atomic::*;
    use std::vec;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let _thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
    }

    #[test]
    fn start() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
        let thread1 = Thread::create(&proc, "thread1", 0).expect("failed to create thread");

        // allocate stack for new thread
        let mut stack = vec![0u8; 0x1000];
        let stack_top = stack.as_mut_ptr() as usize + 0x1000;

        // global variable for validation
        static ARG1: AtomicUsize = AtomicUsize::new(0);
        static ARG2: AtomicUsize = AtomicUsize::new(0);

        // function for new thread
        #[allow(unsafe_code)]
        extern "C" fn entry(arg1: usize, arg2: usize) -> ! {
            unsafe {
                kernel_hal_unix::switch_to_kernel();
            }
            ARG1.store(arg1, Ordering::SeqCst);
            ARG2.store(arg2, Ordering::SeqCst);
            loop {
                std::thread::park();
            }
        }

        // start a new thread
        let thread_ref_count = Arc::strong_count(&thread);
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        proc.start(&thread, entry as usize, stack_top, handle.clone(), 2)
            .expect("failed to start thread");

        // wait 100ms for the new thread to exit
        std::thread::sleep(core::time::Duration::from_millis(100));

        // validate the thread have started and received correct arguments
        assert_eq!(ARG1.load(Ordering::SeqCst), 0);
        assert_eq!(ARG2.load(Ordering::SeqCst), 2);

        // no other references to `Thread`
        assert_eq!(Arc::strong_count(&thread), thread_ref_count);

        // start again should fail
        assert_eq!(
            proc.start(&thread, entry as usize, stack_top, handle.clone(), 2),
            Err(ZxError::BAD_STATE)
        );

        // start another thread should fail
        assert_eq!(
            proc.start(&thread1, entry as usize, stack_top, handle.clone(), 2),
            Err(ZxError::BAD_STATE)
        );
    }
}

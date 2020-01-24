use {
    super::process::Process, super::*, crate::object::*, alloc::string::String, alloc::sync::Arc,
    spin::Mutex,
};

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
/// [`KernelObject::wait_signal()`] family of functions thus
/// you may see any combination of requested signals when they return.
///
/// [`Thread::create()`]: Thread::create
/// [`Thread::exit()`]: Thread::exit
/// [`Process::exit()`]: crate::task::Process::exit
/// [`KernelObject::wait_signal()`]: crate::object::KernelObject::wait_signal
/// [`THREAD_TERMINATED`]: crate::object::Signal::THREAD_TERMINATED
/// [`THREAD_SUSPENDED`]: crate::object::Signal::THREAD_SUSPENDED
/// [`THREAD_RUNNING`]: crate::object::Signal::THREAD_RUNNING
pub struct Thread {
    base: KObjectBase,
    #[allow(dead_code)]
    name: String,
    pub proc: Arc<Process>,
    inner: Mutex<ThreadInner>,
}

impl_kobject!(Thread);

#[derive(Default)]
struct ThreadInner {
    hal_thread: Option<crate::hal::Thread>,
}

impl Thread {
    /// Create a new thread.
    pub fn create(proc: &Arc<Process>, name: &str, _options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let thread = Arc::new(Thread {
            base: KObjectBase::new(),
            name: String::from(name),
            proc: proc.clone(),
            inner: Mutex::new(ThreadInner::default()),
        });
        proc.add_thread(thread.clone());
        Ok(thread)
    }

    /// Get current `Thread` object.
    pub fn current() -> Arc<Self> {
        crate::hal::Thread::tls()
    }

    /// Start execution on the thread.
    pub fn start(
        self: &Arc<Self>,
        entry: usize,
        stack: usize,
        arg1: usize,
        arg2: usize,
    ) -> ZxResult<()> {
        let hal_thread = crate::hal::Thread::spawn(entry, stack, arg1, arg2, self.clone());
        let mut inner = self.inner.lock();
        if inner.hal_thread.is_some() {
            return Err(ZxError::BAD_STATE);
        }
        inner.hal_thread = Some(hal_thread);
        Ok(())
    }

    /// Terminate the current running thread.
    pub fn exit(&self) -> ZxResult<()> {
        self.inner
            .lock()
            .hal_thread
            .as_mut()
            .ok_or(ZxError::BAD_STATE)?
            .exit();
        Ok(())
    }

    /// Read one aspect of thread state.
    pub fn read_state(&self, _kind: ThreadStateKind) -> ZxResult<ThreadState> {
        unimplemented!()
    }

    /// Write one aspect of thread state.
    pub fn write_state(&self, _state: &ThreadState) -> ZxResult<()> {
        unimplemented!()
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum ThreadStateKind {
    General = 0,
    FloatPoint = 1,
    Vector = 2,
    Debug = 4,
    SingleStep = 5,
    FS = 6,
    GS = 7,
}

#[derive(Debug)]
pub enum ThreadState {}

#[cfg(test)]
mod tests {
    use super::job::Job;
    use super::*;
    use std::sync::atomic::*;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let _thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
    }

    #[test]
    #[allow(unsafe_code)]
    fn start() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
        let thread1 = Thread::create(&proc, "thread1", 0).expect("failed to create thread");

        // allocate stack for new thread
        static mut STACK: [u8; 0x1000] = [0u8; 0x1000];
        let stack_top = unsafe { STACK.as_ptr() } as usize + 0x1000;

        // global variable for validation
        static ARG1: AtomicUsize = AtomicUsize::new(0);
        static ARG2: AtomicUsize = AtomicUsize::new(0);

        // function for new thread
        extern "C" fn entry(arg1: usize, arg2: usize) -> ! {
            unsafe {
                zircon_hal_unix::switch_to_kernel();
            }
            ARG1.store(arg1, Ordering::SeqCst);
            ARG2.store(arg2, Ordering::SeqCst);
            loop {
                std::thread::park();
            }
        }

        // start a new thread
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        proc.start(&thread, entry as usize, stack_top, handle.clone(), 2)
            .expect("failed to start thread");

        // wait 100ms for the new thread to exit
        std::thread::sleep(core::time::Duration::from_millis(100));

        // validate the thread have started and received correct arguments
        assert_eq!(ARG1.load(Ordering::SeqCst), 0);
        assert_eq!(ARG2.load(Ordering::SeqCst), 2);

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

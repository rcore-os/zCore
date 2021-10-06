use {
    super::exception::*,
    super::process::Process,
    super::*,
    crate::object::*,
    alloc::{boxed::Box, sync::Arc},
    bitflags::bitflags,
    core::{
        any::Any,
        future::Future,
        ops::Deref,
        pin::Pin,
        task::{Context, Poll, Waker},
        time::Duration,
    },
    futures::{channel::oneshot::*, future::FutureExt, pin_mut, select_biased},
    kernel_hal::context::{GeneralRegs, UserContext},
    spin::Mutex,
};

pub use self::thread_state::*;

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
/// - by calling [`CurrentThread::exit()`]
/// - when the parent process terminates
/// - by calling [`Task::kill()`]
/// - after generating an exception for which there is no handler or the handler
/// decides to terminate the thread.
///
/// Returning from the entrypoint routine does not terminate execution. The last
/// action of the entrypoint should be to call [`CurrentThread::exit()`].
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
/// `KernelObject::wait_signal()` family of functions thus
/// you may see any combination of requested signals when they return.
///
/// [`Thread::create()`]: Thread::create
/// [`CurrentThread::exit()`]: CurrentThread::exit
/// [`Process::exit()`]: crate::task::Process::exit
/// [`THREAD_TERMINATED`]: crate::object::Signal::THREAD_TERMINATED
/// [`THREAD_SUSPENDED`]: crate::object::Signal::THREAD_SUSPENDED
/// [`THREAD_RUNNING`]: crate::object::Signal::THREAD_RUNNING
pub struct Thread {
    base: KObjectBase,
    _counter: CountHelper,
    proc: Arc<Process>,
    ext: Box<dyn Any + Send + Sync>,
    inner: Mutex<ThreadInner>,
    exceptionate: Arc<Exceptionate>,
}

impl_kobject!(Thread
    fn related_koid(&self) -> KoID {
        self.proc.id()
    }
);
define_count_helper!(Thread);

#[derive(Default)]
struct ThreadInner {
    /// Thread context
    ///
    /// It will be taken away when running this thread.
    context: Option<Box<UserContext>>,

    /// The number of existing `SuspendToken`.
    suspend_count: usize,
    /// The waker of task when suspending.
    waker: Option<Waker>,
    /// A token used to kill blocking thread
    killer: Option<Sender<()>>,
    /// Thread state
    ///
    /// NOTE: This variable will never be `Suspended`. On suspended, the
    /// `suspend_count` is non-zero, and this represents the state before suspended.
    state: ThreadState,
    /// The currently processing exception
    exception: Option<Arc<Exception>>,
    /// Should The ProcessStarting exception generated at start of this thread
    first_thread: bool,
    /// Should The ThreadExiting exception do not block this thread
    killed: bool,
    /// The time this thread has run on cpu
    time: u128,
    flags: ThreadFlag,
}

impl ThreadInner {
    fn state(&self) -> ThreadState {
        // Dying > Exception > Suspend > Blocked
        if self.suspend_count == 0
            || self.context.is_none()
            || self.state == ThreadState::BlockedException
            || self.state == ThreadState::Dying
            || self.state == ThreadState::Dead
        {
            self.state
        } else {
            ThreadState::Suspended
        }
    }

    /// Change state and update signal.
    fn change_state(&mut self, state: ThreadState, base: &KObjectBase) {
        self.state = state;
        match self.state() {
            ThreadState::Dead => base.signal_change(
                Signal::THREAD_RUNNING | Signal::THREAD_SUSPENDED,
                Signal::THREAD_TERMINATED,
            ),
            ThreadState::New | ThreadState::Dying => base.signal_clear(
                Signal::THREAD_RUNNING | Signal::THREAD_SUSPENDED | Signal::THREAD_TERMINATED,
            ),
            ThreadState::Suspended => base.signal_change(
                Signal::THREAD_RUNNING | Signal::THREAD_TERMINATED,
                Signal::THREAD_SUSPENDED,
            ),
            _ => base.signal_change(
                Signal::THREAD_TERMINATED | Signal::THREAD_SUSPENDED,
                Signal::THREAD_RUNNING,
            ),
        }
    }
}

bitflags! {
    /// Thread flags.
    #[derive(Default)]
    pub struct ThreadFlag: usize {
        /// The thread currently has a VCPU.
        const VCPU = 1 << 3;
    }
}

/// The type of a new thread function.
pub type ThreadFn = fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

impl Thread {
    /// Create a new thread.
    pub fn create(proc: &Arc<Process>, name: &str) -> ZxResult<Arc<Self>> {
        Self::create_with_ext(proc, name, ())
    }

    /// Create a new thread with extension info.
    ///
    /// # Example
    /// ```
    /// # use std::sync::Arc;
    /// # use zircon_object::task::*;
    /// # kernel_hal::init();
    /// let job = Job::root();
    /// let proc = Process::create(&job, "proc").unwrap();
    /// // create a thread with extension info
    /// let thread = Thread::create_with_ext(&proc, "thread", job.clone()).unwrap();
    /// // get the extension info
    /// let ext = thread.ext().downcast_ref::<Arc<Job>>().unwrap();
    /// assert!(Arc::ptr_eq(ext, &job));
    /// ```
    pub fn create_with_ext(
        proc: &Arc<Process>,
        name: &str,
        ext: impl Any + Send + Sync,
    ) -> ZxResult<Arc<Self>> {
        let thread = Arc::new(Thread {
            base: KObjectBase::with_name(name),
            _counter: CountHelper::new(),
            proc: proc.clone(),
            ext: Box::new(ext),
            exceptionate: Exceptionate::new(ExceptionChannelType::Thread),
            inner: Mutex::new(ThreadInner {
                context: Some(Box::new(UserContext::default())),
                ..Default::default()
            }),
        });
        proc.add_thread(thread.clone())?;
        Ok(thread)
    }

    /// Get the process.
    pub fn proc(&self) -> &Arc<Process> {
        &self.proc
    }

    /// Get the extension info.
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
        thread_fn: ThreadFn,
    ) -> ZxResult {
        {
            let mut inner = self.inner.lock();
            let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
            #[cfg(target_arch = "x86_64")]
            {
                context.general.rip = entry;
                context.general.rsp = stack;
                context.general.rdi = arg1;
                context.general.rsi = arg2;
                // FIXME: set IOPL = 0 when IO port bitmap is supported
                context.general.rflags = 0x3202; // IOPL = 3, enable interrupt
            }
            #[cfg(target_arch = "aarch64")]
            {
                context.elr = entry;
                context.sp = stack;
                context.general.x0 = arg1;
                context.general.x1 = arg2;
            }
            #[cfg(target_arch = "riscv64")]
            {
                context.sepc = entry;
                context.general.sp = stack;
                context.general.a0 = arg1;
                context.general.a1 = arg2;
                context.sstatus = 1 << 18 | 3 << 13 | 1 << 5; // SUM | FS | SPIE
            }
            inner.change_state(ThreadState::Running, &self.base);
        }
        let vmtoken = self.proc().vmar().table_phys();
        kernel_hal::thread::spawn(thread_fn(CurrentThread(self.clone())), vmtoken);
        Ok(())
    }

    /// Start execution with given registers.
    pub fn start_with_regs(self: &Arc<Self>, regs: GeneralRegs, thread_fn: ThreadFn) -> ZxResult {
        {
            let mut inner = self.inner.lock();
            let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
            context.general = regs;
            #[cfg(target_arch = "x86_64")]
            {
                // FIXME: set IOPL = 0 when IO port bitmap is supported
                context.general.rflags |= 0x3202; // IOPL = 3, enable interrupt
            }
            inner.change_state(ThreadState::Running, &self.base);
        }
        let vmtoken = self.proc().vmar().table_phys();
        kernel_hal::thread::spawn(thread_fn(CurrentThread(self.clone())), vmtoken);
        Ok(())
    }

    /// Similar to start_with_regs(), but change a parameter: context
    pub fn start_with_context(self: &Arc<Self>, cx: &UserContext, thread_fn: ThreadFn) -> ZxResult {
        {
            let mut inner = self.inner.lock();
            let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
            context.general = cx.general;
            context.set_syscall_ret(0);

            #[cfg(target_arch = "riscv64")]
            {
                context.sepc = cx.sepc;
                context.sstatus = 1 << 18 | 3 << 13 | 1 << 5; // SUM | FS | SPIE
                debug!("start_with_regs_pc(), sepc: {:#x}", context.sepc);
            }
            inner.change_state(ThreadState::Running, &self.base);
        }
        let vmtoken = self.proc().vmar().table_phys();
        kernel_hal::thread::spawn(thread_fn(CurrentThread(self.clone())), vmtoken);
        Ok(())
    }

    /// Stop the thread. Internal implementation of `exit` and `kill`.
    ///
    /// The thread do not terminate immediately when stopped. It is just made dying.
    /// It will terminate after some cleanups (when `terminate` are called **explicitly** by upper layer).
    fn stop(&self, killed: bool) {
        let mut inner = self.inner.lock();
        if inner.state == ThreadState::Dead {
            return;
        }
        if killed {
            inner.killed = true;
        }
        if inner.state == ThreadState::Dying {
            if killed {
                if let Some(killer) = inner.killer.take() {
                    // It's ok to ignore the error since the other end could be closed
                    killer.send(()).ok();
                }
            }
            return;
        }
        inner.change_state(ThreadState::Dying, &self.base);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
        // For blocking thread, use the killer
        if let Some(killer) = inner.killer.take() {
            // It's ok to ignore the error since the other end could be closed
            killer.send(()).ok();
        }
    }

    /// Read one aspect of thread state.
    pub fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize> {
        let inner = self.inner.lock();
        let state = inner.state();
        if state != ThreadState::BlockedException && state != ThreadState::Suspended {
            if inner.exception.is_some() {
                return Err(ZxError::NOT_SUPPORTED);
            }
            return Err(ZxError::BAD_STATE);
        }
        let context = inner.context.as_ref().ok_or(ZxError::BAD_STATE)?;
        context.read_state(kind, buf)
    }

    /// Write one aspect of thread state.
    pub fn write_state(&self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult {
        let mut inner = self.inner.lock();
        let state = inner.state();
        if state != ThreadState::BlockedException && state != ThreadState::Suspended {
            if inner.exception.is_some() {
                return Err(ZxError::NOT_SUPPORTED);
            }
            return Err(ZxError::BAD_STATE);
        }
        let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
        context.write_state(kind, buf)
    }

    /// Get the thread's information.
    pub fn get_thread_info(&self) -> ThreadInfo {
        let inner = self.inner.lock();
        ThreadInfo {
            state: inner.state() as u32,
            wait_exception_channel_type: inner
                .exception
                .as_ref()
                .map_or(0, |exception| exception.current_channel_type() as u32),
            cpu_affinity_mask: [0u64; 8],
        }
    }

    /// Get the thread's exception report.
    pub fn get_thread_exception_info(&self) -> ZxResult<ExceptionReport> {
        let inner = self.inner.lock();
        if inner.state() != ThreadState::BlockedException {
            return Err(ZxError::BAD_STATE);
        }
        let report = inner.exception.as_ref().ok_or(ZxError::BAD_STATE)?.report();
        Ok(report)
    }

    /// Get the thread state.
    pub fn state(&self) -> ThreadState {
        self.inner.lock().state()
    }

    /// Add the parameter to the time this thread has run on cpu.
    pub fn time_add(&self, time: u128) {
        self.inner.lock().time += time;
    }

    /// Get the time this thread has run on cpu.
    pub fn get_time(&self) -> u64 {
        self.inner.lock().time as u64
    }

    /// Set this thread as the first thread of a process.
    pub(super) fn set_first_thread(&self) {
        self.inner.lock().first_thread = true;
    }

    /// Whether this thread is the first thread of a process.
    pub fn is_first_thread(&self) -> bool {
        self.inner.lock().first_thread
    }

    /// Get the thread's flags.
    pub fn flags(&self) -> ThreadFlag {
        self.inner.lock().flags
    }

    /// Apply `f` to the thread's flags.
    pub fn update_flags(&self, f: impl FnOnce(&mut ThreadFlag)) {
        f(&mut self.inner.lock().flags)
    }

    /// Set the thread local fsbase register on x86_64.
    #[cfg(target_arch = "x86_64")]
    pub fn set_fsbase(&self, fsbase: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
        context.general.fsbase = fsbase;
        Ok(())
    }

    /// Set the thread local gsbase register on x86_64.
    #[cfg(target_arch = "x86_64")]
    pub fn set_gsbase(&self, gsbase: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        let context = inner.context.as_mut().ok_or(ZxError::BAD_STATE)?;
        context.general.gsbase = gsbase;
        Ok(())
    }
}

impl Task for Thread {
    fn kill(&self) {
        self.stop(true)
    }

    fn suspend(&self) {
        let mut inner = self.inner.lock();
        inner.suspend_count += 1;
        let state = inner.state;
        inner.change_state(state, &self.base);
    }

    fn resume(&self) {
        let mut inner = self.inner.lock();
        assert_ne!(inner.suspend_count, 0);
        inner.suspend_count -= 1;
        if inner.suspend_count == 0 {
            let state = inner.state;
            inner.change_state(state, &self.base);
            if let Some(waker) = inner.waker.take() {
                waker.wake();
            }
        }
    }

    fn exceptionate(&self) -> Arc<Exceptionate> {
        self.exceptionate.clone()
    }

    fn debug_exceptionate(&self) -> Arc<Exceptionate> {
        panic!("thread do not have debug exceptionate");
    }
}

/// A handle to current thread.
///
/// This is a wrapper of [`Thread`] that provides additional methods for the thread runner.
/// It can only be obtained from the argument of `thread_fn` in a new thread started by [`Thread::start`].
///
/// It will terminate current thread on drop.
///
/// [`Thread`]: crate::task::Thread
/// [`Thread::start`]: crate::task::Thread::start
pub struct CurrentThread(pub(super) Arc<Thread>);

impl Deref for CurrentThread {
    type Target = Arc<Thread>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for CurrentThread {
    /// Terminate the current running thread.
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        self.exceptionate.shutdown();
        inner.change_state(ThreadState::Dead, &self.base);
        self.proc().remove_thread(self.base.id);
    }
}

impl CurrentThread {
    /// Exit the current thread.
    ///
    /// The thread do not terminate immediately when exited. It is just made dying.
    /// It will terminate after some cleanups on this struct drop.
    pub fn exit(&self) {
        self.stop(false);
    }

    /// Wait until the thread is ready to run (not suspended),
    /// and then take away its context to run the thread.
    pub fn wait_for_run(&self) -> impl Future<Output = Box<UserContext>> {
        #[must_use = "wait_for_run does nothing unless polled/`await`-ed"]
        struct RunnableChecker {
            thread: Arc<Thread>,
        }
        impl Future for RunnableChecker {
            type Output = Box<UserContext>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                let mut inner = self.thread.inner.lock();
                if inner.state() != ThreadState::Suspended {
                    // resume:  return the context token from thread object
                    // There is no need to call change_state here
                    // since take away the context of a non-suspended thread won't change it's state
                    Poll::Ready(inner.context.take().unwrap())
                } else {
                    // suspend: put waker into the thread object
                    inner.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
        RunnableChecker {
            thread: self.0.clone(),
        }
    }

    /// The thread ends running and takes back the context.
    pub fn end_running(&self, context: Box<UserContext>) {
        let mut inner = self.inner.lock();
        inner.context = Some(context);
        let state = inner.state;
        inner.change_state(state, &self.base);
    }

    /// Access saved context of current thread.
    ///
    /// Will panic if the context is not availiable.
    pub fn with_context<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&mut UserContext) -> T,
    {
        let mut inner = self.inner.lock();
        let mut cx = inner.context.as_mut().unwrap();
        f(&mut cx)
    }

    /// Run async future and change state while blocking.
    pub async fn blocking_run<F, T, FT>(
        &self,
        future: F,
        state: ThreadState,
        deadline: Duration,
        cancel_token: Option<Receiver<()>>,
    ) -> ZxResult<T>
    where
        F: Future<Output = FT> + Unpin,
        FT: IntoResult<T>,
    {
        let (old_state, killed) = {
            let mut inner = self.inner.lock();
            if inner.state() == ThreadState::Dying {
                return Err(ZxError::STOP);
            }
            let (sender, receiver) = channel();
            inner.killer = Some(sender);
            let old_state = inner.state;
            inner.change_state(state, &self.base);
            (old_state, receiver)
        };
        let ret = if let Some(cancel_token) = cancel_token {
            select_biased! {
                ret = future.fuse() => ret.into_result(),
                _ = killed.fuse() => Err(ZxError::STOP),
                _ = kernel_hal::thread::sleep_until(deadline).fuse() => Err(ZxError::TIMED_OUT),
                _ = cancel_token.fuse() => Err(ZxError::CANCELED),
            }
        } else {
            select_biased! {
                ret = future.fuse() => ret.into_result(),
                _ = killed.fuse() => Err(ZxError::STOP),
                _ = kernel_hal::thread::sleep_until(deadline).fuse() => Err(ZxError::TIMED_OUT),
            }
        };
        let mut inner = self.inner.lock();
        inner.killer = None;
        if inner.state() == ThreadState::Dying {
            return ret;
        }
        assert_eq!(inner.state, state);
        inner.change_state(old_state, &self.base);
        ret
    }

    /// Create an exception on this thread and wait for the handling.
    pub async fn handle_exception(&self, type_: ExceptionType) {
        let exception = {
            let mut inner = self.inner.lock();
            let cx = if !type_.is_synth() {
                inner.context.as_ref().map(|cx| cx.as_ref())
            } else {
                None
            };
            if !type_.is_synth() {
                error!(
                    "User mode exception: {:?} {:#x?}",
                    type_,
                    cx.expect("Architectural exception should has context")
                );
            }
            let exception = Exception::new(&self.0, type_, cx);
            inner.exception = Some(exception.clone());
            exception
        };
        if type_ == ExceptionType::ThreadExiting {
            let handled = self
                .0
                .proc()
                .debug_exceptionate()
                .send_exception(&exception);
            if let Ok(future) = handled {
                self.dying_run(future).await.ok();
            }
        } else {
            let future = exception.handle();
            pin_mut!(future);
            self.blocking_run(
                future,
                ThreadState::BlockedException,
                Duration::from_nanos(u64::max_value()),
                None,
            )
            .await
            .ok();
        }
        self.inner.lock().exception = None;
    }

    /// Run a blocking task when the thread is exited itself and dying.
    ///
    /// The task will stop running if and once the thread is killed.
    async fn dying_run<F, T, FT>(&self, future: F) -> ZxResult<T>
    where
        F: Future<Output = FT> + Unpin,
        FT: IntoResult<T>,
    {
        let killed = {
            let mut inner = self.inner.lock();
            if inner.killed {
                return Err(ZxError::STOP);
            }
            let (sender, receiver) = channel::<()>();
            inner.killer = Some(sender);
            receiver
        };
        select_biased! {
            ret = future.fuse() => ret.into_result(),
            _ = killed.fuse() => Err(ZxError::STOP),
        }
    }
}

/// `into_result` returns `Self` if the type parameter is already a `ZxResult`,
/// otherwise wraps the value in an `Ok`.
///
/// Used to implement `Thread::blocking_run`, which takes a future whose `Output` may
/// or may not be a `ZxResult`.
pub trait IntoResult<T> {
    /// Performs the conversion.
    fn into_result(self) -> ZxResult<T>;
}

impl<T> IntoResult<T> for T {
    fn into_result(self) -> ZxResult<T> {
        Ok(self)
    }
}

impl<T> IntoResult<T> for ZxResult<T> {
    fn into_result(self) -> ZxResult<T> {
        self
    }
}

/// The thread state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ThreadState {
    /// The thread has been created but it has not started running yet.
    New = 0,
    /// The thread is running user code normally.
    Running = 1,
    /// Stopped due to `zx_task_suspend()`.
    Suspended = 2,
    /// In a syscall or handling an exception.
    Blocked = 3,
    /// The thread is in the process of being terminated, but it has not been stopped yet.
    Dying = 4,
    /// The thread has stopped running.
    Dead = 5,
    /// The thread is stopped in an exception.
    BlockedException = 0x103,
    /// The thread is stopped in `zx_nanosleep()`.
    BlockedSleeping = 0x203,
    /// The thread is stopped in `zx_futex_wait()`.
    BlockedFutex = 0x303,
    /// The thread is stopped in `zx_port_wait()`.
    BlockedPort = 0x403,
    /// The thread is stopped in `zx_channel_call()`.
    BlockedChannel = 0x503,
    /// The thread is stopped in `zx_object_wait_one()`.
    BlockedWaitOne = 0x603,
    /// The thread is stopped in `zx_object_wait_many()`.
    BlockedWaitMany = 0x703,
    /// The thread is stopped in `zx_interrupt_wait()`.
    BlockedInterrupt = 0x803,
    /// Pager.
    BlockedPager = 0x903,
}

impl Default for ThreadState {
    fn default() -> Self {
        ThreadState::New
    }
}

/// The thread information.
#[repr(C)]
pub struct ThreadInfo {
    state: u32,
    wait_exception_channel_type: u32,
    cpu_affinity_mask: [u64; 8],
}

#[cfg(test)]
mod tests {
    use super::job::Job;
    use super::*;
    use kernel_hal::timer::timer_now;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");
        assert_eq!(thread.flags(), ThreadFlag::empty());

        assert_eq!(thread.related_koid(), proc.id());
        let child = proc.get_child(thread.id()).unwrap().downcast_arc().unwrap();
        assert!(Arc::ptr_eq(&child, &thread));
    }

    #[async_std::test]
    async fn start() {
        kernel_hal::init();
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");
        let thread1 = Thread::create(&proc, "thread1").expect("failed to create thread");

        // function for new thread
        async fn new_thread(thread: CurrentThread) {
            let cx = thread.wait_for_run().await;
            assert_eq!(cx.general.rip, 1);
            assert_eq!(cx.general.rsp, 4);
            assert_eq!(cx.general.rdi, 3);
            assert_eq!(cx.general.rsi, 2);
            async_std::task::sleep(Duration::from_millis(10)).await;
            thread.end_running(cx);
        }

        // start a new thread
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        proc.start(&thread, 1, 4, Some(handle.clone()), 2, |thread| {
            Box::pin(new_thread(thread))
        })
        .expect("failed to start thread");

        // check info and state
        let info = proc.get_info();
        assert!(info.started && !info.has_exited && info.return_code == 0);
        assert_eq!(proc.status(), Status::Running);
        assert_eq!(thread.state(), ThreadState::Running);

        // start again should fail
        assert_eq!(
            proc.start(&thread, 1, 4, Some(handle.clone()), 2, |thread| Box::pin(
                new_thread(thread)
            )),
            Err(ZxError::BAD_STATE)
        );

        // start another thread should fail
        assert_eq!(
            proc.start(&thread1, 1, 4, Some(handle.clone()), 2, |thread| Box::pin(
                new_thread(thread)
            )),
            Err(ZxError::BAD_STATE)
        );

        // wait 100ms for the new thread to exit
        async_std::task::sleep(core::time::Duration::from_millis(100)).await;

        // no other references to `Thread`
        assert_eq!(Arc::strong_count(&thread), 1);
        assert_eq!(thread.state(), ThreadState::Dead);
    }

    #[async_std::test]
    async fn blocking_run() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");
        let thread = CurrentThread(thread);

        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        let handle_value = proc.add_handle(handle);
        let object = proc
            .get_dyn_object_with_rights(handle_value, Rights::WAIT)
            .unwrap();

        let cancel_token = proc.get_cancel_token(handle_value).unwrap();
        let future = object.wait_signal(Signal::READABLE);
        let deadline = timer_now() + Duration::from_millis(20);
        let result = thread
            .blocking_run(
                future,
                ThreadState::BlockedWaitOne,
                deadline.into(),
                Some(cancel_token),
            )
            .await;
        assert_eq!(result.err(), Some(ZxError::TIMED_OUT));

        let cancel_token = proc.get_cancel_token(handle_value).unwrap();
        let future = object.wait_signal(Signal::READABLE);
        let deadline = timer_now() + Duration::from_millis(20);
        async_std::task::spawn({
            let proc = proc.clone();
            async move {
                async_std::task::sleep(Duration::from_millis(10)).await;
                proc.remove_handle(handle_value).unwrap();
            }
        });
        let result = thread
            .blocking_run(
                future,
                ThreadState::BlockedWaitOne,
                deadline.into(),
                Some(cancel_token),
            )
            .await;
        assert_eq!(result.err(), Some(ZxError::CANCELED));
    }

    #[test]
    fn info() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        let info = thread.get_thread_info();
        assert!(info.state == thread.state() as u32 && info.wait_exception_channel_type == 0);
        assert_eq!(
            thread.get_thread_exception_info().err(),
            Some(ZxError::BAD_STATE)
        );
    }

    #[test]
    fn read_write_state() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        const SIZE: usize = core::mem::size_of::<GeneralRegs>();
        let mut buf = [0; 10];
        assert_eq!(
            thread.read_state(ThreadStateKind::General, &mut buf).err(),
            Some(ZxError::BAD_STATE)
        );
        assert_eq!(
            thread.write_state(ThreadStateKind::General, &buf).err(),
            Some(ZxError::BAD_STATE)
        );

        thread.suspend();

        assert_eq!(
            thread.read_state(ThreadStateKind::General, &mut buf).err(),
            Some(ZxError::BUFFER_TOO_SMALL)
        );
        assert_eq!(
            thread.write_state(ThreadStateKind::General, &buf).err(),
            Some(ZxError::BUFFER_TOO_SMALL)
        );

        let mut buf = [0; SIZE];
        assert!(thread
            .read_state(ThreadStateKind::General, &mut buf)
            .is_ok());
        assert!(thread.write_state(ThreadStateKind::General, &buf).is_ok());
        // TODO
    }

    #[test]
    fn ext() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        let _ext = thread.ext();
        // TODO
    }

    #[async_std::test]
    async fn wait_for_run() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        assert_eq!(thread.state(), ThreadState::New);

        thread
            .start(0, 0, 0, 0, |thread| Box::pin(new_thread(thread)))
            .unwrap();
        async fn new_thread(thread: CurrentThread) {
            assert_eq!(thread.state(), ThreadState::Running);

            // without suspend
            let context = thread.wait_for_run().await;
            thread.end_running(context);

            // with suspend
            thread.suspend();
            thread.suspend();
            assert_eq!(thread.state(), ThreadState::Suspended);
            async_std::task::spawn({
                let thread = (*thread).clone();
                async move {
                    async_std::task::sleep(Duration::from_millis(10)).await;
                    thread.resume();
                    async_std::task::sleep(Duration::from_millis(10)).await;
                    thread.resume();
                }
            });
            let time = timer_now();
            let _context = thread.wait_for_run().await;
            assert!(timer_now() - time >= Duration::from_millis(20));
        }
        let thread: Arc<dyn KernelObject> = thread;
        thread.wait_signal(Signal::THREAD_TERMINATED).await;
    }

    #[test]
    fn time() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        assert_eq!(thread.get_time(), 0);
        thread.time_add(10);
        assert_eq!(thread.get_time(), 10);
    }
}

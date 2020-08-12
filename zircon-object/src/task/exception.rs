#![allow(dead_code)]

use {
    super::*, crate::ipc::*, crate::object::*, alloc::sync::Arc, alloc::vec, alloc::vec::Vec,
    core::mem::size_of, core::time::Duration, futures::channel::oneshot, futures::pin_mut,
    kernel_hal::UserContext, spin::Mutex,
};

/// Kernel-owned exception channel endpoint.
pub struct Exceptionate {
    type_: ExceptionChannelType,
    inner: Mutex<ExceptionateInner>,
}

struct ExceptionateInner {
    channel: Option<Arc<Channel>>,
    thread_rights: Rights,
    process_rights: Rights,
    shutdowned: bool,
}

impl Exceptionate {
    /// Create an `Exceptionate`.
    pub fn new(type_: ExceptionChannelType) -> Arc<Self> {
        Arc::new(Exceptionate {
            type_,
            inner: Mutex::new(ExceptionateInner {
                channel: None,
                thread_rights: Rights::empty(),
                process_rights: Rights::empty(),
                shutdowned: false,
            }),
        })
    }

    /// Shutdown the exceptionate.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock();
        inner.channel.take();
        inner.shutdowned = true;
    }

    /// Create an exception channel endpoint for user.
    pub fn create_channel(
        &self,
        thread_rights: Rights,
        process_rights: Rights,
    ) -> ZxResult<Arc<Channel>> {
        let mut inner = self.inner.lock();
        if inner.shutdowned {
            return Err(ZxError::BAD_STATE);
        }
        if let Some(channel) = inner.channel.as_ref() {
            if channel.peer().is_ok() {
                // already has a valid channel
                return Err(ZxError::ALREADY_BOUND);
            }
        }
        let (sender, receiver) = Channel::create();
        inner.channel.replace(sender);
        inner.process_rights = process_rights;
        inner.thread_rights = thread_rights;
        Ok(receiver)
    }

    pub(super) fn has_channel(&self) -> bool {
        let mut inner = self.inner.lock();
        if let Some(channel) = inner.channel.as_ref() {
            if channel.peer().is_ok() {
                return true;
            } else {
                inner.channel.take();
            }
        }
        false
    }

    /// Send exception to the user-owned endpoint.
    pub fn send_exception(&self, exception: &Arc<Exception>) -> ZxResult<oneshot::Receiver<()>> {
        debug!(
            "Exception: {:?} ,try send to {:?}",
            exception.type_, self.type_
        );
        let mut inner = self.inner.lock();
        let channel = inner.channel.as_ref().ok_or(ZxError::NEXT)?;
        let info = ExceptionInfo {
            pid: exception.thread.proc().id(),
            tid: exception.thread.id(),
            type_: exception.type_,
            padding: Default::default(),
        };
        let (sender, receiver) = oneshot::channel::<()>();
        let object = ExceptionObject::create(exception.clone(), sender);
        let handle = Handle::new(object, Rights::DEFAULT_EXCEPTION);
        let msg = MessagePacket {
            data: info.pack(),
            handles: vec![handle],
        };
        exception.set_rights(inner.thread_rights, inner.process_rights);
        channel.write(msg).map_err(|err| {
            if err == ZxError::PEER_CLOSED {
                inner.channel.take();
                return ZxError::NEXT;
            }
            err
        })?;
        Ok(receiver)
    }
}

#[repr(C)]
struct ExceptionInfo {
    tid: KoID,
    pid: KoID,
    type_: ExceptionType,
    padding: u32,
}

impl ExceptionInfo {
    #[allow(unsafe_code)]
    pub fn pack(&self) -> Vec<u8> {
        let buf: [u8; size_of::<ExceptionInfo>()] = unsafe { core::mem::transmute_copy(self) };
        Vec::from(buf)
    }
}

/// The common header of all exception reports.
#[repr(C)]
#[derive(Clone)]
pub struct ExceptionHeader {
    /// The actual size, in bytes, of the report (including this field).
    pub size: u32,
    /// The type of the exception.
    pub type_: ExceptionType,
}

/// Data associated with an exception (siginfo in linux parlance)
/// Things available from regsets (e.g., pc) are not included here.
/// For an example list of things one might add, see linux siginfo.
#[allow(missing_docs)]
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Default, Clone)]
pub struct ExceptionContext {
    pub vector: u64,
    pub err_code: u64,
    pub cr2: u64,
}

/// Data associated with an exception (siginfo in linux parlance)
/// Things available from regsets (e.g., pc) are not included here.
/// For an example list of things one might add, see linux siginfo.
#[allow(missing_docs)]
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Default, Clone)]
pub struct ExceptionContext {
    pub esr: u32,
    pub padding1: u32,
    pub far: u64,
    pub padding2: u64,
}

impl ExceptionContext {
    #[cfg(target_arch = "x86_64")]
    fn from_user_context(cx: &UserContext) -> Self {
        ExceptionContext {
            vector: cx.trap_num as u64,
            err_code: cx.error_code as u64,
            cr2: kernel_hal::fetch_fault_vaddr() as u64,
        }
    }
    #[cfg(target_arch = "aarch64")]
    fn from_user_context(_cx: &UserContext) -> Self {
        unimplemented!()
    }
}

/// Data reported to an exception handler for most exceptions.
#[repr(C)]
#[derive(Clone)]
pub struct ExceptionReport {
    /// The common header of all exception reports.
    pub header: ExceptionHeader,
    /// Exception-specific data.
    pub context: ExceptionContext,
}

impl ExceptionReport {
    fn new(type_: ExceptionType, cx: Option<&UserContext>) -> Self {
        ExceptionReport {
            header: ExceptionHeader {
                type_,
                size: core::mem::size_of::<ExceptionReport>() as u32,
            },
            context: cx
                .map(ExceptionContext::from_user_context)
                .unwrap_or_default(),
        }
    }
}

/// Type of exception
#[allow(missing_docs)]
#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum ExceptionType {
    General = 0x008,
    FatalPageFault = 0x108,
    UndefinedInstruction = 0x208,
    SoftwareBreakpoint = 0x308,
    HardwareBreakpoint = 0x408,
    UnalignedAccess = 0x508,
    // exceptions generated by kernel instead of the hardware
    Synth = 0x8000,
    ThreadStarting = 0x8008,
    ThreadExiting = 0x8108,
    PolicyError = 0x8208,
    ProcessStarting = 0x8308,
}

/// Type of the exception channel
#[allow(missing_docs)]
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ExceptionChannelType {
    None = 0,
    Debugger = 1,
    Thread = 2,
    Process = 3,
    Job = 4,
    JobDebugger = 5,
}

/// This will be transmitted to registered exception handlers in userspace
/// and provides them with exception state and control functionality.
/// We do not send exception directly since it's hard to figure out
/// when will the handle close.
pub struct ExceptionObject {
    base: KObjectBase,
    exception: Arc<Exception>,
    close_signal: Option<oneshot::Sender<()>>,
}

impl_kobject!(ExceptionObject);

impl ExceptionObject {
    /// Create an `ExceptionObject`.
    fn create(exception: Arc<Exception>, close_signal: oneshot::Sender<()>) -> Arc<Self> {
        Arc::new(ExceptionObject {
            base: KObjectBase::new(),
            exception,
            close_signal: Some(close_signal),
        })
    }
    /// Get the exception.
    pub fn get_exception(&self) -> &Arc<Exception> {
        &self.exception
    }
}

impl Drop for ExceptionObject {
    fn drop(&mut self) {
        self.close_signal
            .take()
            .and_then(|signal| signal.send(()).ok());
    }
}

/// An Exception represents a single currently-active exception.
pub struct Exception {
    thread: Arc<Thread>,
    type_: ExceptionType,
    report: ExceptionReport,
    inner: Mutex<ExceptionInner>,
}

struct ExceptionInner {
    current_channel_type: ExceptionChannelType,
    // Task rights copied from Exceptionate
    thread_rights: Rights,
    process_rights: Rights,
    handled: bool,
    second_chance: bool,
}

impl Exception {
    /// Create an `Exception`.
    pub fn create(
        thread: Arc<Thread>,
        type_: ExceptionType,
        cx: Option<&UserContext>,
    ) -> Arc<Self> {
        Arc::new(Exception {
            thread,
            type_,
            report: ExceptionReport::new(type_, cx),
            inner: Mutex::new(ExceptionInner {
                current_channel_type: ExceptionChannelType::None,
                thread_rights: Rights::DEFAULT_THREAD,
                process_rights: Rights::DEFAULT_PROCESS,
                handled: false,
                second_chance: false,
            }),
        })
    }
    /// Handle the exception. The return value indicate if the thread is exited after this.
    /// Note that it's possible that this may returns before exception was send to any exception channel
    /// This happens only when the thread is killed before we send the exception
    pub async fn handle(self: &Arc<Self>, fatal: bool) -> bool {
        self.handle_with_exceptionates(fatal, ExceptionateIterator::new(self), false)
            .await
    }

    /// Same as handle, but use a customed iterator
    /// If first_only is true, this will only send exception to the first one that received the exception
    /// even when the exception is not handled
    pub async fn handle_with_exceptionates(
        self: &Arc<Self>,
        fatal: bool,
        exceptionates: impl IntoIterator<Item = Arc<Exceptionate>>,
        first_only: bool,
    ) -> bool {
        self.thread.set_exception(Some(self.clone()));
        let future = self.handle_internal(exceptionates, first_only);
        pin_mut!(future);
        let result: ZxResult = self
            .thread
            .blocking_run(
                future,
                ThreadState::BlockedException,
                Duration::from_nanos(u64::max_value()),
            )
            .await;
        self.thread.set_exception(None);
        if let Err(err) = result {
            if err == ZxError::STOP {
                // We are killed
                return false;
            } else if err == ZxError::NEXT && fatal {
                // Nobody handled the exception, kill myself
                self.thread.proc().exit(TASK_RETCODE_SYSCALL_KILL);
                return false;
            }
        }
        true
    }

    async fn handle_internal(
        self: &Arc<Self>,
        exceptionates: impl IntoIterator<Item = Arc<Exceptionate>>,
        first_only: bool,
    ) -> ZxResult {
        for exceptionate in exceptionates.into_iter() {
            let closed = match exceptionate.send_exception(self) {
                Ok(receiver) => receiver,
                // This channel is not available now!
                Err(ZxError::NEXT) => continue,
                Err(err) => return Err(err),
            };
            self.inner.lock().current_channel_type = exceptionate.type_;
            // If this error, the sender is dropped, and the handle should also be closed.
            closed.await.ok();
            let handled = {
                let mut inner = self.inner.lock();
                inner.current_channel_type = ExceptionChannelType::None;
                inner.handled
            };
            if handled | first_only {
                return Ok(());
            }
        }
        Err(ZxError::NEXT)
    }

    /// Get the exception's thread and thread rights.
    pub fn get_thread_and_rights(&self) -> (Arc<Thread>, Rights) {
        (self.thread.clone(), self.inner.lock().thread_rights)
    }

    /// Get the exception's process and process rights.
    pub fn get_process_and_rights(&self) -> (Arc<Process>, Rights) {
        (self.thread.proc().clone(), self.inner.lock().process_rights)
    }

    /// Get the exception's channel type.
    pub fn get_current_channel_type(&self) -> ExceptionChannelType {
        self.inner.lock().current_channel_type
    }

    /// Get a report of the exception.
    pub fn get_report(&self) -> ExceptionReport {
        self.report.clone()
    }

    /// Get whether closing the exception handle will
    /// finish exception processing and resume the underlying thread.
    pub fn get_state(&self) -> u32 {
        self.inner.lock().handled as u32
    }

    /// Set whether closing the exception handle will
    /// finish exception processing and resume the underlying thread.
    pub fn set_state(&self, state: u32) -> ZxResult {
        if state > 1 {
            return Err(ZxError::INVALID_ARGS);
        }
        self.inner.lock().handled = state == 1;
        Ok(())
    }

    /// Get whether the debugger gets a 'second chance' at handling the exception
    /// if the process-level handler fails to do so.
    pub fn get_strategy(&self) -> u32 {
        self.inner.lock().second_chance as u32
    }

    /// Set whether the debugger gets a 'second chance' at handling the exception
    /// if the process-level handler fails to do so.
    pub fn set_strategy(&self, strategy: u32) -> ZxResult {
        if strategy > 1 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut inner = self.inner.lock();
        match inner.current_channel_type {
            ExceptionChannelType::Debugger | ExceptionChannelType::JobDebugger => {
                inner.second_chance = strategy == 1;
                Ok(())
            }
            _ => Err(ZxError::BAD_STATE),
        }
    }

    fn set_rights(&self, thread_rights: Rights, process_rights: Rights) {
        let mut inner = self.inner.lock();
        inner.thread_rights = thread_rights;
        inner.process_rights = process_rights;
    }
}

/// An iterator used to find Exceptionates used while handling the exception
/// This is only used to handle normal exceptions (Architectural & Policy)
/// We can use rust generator instead here but that is somehow not stable
/// Exception handlers are tried in the following order:
/// - process debugger
/// - thread
/// - process
/// - process debugger (in dealing with a second-chance exception)
/// - job (first owning job, then its parent job, and so on up to root job)
struct ExceptionateIterator<'a> {
    exception: &'a Exception,
    state: ExceptionateIteratorState,
}

/// The state used in ExceptionateIterator.
/// Name of options is what to consider next
enum ExceptionateIteratorState {
    Debug(bool),
    Thread,
    Process,
    Job(Arc<Job>),
    Finished,
}

impl<'a> ExceptionateIterator<'a> {
    fn new(exception: &'a Exception) -> Self {
        ExceptionateIterator {
            exception,
            state: ExceptionateIteratorState::Debug(false),
        }
    }
}

impl<'a> Iterator for ExceptionateIterator<'a> {
    type Item = Arc<Exceptionate>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &self.state {
                ExceptionateIteratorState::Debug(second_chance) => {
                    if *second_chance && !self.exception.inner.lock().second_chance {
                        self.state =
                            ExceptionateIteratorState::Job(self.exception.thread.proc().job());
                        continue;
                    }
                    let proc = self.exception.thread.proc();
                    self.state = if *second_chance {
                        ExceptionateIteratorState::Job(self.exception.thread.proc().job())
                    } else {
                        ExceptionateIteratorState::Thread
                    };
                    return Some(proc.get_debug_exceptionate());
                }
                ExceptionateIteratorState::Thread => {
                    self.state = ExceptionateIteratorState::Process;
                    return Some(self.exception.thread.get_exceptionate());
                }
                ExceptionateIteratorState::Process => {
                    let proc = self.exception.thread.proc();
                    self.state = ExceptionateIteratorState::Debug(true);
                    return Some(proc.get_exceptionate());
                }
                ExceptionateIteratorState::Job(job) => {
                    let parent = job.parent();
                    let result = job.get_exceptionate();
                    self.state = parent.map_or(
                        ExceptionateIteratorState::Finished,
                        ExceptionateIteratorState::Job,
                    );
                    return Some(result);
                }
                ExceptionateIteratorState::Finished => return None,
            }
        }
    }
}

/// This is only used by ProcessStarting exceptions
pub struct JobDebuggerIterator {
    job: Option<Arc<Job>>,
}

impl JobDebuggerIterator {
    /// Create a new JobDebuggerIterator
    pub fn new(job: Arc<Job>) -> Self {
        JobDebuggerIterator { job: Some(job) }
    }
}

impl Iterator for JobDebuggerIterator {
    type Item = Arc<Exceptionate>;
    fn next(&mut self) -> Option<Self::Item> {
        let result = self.job.as_ref().map(|job| job.get_debug_exceptionate());
        self.job = self.job.as_ref().and_then(|job| job.parent());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exceptionate_iterator() {
        let parent_job = Job::root();
        let job = parent_job.create_child(0).unwrap();
        let proc = Process::create(&job, "proc", 0).unwrap();
        let thread = Thread::create(&proc, "thread", 0).unwrap();

        let exception = Exception::create(thread.clone(), ExceptionType::Synth, None);
        let mut iterator = ExceptionateIterator::new(&exception);

        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &proc.get_debug_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &thread.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &proc.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &job.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &parent_job.get_exceptionate()
        ));
        assert!(iterator.next().is_none());
    }

    #[test]
    fn exceptionate_iterator_second_chance() {
        let parent_job = Job::root();
        let job = parent_job.create_child(0).unwrap();
        let proc = Process::create(&job, "proc", 0).unwrap();
        let thread = Thread::create(&proc, "thread", 0).unwrap();

        let exception = Exception::create(thread.clone(), ExceptionType::Synth, None);
        let mut iterator = ExceptionateIterator::new(&exception);

        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &proc.get_debug_exceptionate()
        ));
        exception.inner.lock().second_chance = true;
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &thread.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &proc.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &proc.get_debug_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &job.get_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &parent_job.get_exceptionate()
        ));
        assert!(iterator.next().is_none());
    }

    #[test]
    fn job_debugger_iterator() {
        let parent_job = Job::root();
        let job = parent_job.create_child(0).unwrap();
        let child_job = job.create_child(0).unwrap();
        let _grandson_job = child_job.create_child(0).unwrap();

        let mut iterator = JobDebuggerIterator::new(child_job.clone());

        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &child_job.get_debug_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &job.get_debug_exceptionate()
        ));
        assert!(Arc::ptr_eq(
            &iterator.next().unwrap(),
            &parent_job.get_debug_exceptionate()
        ));
        assert!(iterator.next().is_none());
    }

    #[async_std::test]
    async fn exception_handling() {
        let parent_job = Job::root();
        let job = parent_job.create_child(0).unwrap();
        let proc = Process::create(&job, "proc", 0).unwrap();
        let thread = Thread::create(&proc, "thread", 0).unwrap();

        let exception = Exception::create(thread.clone(), ExceptionType::Synth, None);

        // This is used to verify that exceptions are handled in a specific order
        let handled_order = Arc::new(Mutex::new(Vec::<usize>::new()));

        let create_handler = |exceptionate: &Arc<Exceptionate>,
                              should_receive: bool,
                              should_handle: bool,
                              order: usize| {
            let channel = exceptionate
                .create_channel(Rights::DEFAULT_THREAD, Rights::DEFAULT_PROCESS)
                .unwrap();
            let handled_order = handled_order.clone();

            async_std::task::spawn(async move {
                // wait for the channel is ready
                let channel_object: Arc<dyn KernelObject> = channel.clone();
                channel_object
                    .wait_signal(Signal::READABLE | Signal::PEER_CLOSED)
                    .await;

                if !should_receive {
                    // channel should be closed without message
                    let read_result = channel.read();
                    if let Err(err) = read_result {
                        assert_eq!(err, ZxError::PEER_CLOSED);
                    } else {
                        unreachable!();
                    }
                    return;
                }

                // we should get the exception here
                let data = channel.read().unwrap();
                assert_eq!(data.handles.len(), 1);
                let exception = data.handles[0]
                    .object
                    .clone()
                    .downcast_arc::<ExceptionObject>()
                    .unwrap();
                if should_handle {
                    exception.get_exception().set_state(1).unwrap();
                }
                // record the order of the handler used
                handled_order.lock().push(order);
            })
        };

        // proc debug should get the exception first
        create_handler(&proc.get_debug_exceptionate(), true, false, 0);
        // thread should get the exception next
        create_handler(&thread.get_exceptionate(), true, false, 1);
        // here we omit proc to test that we can handle the case that there is none handler
        // job should get the exception and handle it next
        create_handler(&job.get_exceptionate(), true, true, 3);
        // since exception is handled we should not get it from parent job
        create_handler(&parent_job.get_exceptionate(), false, false, 4);

        exception.handle(false).await;

        // terminate handlers by shutdown the related exceptionates
        thread.get_exceptionate().shutdown();
        proc.get_debug_exceptionate().shutdown();
        job.get_exceptionate().shutdown();
        parent_job.get_exceptionate().shutdown();

        // test for the order: proc debug -> thread -> job
        assert_eq!(handled_order.lock().clone(), vec![0, 1, 3]);
    }
}

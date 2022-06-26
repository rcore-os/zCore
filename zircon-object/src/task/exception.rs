use alloc::{sync::Arc, vec::Vec};
use core::mem::size_of;

use futures::channel::oneshot;
use kernel_hal::context::{TrapReason, UserContext};
use lock::Mutex;

use super::{Job, Task, Thread};
use crate::ipc::{Channel, MessagePacket};
use crate::object::{Handle, KObjectBase, KernelObject, KoID, Rights, Signal};
use crate::{impl_kobject, ZxError, ZxResult};

/// Kernel-owned exception channel endpoint.
pub struct Exceptionate {
    type_: ExceptionChannelType,
    inner: Mutex<ExceptionateInner>,
}

enum ExceptionateInner {
    Init,
    Bind {
        channel: Arc<Channel>,
        rights: Rights,
    },
    Shutdown,
}

impl Exceptionate {
    /// Create an `Exceptionate`.
    pub(super) fn new(type_: ExceptionChannelType) -> Arc<Self> {
        Arc::new(Exceptionate {
            type_,
            inner: Mutex::new(ExceptionateInner::Init),
        })
    }

    /// Shutdown the exceptionate.
    pub(super) fn shutdown(&self) {
        *self.inner.lock() = ExceptionateInner::Shutdown;
    }

    /// Create an exception channel endpoint for user.
    pub fn create_channel(&self, rights: Rights) -> ZxResult<Arc<Channel>> {
        let mut inner = self.inner.lock();
        match &*inner {
            ExceptionateInner::Shutdown => return Err(ZxError::BAD_STATE),
            ExceptionateInner::Bind { channel, .. } if channel.peer().is_ok() => {
                // already has a valid channel
                return Err(ZxError::ALREADY_BOUND);
            }
            _ => {}
        }
        let (channel, user_channel) = Channel::create();
        *inner = ExceptionateInner::Bind { channel, rights };
        Ok(user_channel)
    }

    /// Whether the user-owned channel endpoint is alive.
    pub(super) fn has_channel(&self) -> bool {
        let inner = self.inner.lock();
        matches!(&*inner, ExceptionateInner::Bind { channel, .. } if channel.peer().is_ok())
    }

    /// Send exception to the user-owned endpoint.
    pub(super) fn send_exception(
        &self,
        exception: &Arc<Exception>,
    ) -> ZxResult<oneshot::Receiver<()>> {
        debug!(
            "Exception: {:?} ,try send to {:?}",
            exception.type_, self.type_
        );
        let mut inner = self.inner.lock();
        let (channel, rights) = match &*inner {
            ExceptionateInner::Bind { channel, rights } => (channel, *rights),
            _ => return Err(ZxError::NEXT),
        };
        let info = ExceptionInfo {
            pid: exception.thread.proc().id(),
            tid: exception.thread.id(),
            type_: exception.type_,
            padding: Default::default(),
        };
        let (object, closed) = ExceptionObject::create(exception.clone(), rights);
        let msg = MessagePacket {
            data: info.pack(),
            handles: alloc::vec![Handle::new(object, Rights::DEFAULT_EXCEPTION)],
        };
        channel.write(msg).map_err(|err| {
            if err == ZxError::PEER_CLOSED {
                *inner = ExceptionateInner::Init;
                return ZxError::NEXT;
            }
            err
        })?;
        Ok(closed)
    }
}

#[repr(C)]
#[derive(Debug)]
struct ExceptionInfo {
    pid: KoID,
    tid: KoID,
    type_: ExceptionType,
    padding: u32,
}

impl ExceptionInfo {
    #[allow(unsafe_code)]
    fn pack(&self) -> Vec<u8> {
        let buf: [u8; size_of::<ExceptionInfo>()] = unsafe { core::mem::transmute_copy(self) };
        Vec::from(buf)
    }
}

/// The common header of all exception reports.
#[repr(C)]
#[derive(Debug, Clone)]
struct ExceptionHeader {
    /// The actual size, in bytes, of the report (including this field).
    size: u32,
    /// The type of the exception.
    type_: ExceptionType,
}

/// Data associated with an exception (siginfo in linux parlance)
/// Things available from regsets (e.g., pc) are not included here.
/// For an example list of things one might add, see linux siginfo.
#[repr(transparent)]
#[derive(Debug, Default, Clone)]
struct ExceptionContext(ExceptionContextInner);

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[repr(C)]
        #[derive(Debug, Default, Clone)]
        struct ExceptionContextInner {
            vector: u64,
            err_code: u64,
            cr2: u64,
        }
    } else if #[cfg(target_arch = "aarch64")] {
        #[repr(C)]
        #[derive(Debug, Default, Clone)]
        struct ExceptionContextInner {
            esr: u32,
            _padding1: u32,
            far: u64,
            _padding2: u64,
        }
    } else if #[cfg(target_arch = "riscv64")] {
        #[repr(C)]
        #[derive(Debug, Default, Clone)]
        struct ExceptionContextInner {
            scause: u64,
            stval: u64,
            _padding: u64,
        }
    }
}

impl ExceptionContext {
    fn from_user_context(ctx: &UserContext) -> Self {
        let fault_vaddr = if let TrapReason::PageFault(vaddr, _) = ctx.trap_reason() {
            vaddr as u64
        } else {
            return Default::default();
        };
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                Self(ExceptionContextInner {
                    vector: ctx.raw_trap_reason() as _,
                    err_code: ctx.error_code() as _,
                    cr2: fault_vaddr,
                })
            } else if #[cfg(target_arch = "aarch64")] {
                Self(ExceptionContextInner {
                    esr: ctx.raw_trap_reason() as _,
                    far: fault_vaddr,
                    ..Default::default()
                })
            } else if #[cfg(target_arch = "riscv64")] {
                Self(ExceptionContextInner {
                    scause: ctx.raw_trap_reason() as _,
                    stval: fault_vaddr,
                    ..Default::default()
                })
            }
        }
    }
}

/// Data reported to an exception handler for most exceptions.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ExceptionReport {
    /// The common header of all exception reports.
    header: ExceptionHeader,
    /// Exception-specific data.
    context: ExceptionContext,
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
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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

impl ExceptionType {
    /// Is the exception type generated by kernel instead of the hardware.
    pub fn is_synth(self) -> bool {
        (self as u32) & (ExceptionType::Synth as u32) != 0
    }
}

/// Type of the exception channel
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ExceptionChannelType {
    None = 0,
    Debugger = 1,
    Thread = 2,
    Process = 3,
    Job = 4,
    JobDebugger = 5,
}

/// The exception object received from the exception channel.
///
/// This will be transmitted to registered exception handlers in userspace
/// and provides them with exception state and control functionality.
/// We do not send exception directly since it's hard to figure out
/// when will the handle close.
pub struct ExceptionObject {
    base: KObjectBase,
    exception: Arc<Exception>,
    /// Task rights copied from `Exceptionate`.
    rights: Rights,
    close_signal: Option<oneshot::Sender<()>>,
}

impl_kobject!(ExceptionObject);

impl ExceptionObject {
    /// Create an kernel object of `Exception`.
    ///
    /// Return the object and a `Receiver` of the object dropped event.
    fn create(exception: Arc<Exception>, rights: Rights) -> (Arc<Self>, oneshot::Receiver<()>) {
        let (sender, receiver) = oneshot::channel();
        let object = Arc::new(ExceptionObject {
            base: KObjectBase::new(),
            exception,
            rights,
            close_signal: Some(sender),
        });
        (object, receiver)
    }

    /// Create a handle for the exception's thread.
    pub fn get_thread_handle(&self) -> Handle {
        Handle {
            object: self.exception.thread.clone(),
            rights: self.rights & Rights::DEFAULT_THREAD,
        }
    }

    /// Create a handle for the exception's process.
    pub fn get_process_handle(&self) -> ZxResult<Handle> {
        if self.exception.current_channel_type() == ExceptionChannelType::Thread {
            return Err(ZxError::ACCESS_DENIED);
        }
        Ok(Handle {
            object: self.exception.thread.proc().clone(),
            rights: self.rights & Rights::DEFAULT_PROCESS,
        })
    }

    /// Get whether closing the exception handle will
    /// finish exception processing and resume the underlying thread.
    pub fn state(&self) -> u32 {
        self.exception.inner.lock().handled as u32
    }

    /// Set whether closing the exception handle will
    /// finish exception processing and resume the underlying thread.
    pub fn set_state(&self, state: u32) -> ZxResult {
        if state > 1 {
            return Err(ZxError::INVALID_ARGS);
        }
        self.exception.inner.lock().handled = state == 1;
        Ok(())
    }

    /// Get whether the debugger gets a 'second chance' at handling the exception
    /// if the process-level handler fails to do so.
    pub fn strategy(&self) -> u32 {
        self.exception.inner.lock().second_chance as u32
    }

    /// Set whether the debugger gets a 'second chance' at handling the exception
    /// if the process-level handler fails to do so.
    pub fn set_strategy(&self, strategy: u32) -> ZxResult {
        if strategy > 1 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut inner = self.exception.inner.lock();
        match inner.current_channel_type {
            ExceptionChannelType::Debugger | ExceptionChannelType::JobDebugger => {
                inner.second_chance = strategy == 1;
                Ok(())
            }
            _ => Err(ZxError::BAD_STATE),
        }
    }
}

impl Drop for ExceptionObject {
    fn drop(&mut self) {
        self.close_signal.take().unwrap().send(()).ok();
    }
}

/// An Exception represents a single currently-active exception.
pub(super) struct Exception {
    thread: Arc<Thread>,
    type_: ExceptionType,
    report: ExceptionReport,
    inner: Mutex<ExceptionInner>,
}

struct ExceptionInner {
    current_channel_type: ExceptionChannelType,
    handled: bool,
    second_chance: bool,
}

impl Exception {
    /// Create an `Exception`.
    pub fn new(thread: &Arc<Thread>, type_: ExceptionType, cx: Option<&UserContext>) -> Arc<Self> {
        Arc::new(Exception {
            thread: thread.clone(),
            type_,
            report: ExceptionReport::new(type_, cx),
            inner: Mutex::new(ExceptionInner {
                current_channel_type: ExceptionChannelType::None,
                handled: false,
                second_chance: false,
            }),
        })
    }

    /// Handle the exception.
    ///
    /// Note that it's possible that this may returns before exception was send to any exception channel.
    /// This happens only when the thread is killed before we send the exception.
    pub async fn handle(self: &Arc<Self>) {
        let result = match self.type_ {
            ExceptionType::ProcessStarting => {
                self.handle_with(JobDebuggerIterator::new(self.thread.proc().job()), true)
                    .await
            }
            ExceptionType::ThreadStarting | ExceptionType::ThreadExiting => {
                self.handle_with(Some(self.thread.proc().debug_exceptionate()), false)
                    .await
            }
            _ => {
                self.handle_with(ExceptionateIterator::new(self), false)
                    .await
            }
        };
        if result == Err(ZxError::NEXT) && !self.type_.is_synth() {
            // Nobody handled the exception, kill myself
            self.thread.proc().exit(super::TASK_RETCODE_SYSCALL_KILL);
        }
    }

    /// Handle the exception with a customized iterator.
    ///
    /// If `first_only` is true, this will only send exception to the first one that received the exception
    /// even when the exception is not handled.
    async fn handle_with(
        self: &Arc<Self>,
        exceptionates: impl IntoIterator<Item = Arc<Exceptionate>>,
        first_only: bool,
    ) -> ZxResult {
        for exceptionate in exceptionates.into_iter() {
            let closed = match exceptionate.send_exception(self) {
                // This channel is not available now!
                Err(ZxError::NEXT) => continue,
                res => res?,
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

    /// Get the exception's channel type.
    pub fn current_channel_type(&self) -> ExceptionChannelType {
        self.inner.lock().current_channel_type
    }

    /// Get a report of the exception.
    pub fn report(&self) -> ExceptionReport {
        self.report.clone()
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
                    return Some(proc.debug_exceptionate());
                }
                ExceptionateIteratorState::Thread => {
                    self.state = ExceptionateIteratorState::Process;
                    return Some(self.exception.thread.exceptionate());
                }
                ExceptionateIteratorState::Process => {
                    let proc = self.exception.thread.proc();
                    self.state = ExceptionateIteratorState::Debug(true);
                    return Some(proc.exceptionate());
                }
                ExceptionateIteratorState::Job(job) => {
                    let parent = job.parent();
                    let result = job.exceptionate();
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
struct JobDebuggerIterator {
    job: Option<Arc<Job>>,
}

impl JobDebuggerIterator {
    /// Create a new JobDebuggerIterator
    fn new(job: Arc<Job>) -> Self {
        JobDebuggerIterator { job: Some(job) }
    }
}

impl Iterator for JobDebuggerIterator {
    type Item = Arc<Exceptionate>;
    fn next(&mut self) -> Option<Self::Item> {
        let result = self.job.as_ref().map(|job| job.debug_exceptionate());
        self.job = self.job.as_ref().and_then(|job| job.parent());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::*;

    #[test]
    fn exceptionate_iterator() {
        let parent_job = Job::root();
        let job = parent_job.create_child().unwrap();
        let proc = Process::create(&job, "proc").unwrap();
        let thread = Thread::create(&proc, "thread").unwrap();

        let exception = Exception::new(&thread, ExceptionType::Synth, None);
        let actual: Vec<_> = ExceptionateIterator::new(&exception).collect();
        let expected = [
            proc.debug_exceptionate(),
            thread.exceptionate(),
            proc.exceptionate(),
            job.exceptionate(),
            parent_job.exceptionate(),
        ];
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!(Arc::ptr_eq(&actual, expected));
        }
    }

    #[test]
    fn exceptionate_iterator_second_chance() {
        let parent_job = Job::root();
        let job = parent_job.create_child().unwrap();
        let proc = Process::create(&job, "proc").unwrap();
        let thread = Thread::create(&proc, "thread").unwrap();

        let exception = Exception::new(&thread, ExceptionType::Synth, None);
        exception.inner.lock().second_chance = true;
        let actual: Vec<_> = ExceptionateIterator::new(&exception).collect();
        let expected = [
            proc.debug_exceptionate(),
            thread.exceptionate(),
            proc.exceptionate(),
            proc.debug_exceptionate(),
            job.exceptionate(),
            parent_job.exceptionate(),
        ];
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!(Arc::ptr_eq(&actual, expected));
        }
    }

    #[test]
    fn job_debugger_iterator() {
        let parent_job = Job::root();
        let job = parent_job.create_child().unwrap();
        let child_job = job.create_child().unwrap();
        let _grandson_job = child_job.create_child().unwrap();

        let actual: Vec<_> = JobDebuggerIterator::new(child_job.clone()).collect();
        let expected = [
            child_job.debug_exceptionate(),
            job.debug_exceptionate(),
            parent_job.debug_exceptionate(),
        ];
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!(Arc::ptr_eq(&actual, expected));
        }
    }

    #[async_std::test]
    async fn exception_handling() {
        let parent_job = Job::root();
        let job = parent_job.create_child().unwrap();
        let proc = Process::create(&job, "proc").unwrap();
        let thread = Thread::create(&proc, "thread").unwrap();

        let exception = Exception::new(&thread, ExceptionType::Synth, None);

        // This is used to verify that exceptions are handled in a specific order
        let handled_order = Arc::new(Mutex::new(Vec::<usize>::new()));

        let create_handler = |exceptionate: &Arc<Exceptionate>,
                              should_receive: bool,
                              should_handle: bool,
                              order: usize| {
            let channel = exceptionate
                .create_channel(Rights::DEFAULT_THREAD | Rights::DEFAULT_PROCESS)
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
                    assert_eq!(channel.read().err(), Some(ZxError::PEER_CLOSED));
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
                    exception.set_state(1).unwrap();
                }
                // record the order of the handler used
                handled_order.lock().push(order);
            })
        };

        // proc debug should get the exception first
        create_handler(&proc.debug_exceptionate(), true, false, 0);
        // thread should get the exception next
        create_handler(&thread.exceptionate(), true, false, 1);
        // here we omit proc to test that we can handle the case that there is none handler
        // job should get the exception and handle it next
        create_handler(&job.exceptionate(), true, true, 3);
        // since exception is handled we should not get it from parent job
        create_handler(&parent_job.exceptionate(), false, false, 4);

        exception.handle().await;

        // terminate handlers by shutdown the related exceptionates
        thread.exceptionate().shutdown();
        proc.debug_exceptionate().shutdown();
        job.exceptionate().shutdown();
        parent_job.exceptionate().shutdown();

        // test for the order: proc debug -> thread -> job
        assert_eq!(handled_order.lock().clone(), vec![0, 1, 3]);
    }
}

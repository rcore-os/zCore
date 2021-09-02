use {
    super::*,
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

/// Suspend the given task.
///
/// Currently only thread or process handles may be suspended.
///
/// # Example
/// ```
/// # use std::sync::Arc;
/// # use zircon_object::task::*;
/// # use zircon_object::object::{KernelObject, Signal};
/// # kernel_hal::init();
/// let job = Job::root();
/// let proc = Process::create(&job, "proc").unwrap();
/// let thread = Thread::create(&proc, "thread").unwrap();
///
/// // start the thread and never terminate
/// thread.start(0, 0, 0, 0, |thread| Box::pin(async move {
///     loop { async_std::task::yield_now().await }
///     let _ = thread;
/// })).unwrap();
///
/// // wait for the thread running
/// let object: Arc<dyn KernelObject> = thread.clone();
/// async_std::task::block_on(object.wait_signal(Signal::THREAD_RUNNING));
/// assert_eq!(thread.state(), ThreadState::Running);
///
/// // suspend the thread
/// {
///     let task: Arc<dyn Task> = thread.clone();
///     let suspend_token = SuspendToken::create(&task);
///     assert_eq!(thread.state(), ThreadState::Suspended);
/// }
/// // suspend token dropped, resume the thread
/// assert_eq!(thread.state(), ThreadState::Running);
/// ```
pub struct SuspendToken {
    base: KObjectBase,
    task: Weak<dyn Task>,
}

impl_kobject!(SuspendToken);

impl SuspendToken {
    /// Create a `SuspendToken` which can suspend the given task.
    pub fn create(task: &Arc<dyn Task>) -> Arc<Self> {
        task.suspend();
        Arc::new(SuspendToken {
            base: KObjectBase::new(),
            task: Arc::downgrade(task),
        })
    }
}

impl Drop for SuspendToken {
    fn drop(&mut self) {
        if let Some(task) = self.task.upgrade() {
            task.resume();
        }
    }
}

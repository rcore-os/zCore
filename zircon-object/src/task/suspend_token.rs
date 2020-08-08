use {
    // super::thread::Thread,
    super::*,
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

/// Suspend the given task. Currently only thread or process handles may be suspended.
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

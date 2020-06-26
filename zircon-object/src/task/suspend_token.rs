use {
    // super::thread::Thread,
    super::*,
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct SuspendToken {
    base: KObjectBase,
    task: Weak<dyn Task>,
}

impl_kobject!(SuspendToken);

impl SuspendToken {
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

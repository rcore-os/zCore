use {
    super::thread::Thread,
    super::*,
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct SuspendTask {
    base: KObjectBase,
    thread: Weak<Thread>,
}

impl_kobject!(SuspendTask);

impl SuspendTask {
    pub fn create(thread: &Arc<Thread>) -> Arc<Self> {
        Arc::new(SuspendTask {
            base: KObjectBase::new(),
            thread: {
                thread.suspend();
                Arc::downgrade(thread)
            },
        })
    }
}

impl Drop for SuspendTask {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.upgrade() {
            thread.resume();
        }
    }
}

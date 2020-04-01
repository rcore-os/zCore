use {
    super::thread::Thread,
    super::*,
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct SuspendToken {
    base: KObjectBase,
    thread: Weak<Thread>,
}

impl_kobject!(SuspendToken);

impl SuspendToken {
    pub fn create(thread: &Arc<Thread>) -> Arc<Self> {
        thread.suspend();
        Arc::new(SuspendToken {
            base: KObjectBase::new(),
            thread: Arc::downgrade(thread),
        })
    }
}

impl Drop for SuspendToken {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.upgrade() {
            thread.resume();
        }
    }
}

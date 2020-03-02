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
            base: {
                let mut res = KObjectBase::new();
                res.obj_type = OBJ_TYPE_SUSPEND_TOKEN;
                res
            },
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

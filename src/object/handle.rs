use super::rights::Rights;
use super::*;
use alloc::sync::Arc;
use spin::Mutex;

pub struct Handle {
    object: Arc<Mutex<dyn KernelObject>>,
    rights: Rights,
}

impl Handle {
    pub fn id(&self) -> KoID {
        self.object.lock().id()
    }

    pub fn do_mut<T: KernelObject, F: FnMut(&mut T) -> u64>(&self, mut f: F) -> u64 {
        let mut lock_object = self.object.lock();
        let obj = lock_object.downcast::<T>().unwrap();
        f(obj)
    }
}

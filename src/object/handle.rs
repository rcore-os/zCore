//use super::rights::Rights;
use super::*;
use alloc::sync::Arc;
use spin::Mutex;

pub struct Handle {
    object: Arc<Mutex<dyn KernelObject>>,
    rights: Rights,
}

impl Handle {
    pub fn new(object: Arc<Mutex<dyn KernelObject>>, rights: Rights) -> Self {
        Handle { object, rights }
    }

    pub fn id(&self) -> KoID {
        self.object.lock().id()
    }

    pub fn do_mut<T: KernelObject, F: FnMut(&mut T) -> ZxError>(&self, mut f: F) -> ZxError {
        let mut lock_object = self.object.lock();
        let obj = lock_object.downcast::<T>().unwrap();
        f(obj)
    }
}

//use super::rights::Rights;
use super::*;
use alloc::sync::Arc;
use spin::Mutex;

pub type HandleValue = u32;

#[derive(Clone)]
pub struct Handle {
    pub object: Arc<dyn KernelObject>,
    pub rights: Rights,
}

impl Handle {
    pub fn new(object: Arc<dyn KernelObject>, rights: Rights) -> Self {
        Handle { object, rights }
    }

    pub fn id(&self) -> KoID {
        self.object.id()
    }
}

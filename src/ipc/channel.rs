use crate::object::*;
use core::any::Any;
use alloc::sync::Arc;
use spin::Mutex;

pub struct Channel {
    koid: KoID,
}

impl Channel {
    fn new(koid: KoID) -> Self {
        Channel { koid }
    }

    pub fn id(&self) -> KoID {
        self.koid
    }
}

impl KernelObject for Channel {
    fn id(&self) -> KoID {
        self.koid
    }

    fn as_any(&mut self) -> &mut dyn Any{
        self
    }
}

pub fn create() -> (Handle, Handle) {
    let end0 = Channel::new(0);
    let end1 = Channel::new(1);
    let handle0 = Handle::new(Arc::new(Mutex::new(end0)), Rights::DUPLICATE);
    let handle1 = Handle::new(Arc::new(Mutex::new(end1)), Rights::DUPLICATE);
    (handle0, handle1)
}

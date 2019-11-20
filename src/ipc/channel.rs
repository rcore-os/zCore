use crate::object::*;
use alloc::sync::Arc;
use core::any::Any;
use spin::Mutex;

pub struct Channel {
    koid: KoID,
}

impl Channel {
    fn new(koid: KoID) -> Self {
        Channel { koid }
    }

    pub fn create() -> (Handle, Handle) {
        let end0 = Channel::new(0);
        let end1 = Channel::new(1);
        let handle0 = Handle::new(Arc::new(Mutex::new(end0)), Rights::DUPLICATE);
        let handle1 = Handle::new(Arc::new(Mutex::new(end1)), Rights::DUPLICATE);
        (handle0, handle1)
    }
}

impl KernelObject for Channel {
    fn id(&self) -> KoID {
        self.koid
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn is_work() {
        use crate::error::*;
        use crate::ipc::channel::Channel;
        use crate::object::handle::Handle;
        use crate::object::KernelObject;
        let (handle0, handle1) = Channel::create();
        handle0.do_mut(|ch: &mut Channel| {
            assert_eq!(0u64, ch.id());
            ZxError::OK
        });
        handle1.do_mut(|ch: &mut Channel| {
            assert_eq!(1u64, ch.id());
            ZxError::OK
        });
    }
}

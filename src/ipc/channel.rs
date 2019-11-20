use core::any::Any;
use crate::error::*;
use crate::object::{*, handle::Handle};

pub struct Channel {
    koid: KoID,
}

impl KernelObject for Channel {
    fn id(&self) -> KoID {
        self.koid
    }

    fn as_any(&mut self) -> &mut dyn Any{
        self
    }
}

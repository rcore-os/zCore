use core::any::Any;

pub use super::*;
pub use handle::*;
pub use rights::*;

pub mod handle;
pub mod rights;

pub trait KernelObject: Any + Sync + Send {
    fn id(&self) -> KoID;
    fn as_any(&mut self) -> &mut dyn Any;
}

impl dyn KernelObject {
    pub fn downcast<T: KernelObject>(&mut self) -> Option<&mut T> {
        self.as_any().downcast_mut::<T>()
    }
}

pub type KoID = u64;

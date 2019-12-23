use core::any::Any;
use core::sync::atomic::*;

pub use super::*;
pub use handle::*;
pub use rights::*;

pub mod handle;
pub mod rights;

pub trait KernelObject: Any + Sync + Send {
    fn id(&self) -> KoID;
    fn as_any(&self) -> &dyn Any;
}

impl dyn KernelObject {
    pub fn downcast_ref<T: KernelObject>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// The base struct of a kernel object
pub struct KObjectBase {
    pub id: KoID,
}

impl KObjectBase {
    pub fn new() -> Self {
        static KOID: AtomicU64 = AtomicU64::new(0);
        KObjectBase {
            id: KOID.fetch_add(1, Ordering::SeqCst),
        }
    }
}

#[macro_export]
macro_rules! impl_kobject {
    ($class:ident) => {
        impl crate::object::KernelObject for $class {
            fn id(&self) -> KoID {
                self.base.id
            }
            fn as_any(&self) -> &dyn core::any::Any {
                self
            }
        }
    };
}

pub type KoID = u64;

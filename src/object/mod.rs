use core::any::Any;
use core::fmt::Debug;
use core::sync::atomic::*;
use downcast_rs::{impl_downcast, DowncastSync};

pub use super::*;
pub use handle::*;
pub use rights::*;

pub mod handle;
pub mod rights;

pub trait KernelObject: DowncastSync + Debug {
    fn id(&self) -> KoID;
}

impl_downcast!(sync KernelObject);

/// The base struct of a kernel object
#[derive(Debug)]
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
        }
        impl core::fmt::Debug for $class {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
                write!(f, "{:?}", self.base)
            }
        }
    };
}

pub type KoID = u64;

//use super::rights::Rights;
use super::*;
use alloc::sync::Arc;

/// The value refers to a Handle in user space.
pub type HandleValue = u32;

/// A Handle is how a specific process refers to a specific kernel object.
#[derive(Debug, Clone)]
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

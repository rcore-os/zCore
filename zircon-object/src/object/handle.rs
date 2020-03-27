use {super::*, alloc::sync::Arc};

/// The value refers to a Handle in user space.
pub type HandleValue = u32;

/// Invalid handle value.
pub const INVALID_HANDLE: HandleValue = 0;

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

    pub fn get_info(&self) -> HandleBasicInfo {
        HandleBasicInfo {
            koid: self.object.id(),
            rights: self.rights.bits(),
            obj_type: self.object.obj_type() as u32,
            related_koid: self.object.related_koid(),
            props: if self.rights.contains(Rights::WAIT) {
                1
            } else {
                0
            },
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Default, Debug)]
pub struct HandleBasicInfo {
    koid: u64,
    rights: u32,
    obj_type: u32,
    related_koid: u64,
    props: u32,
    padding: u32,
}

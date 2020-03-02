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
        let mut ret = HandleBasicInfo {
            rights: self.rights.bits(),
            props: if self.rights.contains(Rights::WAIT) {
                1
            } else {
                0
            },
            ..Default::default()
        };
        self.object.get_info(&mut ret);
        ret
    }
}

#[repr(C)]
#[derive(Default)]
pub struct HandleBasicInfo {
    pub koid: u64,
    rights: u32,
    pub obj_type: u32,
    pub related_koid: u64,
    props: u32,
    padding: [u8; 4],
}

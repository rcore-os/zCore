use {super::*, alloc::sync::Arc};

/// The value refers to a Handle in user space.
pub type HandleValue = u32;

/// Invalid handle value.
pub const INVALID_HANDLE: HandleValue = 0;

/// A Handle is how a specific process refers to a specific kernel object.
#[derive(Debug, Clone)]
pub struct Handle {
    /// The object referred to by the handle.
    pub object: Arc<dyn KernelObject>,
    /// The handle's associated rights.
    pub rights: Rights,
}

impl Handle {
    /// Create a new handle referring to the given object with given rights.
    pub fn new(object: Arc<dyn KernelObject>, rights: Rights) -> Self {
        Handle { object, rights }
    }

    /// Get information about the provided handle and the object the handle refers to.
    pub fn get_info(&self) -> HandleBasicInfo {
        HandleBasicInfo {
            koid: self.object.id(),
            rights: self.rights.bits(),
            obj_type: obj_type(&self.object),
            related_koid: self.object.related_koid(),
            props: if self.rights.contains(Rights::WAIT) {
                1
            } else {
                0
            },
            padding: 0,
        }
    }

    /// Get information about the handle itself.
    ///
    /// The returned `HandleInfo`'s `handle` field should set manually.
    pub fn get_handle_info(&self) -> HandleInfo {
        HandleInfo {
            obj_type: obj_type(&self.object),
            rights: self.rights.bits(),
            ..Default::default()
        }
    }
}

/// Information about a handle and the object it refers to.
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

/// Get an object's type.
pub fn obj_type(object: &Arc<dyn KernelObject>) -> u32 {
    match object.type_name() {
        "Process" => 1,
        "Thread" => 2,
        "VmObject" => 3,
        "Channel" => 4,
        "Event" => 5,
        "Port" => 6,
        "Interrupt" => 9,
        "PciDevice" => 11,
        "Log" | "DebugLog" => 12,
        "Socket" => 14,
        "Resource" => 15,
        "EventPair" => 16,
        "Job" => 17,
        "VmAddressRegion" => 18,
        "Fifo" => 19,
        "Guest" => 20,
        "VCpu" => 21,
        "Timer" => 22,
        "Iommu" => 23,
        "Bti" => 24,
        "Profile" => 25,
        "Pmt" => 26,
        "SuspendToken" => 27,
        "Pager" => 28,
        "Exception" | "ExceptionObject" => 29,
        "Clock" => 30,
        "Stream" => 31,
        "PcieDeviceKObject" => 32,
        _ => unimplemented!("unknown type"),
    }
}

/// Information about a handle itself, including its `HandleValue`.
#[repr(C)]
#[derive(Default, Debug)]
pub struct HandleInfo {
    /// The handle's value in user space.
    pub handle: HandleValue,
    obj_type: u32,
    rights: u32,
    unused: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "unknown type")]
    fn test_ojb_type_unknown() {
        let obj: Arc<dyn KernelObject> = DummyObject::new();
        assert_eq!(1, obj_type(&obj));
    }

    #[test]
    fn test_get_info() {
        let obj = crate::task::Job::root();
        let handle1 = Handle::new(obj.clone(), Rights::DEFAULT_JOB);
        let info1 = handle1.get_info();
        assert_eq!(info1.obj_type, 17);
        assert_eq!(info1.props, 1);

        let handle_info = handle1.get_handle_info();
        assert_eq!(handle_info.obj_type, 17);

        let handle2 = Handle::new(obj, Rights::READ);
        let info2 = handle2.get_info();
        assert_eq!(info2.props, 0);

        // Let struct lines counted covered.
        // See https://github.com/mozilla/grcov/issues/450
        let _ = HandleBasicInfo::default();
    }
}

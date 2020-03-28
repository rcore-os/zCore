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

fn obj_type(object: &Arc<dyn KernelObject>) -> u32 {
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
        "Exception" => 29,
        "Clock" => 30,
        "Stream" => 31,
        _ => unimplemented!("unknown type"),
    }
}

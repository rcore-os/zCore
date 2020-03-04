use {crate::object::*, alloc::string::String, alloc::sync::Arc};

/// ResourceKind definition from fuchsia/zircon/system/public/zircon/syscalls/resource.h
#[repr(u32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ResourceKind {
    MMIO = 0,
    IRQ = 1,
    IOPORT = 2,
    HYPERVISOR = 3,
    ROOT = 4,
    VMEX = 5,
    SMC = 6,
    COUNT = 7,
}

/// Address space rights and accounting.
#[allow(dead_code)]
pub struct Resource {
    base: KObjectBase,
    name: String,
    kind: ResourceKind,
}

impl_kobject!(Resource);

impl Resource {
    /// Create a new `Resource`.
    pub fn create(name: &str, kind: ResourceKind) -> Arc<Self> {
        Arc::new(Resource {
            base: KObjectBase::new(),
            name: String::from(name),
            kind,
        })
    }

    pub fn validate(&self, kind: ResourceKind) -> ZxResult<()> {
        if self.kind == kind || self.kind == ResourceKind::ROOT {
            Ok(())
        } else {
            Err(ZxError::WRONG_TYPE)
        }
    }
}

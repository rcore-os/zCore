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

pub struct Resource {
    base: KObjectBase,
    name: String,
    kind: ResourceKind,
}

impl_kobject!(Resource);

impl Resource {
    pub fn create(name: &str, kind: ResourceKind) -> ZxResult<Arc<Self>> {
        Ok(Arc::new(Resource {
            base: KObjectBase::new(),
            name: String::from(name),
            kind: kind,
        }))
    }

    pub fn validate(&self, kind: ResourceKind) -> ZxResult<()> {
        return if self.kind == kind {
            Ok(())
        } else {
            Err(ZxError::WRONG_TYPE)
        };
    }
}

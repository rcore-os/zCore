use {crate::object::*, alloc::sync::Arc, bitflags::bitflags, numeric_enum_macro::numeric_enum};

numeric_enum! {
    #[repr(u32)]
    /// ResourceKind definition from fuchsia/zircon/system/public/zircon/syscalls/resource.h
    #[allow(missing_docs)]
    #[allow(clippy::upper_case_acronyms)]
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub enum ResourceKind {
        MMIO = 0,
        IRQ = 1,
        IOPORT = 2,
        ROOT = 3,
        SMC = 4,
        SYSTEM = 5,
        COUNT = 6,
    }
}

/// Subranges of the SYSTEM resource, from zircon/syscalls/resource.h.
#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum SystemResource {
    Hypervisor = 0,
    Vmex = 1,
    Debuglog = 12,
}

bitflags! {
    /// Bits for Resource.flags.
    pub struct ResourceFlags: u32 {
        #[allow(clippy::identity_op)]
        /// Exclusive resource.
        const EXCLUSIVE      = 1 << 16;
    }
}

/// Address space rights and accounting.
pub struct Resource {
    base: KObjectBase,
    kind: ResourceKind,
    addr: usize,
    len: usize,
    flags: ResourceFlags,
}

impl_kobject!(Resource);

impl Resource {
    /// Create a new `Resource`.
    pub fn create(
        name: &str,
        kind: ResourceKind,
        addr: usize,
        len: usize,
        flags: ResourceFlags,
    ) -> Arc<Self> {
        Arc::new(Resource {
            base: KObjectBase::with_name(name),
            kind,
            addr,
            len,
            flags,
        })
    }

    /// Validate the resource is the given kind or it is the root resource.
    pub fn validate(&self, kind: ResourceKind) -> ZxResult {
        if self.kind == kind || self.kind == ResourceKind::ROOT {
            Ok(())
        } else {
            Err(ZxError::WRONG_TYPE)
        }
    }

    /// Validate a SYSTEM capability for its specific purpose.
    pub fn validate_system(&self, resource: SystemResource) -> ZxResult {
        self.validate_ranged_resource(ResourceKind::SYSTEM, resource as usize, 1)
    }

    /// Validate the resource is the given kind or it is the root resource,
    /// and [addr, addr+len] is within the range of the resource.
    pub fn validate_ranged_resource(
        &self,
        kind: ResourceKind,
        addr: usize,
        len: usize,
    ) -> ZxResult {
        self.validate(kind)?;
        if addr >= self.addr && len <= self.len && addr - self.addr <= self.len - len {
            Ok(())
        } else {
            Err(ZxError::OUT_OF_RANGE)
        }
    }

    /// Returns `Err(ZxError::INVALID_ARGS)` if the resource is not the root resource, and
    /// either it's flags or parameter `flags` contains `ResourceFlags::EXCLUSIVE`.
    pub fn check_exclusive(&self, flags: ResourceFlags) -> ZxResult {
        if self.kind != ResourceKind::ROOT
            && (self.flags.contains(ResourceFlags::EXCLUSIVE)
                || flags.contains(ResourceFlags::EXCLUSIVE))
        {
            Err(ZxError::INVALID_ARGS)
        } else {
            Ok(())
        }
    }

    /// Get information of the resource.
    pub fn get_info(&self) -> ResourceInfo {
        let name = self.base.name();
        let name = name.as_bytes();
        let mut name_vec = [0u8; 32];
        name_vec[..name.len()].clone_from_slice(name);
        ResourceInfo {
            kind: self.kind as _,
            flags: self.flags.bits,
            base: self.addr as _,
            size: self.len as _,
            name: name_vec,
        }
    }
}

/// Information of a resource.
#[repr(C)]
#[derive(Default)]
pub struct ResourceInfo {
    kind: u32,
    flags: u32,
    base: u64,
    size: u64,
    name: [u8; 32], // should be [char; 32], but I cannot compile it
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_resources_are_scoped_to_their_subrange() {
        let debuglog = Resource::create(
            "debuglog",
            ResourceKind::SYSTEM,
            12,
            1,
            ResourceFlags::empty(),
        );
        assert!(debuglog.validate_system(SystemResource::Debuglog).is_ok());
        assert_eq!(
            debuglog.validate_system(SystemResource::Vmex),
            Err(ZxError::OUT_OF_RANGE)
        );
        assert_eq!(
            debuglog.validate_system(SystemResource::Hypervisor),
            Err(ZxError::OUT_OF_RANGE)
        );
        let root = Resource::create("root", ResourceKind::ROOT, 0, 16, ResourceFlags::empty());
        for purpose in [
            SystemResource::Vmex,
            SystemResource::Debuglog,
            SystemResource::Hypervisor,
        ] {
            assert!(root.validate_system(purpose).is_ok());
        }
        assert_eq!(
            root.validate_ranged_resource(ResourceKind::SYSTEM, usize::MAX, 2),
            Err(ZxError::OUT_OF_RANGE)
        );
    }
}

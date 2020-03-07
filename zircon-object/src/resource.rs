use {crate::object::*, alloc::sync::Arc, bitflags::bitflags};

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
    UNKNOWN,
}

impl ResourceKind {
    pub fn from_number(number: u32) -> ZxResult<Self> {
        match number {
            0 => Ok(ResourceKind::MMIO),
            1 => Ok(ResourceKind::IRQ),
            2 => Ok(ResourceKind::IOPORT),
            3 => Ok(ResourceKind::HYPERVISOR),
            4 => Ok(ResourceKind::ROOT),
            5 => Ok(ResourceKind::VMEX),
            6 => Ok(ResourceKind::SMC),
            7 => Ok(ResourceKind::COUNT),
            _ => Err(ZxError::INVALID_ARGS),
        }
    }
}

bitflags! {
    pub struct ResourceFlags: u32 {
        #[allow(clippy::identity_op)]
        const EXCLUSIVE      = 1 << 16;
    }
}

/// Address space rights and accounting.
#[allow(dead_code)]
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
            base: {
                let base = KObjectBase::new();
                base.set_name(name);
                base
            },
            kind,
            addr,
            len,
            flags,
        })
    }

    pub fn validate(&self, kind: ResourceKind) -> ZxResult<()> {
        if self.kind == kind || self.kind == ResourceKind::ROOT {
            Ok(())
        } else {
            Err(ZxError::WRONG_TYPE)
        }
    }

    pub fn validate_ranged_resource(
        &self,
        kind: ResourceKind,
        addr: usize,
        len: usize,
    ) -> ZxResult<()> {
        self.validate(kind)?;
        if self.kind == ResourceKind::MMIO {
            unimplemented!()
        }
        if addr >= self.addr && (addr + len) <= (self.addr + self.len) {
            Ok(())
        } else {
            Err(ZxError::OUT_OF_RANGE)
        }
    }

    pub fn check_exclusive(&self, flags: ResourceFlags) -> ZxResult<()> {
        if self.kind != ResourceKind::ROOT
            && (self.flags.contains(ResourceFlags::EXCLUSIVE)
                || flags.contains(ResourceFlags::EXCLUSIVE))
        {
            Err(ZxError::INVALID_ARGS)
        } else {
            Ok(())
        }
    }
}

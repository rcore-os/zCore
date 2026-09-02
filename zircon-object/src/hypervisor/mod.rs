//! Objects for Virtual Machine Monitor (hypervisor).

mod guest;
mod vcpu;

use super::ZxError;
use kernel_hal::{GenericPageTable, HalError, MMUFlags, Result};
use rvm::{
    ArchRvmPageTable, GuestPhysAddr, HostPhysAddr, IntoRvmPageTableFlags, RvmError, RvmPageTable,
};

pub use guest::{Guest, GUEST_PHYSICAL_ASPACE_BASE, GUEST_PHYSICAL_ASPACE_SIZE};
pub use rvm::{TrapKind, VcpuIo, VcpuReadWriteKind, VcpuState};
pub use vcpu::Vcpu;

impl From<RvmError> for ZxError {
    fn from(e: RvmError) -> Self {
        match e {
            RvmError::Internal => Self::INTERNAL,
            RvmError::NotSupported => Self::NOT_SUPPORTED,
            RvmError::NoMemory => Self::NO_MEMORY,
            RvmError::InvalidParam => Self::INVALID_ARGS,
            RvmError::OutOfRange => Self::OUT_OF_RANGE,
            RvmError::BadState => Self::BAD_STATE,
            RvmError::NotFound => Self::NOT_FOUND,
        }
    }
}

impl From<ZxError> for RvmError {
    fn from(e: ZxError) -> Self {
        match e {
            ZxError::INTERNAL => Self::Internal,
            ZxError::NOT_SUPPORTED => Self::NotSupported,
            ZxError::NO_MEMORY => Self::NoMemory,
            ZxError::INVALID_ARGS => Self::InvalidParam,
            ZxError::OUT_OF_RANGE => Self::OutOfRange,
            ZxError::BAD_STATE => Self::BadState,
            ZxError::NOT_FOUND => Self::NotFound,
            _ => Self::BadState,
        }
    }
}

/// Virtual Machine Manager's Page Table.
pub struct VmmPageTable(ArchRvmPageTable);

#[derive(Debug)]
struct VmmPageTableFlags(MMUFlags);

impl VmmPageTable {
    /// Create a new VmmPageTable.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(ArchRvmPageTable::new())
    }
}

impl GenericPageTable for VmmPageTable {
    fn map(&mut self, gpaddr: GuestPhysAddr, hpaddr: HostPhysAddr, flags: MMUFlags) -> Result<()> {
        self.0
            .map(gpaddr, hpaddr, VmmPageTableFlags(flags))
            .map_err(|_| HalError)
    }

    fn unmap(&mut self, gpaddr: GuestPhysAddr) -> Result<()> {
        self.0.unmap(gpaddr).map_err(|_| HalError)
    }

    fn protect(&mut self, gpaddr: GuestPhysAddr, flags: MMUFlags) -> Result<()> {
        self.0
            .protect(gpaddr, VmmPageTableFlags(flags))
            .map_err(|_| HalError)
    }

    fn query(&mut self, gpaddr: GuestPhysAddr) -> Result<HostPhysAddr> {
        self.0.query(gpaddr).map_err(|_| HalError)
    }

    fn table_phys(&self) -> HostPhysAddr {
        self.0.table_phys()
    }
}

impl IntoRvmPageTableFlags for VmmPageTableFlags {
    fn is_read(&self) -> bool {
        self.0.contains(MMUFlags::READ)
    }
    fn is_write(&self) -> bool {
        self.0.contains(MMUFlags::WRITE)
    }
    fn is_execute(&self) -> bool {
        self.0.contains(MMUFlags::EXECUTE)
    }
}

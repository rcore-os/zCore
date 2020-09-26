use {
    crate::{
        object::*,
        signal::{Port, PortPacket},
        vm::VmAddressRegion,
    },
    alloc::sync::{Arc, Weak},
    core::convert::{TryFrom, TryInto},
    rvm::{Guest as GuestInner, RvmError, RvmExitPacket, RvmPort, RvmResult, TrapKind},
    rvm::{GuestPhysAddr, GuestPhysMemorySetTrait, HostPhysAddr},
};

/// The base of the Guest's physical address space.
pub const GUEST_PHYSICAL_ASPACE_BASE: u64 = 0;

/// The size of the Guest's physical address space.
pub const GUEST_PHYSICAL_ASPACE_SIZE: u64 = 1 << 36;

/// A guest is a virtual machine that can be run within the hypervisor.
pub struct Guest {
    base: KObjectBase,
    _counter: CountHelper,
    gpm: Arc<GuestPhysMemorySet>,
    inner: Arc<GuestInner>,
}

impl_kobject!(Guest);
define_count_helper!(Guest);

impl Guest {
    /// Create a new Guest.
    pub fn new() -> ZxResult<Arc<Self>> {
        if !rvm::check_hypervisor_feature() {
            return Err(ZxError::NOT_SUPPORTED);
        }

        let gpm = GuestPhysMemorySet::new();
        Ok(Arc::new(Guest {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            inner: GuestInner::new(gpm.clone())?,
            gpm,
        }))
    }

    /// Sets a trap within a guest, which generates a packet when there is an access
    /// by a VCPU within the address range defined by `addr` and `size`,
    /// within the address space defined by `kind`.
    pub fn set_trap(
        &self,
        kind: u32,
        addr: usize,
        size: usize,
        port: Option<Weak<Port>>,
        key: u64,
    ) -> ZxResult {
        let rvm_port = port.map(|p| -> Arc<dyn RvmPort> { Arc::new(GuestPort(p)) });
        self.inner
            .set_trap(TrapKind::try_from(kind)?, addr, size, rvm_port, key)
            .map_err(From::from)
    }

    /// Get the VMAR of the Guest.
    pub fn vmar(&self) -> Arc<VmAddressRegion> {
        self.gpm.vmar.clone()
    }

    pub(super) fn rvm_guest(&self) -> Arc<GuestInner> {
        self.inner.clone()
    }
}

#[derive(Debug)]
struct GuestPhysMemorySet {
    vmar: Arc<VmAddressRegion>,
}

impl GuestPhysMemorySet {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            vmar: VmAddressRegion::new_guest(),
        })
    }
}

impl GuestPhysMemorySetTrait for GuestPhysMemorySet {
    /// Physical address space size.
    fn size(&self) -> u64 {
        GUEST_PHYSICAL_ASPACE_SIZE
    }

    /// Add a contiguous guest physical memory region and create mapping,
    /// with the target host physical address `hpaddr` (optional).
    fn map(
        &self,
        _gpaddr: GuestPhysAddr,
        _size: usize,
        _hpaddr: Option<HostPhysAddr>,
    ) -> RvmResult {
        // All mappings was created by VMAR, should not call this function.
        Err(RvmError::NotSupported)
    }

    /// Remove a guest physical memory region, destroy the mapping.
    fn unmap(&self, gpaddr: GuestPhysAddr, size: usize) -> RvmResult {
        self.vmar.unmap(gpaddr, size).map_err(From::from)
    }

    /// Read from guest address space.
    fn read_memory(&self, gpaddr: GuestPhysAddr, buf: &mut [u8]) -> RvmResult<usize> {
        self.vmar.read_memory(gpaddr, buf).map_err(From::from)
    }

    /// Write to guest address space.
    fn write_memory(&self, gpaddr: GuestPhysAddr, buf: &[u8]) -> RvmResult<usize> {
        self.vmar.write_memory(gpaddr, buf).map_err(From::from)
    }

    /// Called when accessed a non-mapped guest physical adderss `gpaddr`.
    fn handle_page_fault(&self, gpaddr: GuestPhysAddr) -> RvmResult {
        if let Some(mapping) = self.vmar.find_mapping(gpaddr) {
            mapping
                .handle_page_fault(gpaddr, mapping.get_flags(gpaddr).unwrap())
                .map_err(From::from)
        } else {
            Err(RvmError::NotFound)
        }
    }

    /// Page table base address.
    fn table_phys(&self) -> HostPhysAddr {
        self.vmar.table_phys()
    }
}

#[derive(Debug)]
struct GuestPort(Weak<Port>);

impl RvmPort for GuestPort {
    fn send(&self, packet: RvmExitPacket) -> RvmResult {
        let packet: PortPacket = packet.try_into()?;
        if let Some(port) = self.0.upgrade() {
            port.push(packet);
            Ok(())
        } else {
            Err(RvmError::BadState)
        }
    }
}

use {
    crate::{object::*, vm::*},
    alloc::sync::Arc,
    rvm::{Guest as GuestInner, RvmError, RvmResult},
    rvm::{GuestPhysAddr, GuestPhysMemorySetTrait, HostPhysAddr},
};

pub struct Guest {
    base: KObjectBase,
    _counter: CountHelper,
    gpm: Arc<GuestPhysMemorySet>,
    inner: Arc<GuestInner>,
}

impl_kobject!(Guest);
define_count_helper!(Guest);

impl Guest {
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

    pub fn vmar(&self) -> Arc<VmAddressRegion> {
        self.gpm.vmar.clone()
    }

    pub fn rvm_geust(&self) -> Arc<GuestInner> {
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
    /// Add a contiguous guest physical memory region and create mapping,
    /// with the target host physical address `hpaddr` (optional).
    fn add_map(
        &self,
        _gpaddr: GuestPhysAddr,
        _size: usize,
        _hpaddr: Option<HostPhysAddr>,
    ) -> RvmResult {
        Err(RvmError::NotSupported)
    }

    /// Called when accessed a non-mapped guest physical adderss `gpaddr`.
    fn handle_page_fault(&self, gpaddr: GuestPhysAddr) -> RvmResult {
        if let Some(mapping) = self.vmar.find_mapping(gpaddr) {
            mapping
                .handle_page_fault(gpaddr, mapping.get_flags())
                .map_err(From::from)
        } else {
            return Err(RvmError::NotFound);
        }
    }

    /// Page table base address.
    fn table_phys(&self) -> HostPhysAddr {
        self.vmar.table_phys()
    }
}

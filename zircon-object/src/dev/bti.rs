use {
    super::*,
    crate::object::*,
    alloc::{
        sync::Arc,
        vec::Vec,
    },
    dev::Iommu,
    kernel_hal::PAGE_SIZE,
};

// Iommu refers to DummyIommu in fuchsia
#[allow(dead_code)]
pub struct Bti {
    base: KObjectBase,
    iommu: Arc<Iommu>,
    bti_id: u64,
    pinned_memory: Vec<Pinned>,
    quarantine: Vec<Quarantined>
}

struct Pinned {

}

struct Quarantined {

}

impl_kobject!(Bti);

impl Bti {
    pub fn create(iommu: Arc<Iommu>, bti_id: u64) -> Arc<Self> {
        Arc::new(Bti {
            base: KObjectBase::new(),
            iommu,
            bti_id,
            pinned_memory: Vec::new(),
            quarantine: Vec::new(),
        })
    }

    pub fn get_info(&self) -> ZxInfoBti {
        ZxInfoBti {
            minimum_contiguity: PAGE_SIZE as u64,
            aspace_size: 0xffffffffffffffff,
            pmo_count: self.pinned_memory.len() as u64, // need lock
            quarantine_count: self.quarantine.len() as u64, // need lock
        }
    }
}


#[repr(C)]
#[derive(Default)]
pub struct ZxInfoBti {
    minimum_contiguity: u64,
    aspace_size: u64,
    pmo_count: u64,
    quarantine_count: u64,
}
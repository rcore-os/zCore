use {crate::object::*, alloc::sync::Arc, dev::Iommu};

// Iommu refers to DummyIommu in fuchsia
#[allow(dead_code)]
pub struct Bti {
    base: KObjectBase,
    iommu: Arc<Iommu>,
    bti_id: u64,
}

impl_kobject!(Bti);

impl Bti {
    pub fn create(iommu: Arc<Iommu>, bti_id: u64) -> Arc<Self> {
        Arc::new(Bti {
            base: KObjectBase::new(),
            iommu,
            bti_id,
        })
    }
}

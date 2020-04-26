use {
    crate::object::*,
    alloc::{sync::Arc, vec::Vec},
};

// Iommu refers to DummyIommu in fuchsia

pub struct Iommu {
    base: KObjectBase,
}

const IOMMU_TYPE_DUMMY: u32 = 0;

impl_kobject!(Iommu);

impl Iommu {
    pub fn create(type_: u32, _desc: Vec<u8>, _desc_size: usize) -> Arc<Self> {
        if type_ != IOMMU_TYPE_DUMMY {
            panic!("IOMMU {} is not implemented", type_);
        }
        Arc::new(Iommu {
            base: KObjectBase::new(),
        })
    }

    pub fn is_valid_bus_txn_id(&self) -> bool {
        return true;
    }
}

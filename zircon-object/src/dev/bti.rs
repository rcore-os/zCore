use {
    super::*,
    crate::object::*,
    alloc::{
        sync::Arc,
        vec::Vec,
    },
    spin::Mutex,
    dev::Iommu,
    crate::vm::*,
};

// BusTransactionInitiator
#[allow(dead_code)]
pub struct Bti {
    base: KObjectBase,
    iommu: Arc<Iommu>,
    bti_id: u64,
    inner: Mutex<BtiInner>,
}

struct BtiInner {
    pmts: Vec<Arc<Pmt>>,
    quarantine: Vec<Quarantined>
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
            inner: Mutex::new(BtiInner{
                pmts: Vec::new(),
                quarantine: Vec::new(),
            })
        })
    }

    pub fn get_info(&self) -> ZxInfoBti {
        let inner = self.inner.lock();
        ZxInfoBti {
            minimum_contiguity: self.minimum_contiguity() as u64,
            aspace_size: self.aspace_size() as u64,
            pmo_count: inner.pmts.len() as u64,
            quarantine_count: inner.quarantine.len() as u64,
        }
    }

    pub fn pin(&self,
               vmo: Arc<VmObject>,
               offset: usize,
               size: usize,
               perms: IommuPerms) -> ZxResult<Arc<Pmt>> {
        if size == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let pmt = Pmt::create(self, vmo, perms, offset, size)?;
        self.inner.lock().pmts.push(pmt.clone()); // I'm not sure...
        Ok(pmt)
    }

    pub fn minimum_contiguity(&self) -> usize {
        self.iommu.minimum_contiguity()
    }

    pub fn aspace_size(&self) -> usize {
        self.iommu.aspace_size()
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
use {
    super::*,
    crate::object::*,
    crate::vm::*,
    alloc::{
        collections::BTreeMap,
        sync::{Arc, Weak},
        vec::Vec,
    },
    dev::Iommu,
    spin::Mutex,
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
    pmts: BTreeMap<KoID, Arc<Pmt>>,
    self_ref: Weak<Bti>,
}

impl_kobject!(Bti);

impl Bti {
    pub fn create(iommu: Arc<Iommu>, bti_id: u64) -> Arc<Self> {
        let bti = Arc::new(Bti {
            base: KObjectBase::new(),
            iommu,
            bti_id,
            inner: Mutex::new(BtiInner {
                pmts: Default::default(),
                self_ref: Default::default(),
            }),
        });
        bti.inner.lock().self_ref = Arc::downgrade(&bti);
        bti
    }

    pub fn get_info(&self) -> ZxInfoBti {
        ZxInfoBti {
            minimum_contiguity: self.minimum_contiguity() as u64,
            aspace_size: self.aspace_size() as u64,
            pmo_count: self.get_pmo_count() as u64,
            quarantine_count: self.get_quarantine_count() as u64,
        }
    }

    pub fn pin(
        &self,
        vmo: Arc<VmObject>,
        offset: usize,
        size: usize,
        perms: IommuPerms,
    ) -> ZxResult<Arc<Pmt>> {
        if size == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let pmt = Pmt::create(self.inner.lock().self_ref.clone(), vmo, perms, offset, size)?;
        self.inner.lock().pmts.insert(pmt.id(), pmt.clone());
        Ok(pmt)
    }

    pub fn minimum_contiguity(&self) -> usize {
        self.iommu.minimum_contiguity()
    }

    pub fn aspace_size(&self) -> usize {
        self.iommu.aspace_size()
    }

    pub fn get_iommu(&self) -> Arc<Iommu> {
        self.iommu.clone()
    }

    pub fn release_pmt(&self, id: KoID) {
        self.inner.lock().pmts.remove(&id).unwrap();
    }

    pub fn release_quarantine(&self) {
        let mut inner = self.inner.lock();
        let mut to_release: Vec<KoID> = Vec::new();
        for (id, pmt) in inner.pmts.iter() {
            // no handle, the only arc is from self.pmts
            if Arc::strong_count(&pmt) == 1 {
                to_release.push(*id);
            }
        }
        for id in to_release {
            inner.pmts.remove(&id).unwrap();
        }
    }

    pub fn get_pmo_count(&self) -> usize {
        self.inner.lock().pmts.len()
    }

    pub fn get_quarantine_count(&self) -> usize {
        let mut cnt = 0;
        for (_id, pmt) in self.inner.lock().pmts.iter() {
            if Arc::strong_count(&pmt) == 1 {
                // no handle, the only arc is from self.pmts
                cnt += 1;
            }
        }
        cnt
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

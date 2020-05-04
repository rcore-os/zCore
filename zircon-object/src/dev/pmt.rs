use {
    super::*,
    crate::object::*,
    crate::vm::{DevVAddr, ceil},
    crate::vm::*,
    alloc::{
        sync::{Arc, Weak},
        vec::Vec,
    },
    spin::Mutex,
};

// PinnedMemoryToken
pub struct Pmt {
    base: KObjectBase,
    bti: Weak<Bti>,
    vmo: Arc<VmObject>,
    offset: usize,
    size: usize,
    mapped_addrs: Vec<DevVAddr>,
    inner: Mutex<PmtInner>
}

struct PmtInner {
    unpinned: bool,
}

impl_kobject!(Pmt);

impl Drop for Pmt {
    fn drop(&mut self) {
        if !self.inner.lock().unpinned {
            self.unpin().unwrap();
        }
    }
}

impl Pmt {
    pub fn create(
        bti: Weak<Bti>,
        vmo: Arc<VmObject>,
        perms: IommuPerms,
        offset: usize,
        size: usize,
    ) -> ZxResult<Arc<Self>> {
        let iommu = bti.upgrade().unwrap().get_iommu();

        if vmo.is_paged() {
            vmo.commit(offset, size)?;
            vmo.pin(offset, size)?;
        }

        let mapped_addrs = Pmt::map_into_iommu(iommu, vmo.clone(), offset, size, perms)?;

        Ok(Arc::new(Pmt {
            base: KObjectBase::new(),
            bti,
            vmo,
            offset,
            size,
            mapped_addrs,
            inner: Mutex::new(PmtInner{
                unpinned: false,
            }),
        }))
    }

    pub fn map_into_iommu(
        iommu: Arc<Iommu>,
        vmo: Arc<VmObject>,
        offset: usize,
        size: usize,
        perms: IommuPerms
    ) -> ZxResult<Vec<DevVAddr>> {
        if vmo.is_contiguous() {
            let (vaddr, _mapped_len) = iommu.map_contiguous(vmo, offset, size, perms)?;
            // vec! is better, but compile error occurs.
            let mut mapped_addrs: Vec<DevVAddr> = Vec::new(); 
            mapped_addrs.push(vaddr);
            Ok(mapped_addrs)
        } else {
            assert_eq!(size % iommu.minimum_contiguity(), 0);
            let mut mapped_addrs: Vec<DevVAddr> = Vec::new(); 
            let mut remaining = size;
            let mut cur_offset = offset;
            while remaining > 0 {
                let (mut vaddr, mapped_len) = iommu.map(vmo.clone(), cur_offset, remaining, perms)?;
                assert_eq!(mapped_len % iommu.minimum_contiguity(), 0);
                for _ in 0 .. mapped_len / iommu.minimum_contiguity() {
                    mapped_addrs.push(vaddr);
                    vaddr += iommu.minimum_contiguity();
                }
                remaining -= mapped_len;
                cur_offset += mapped_len;
            }
            Ok(mapped_addrs)
        }
    }

    pub fn encode_addrs(
        &self,
        compress_results: bool,
        contiguous: bool
    ) -> ZxResult<Vec<DevVAddr>> {
        let iommu = self.bti.upgrade().unwrap().get_iommu();
        if compress_results {
            if self.vmo.is_contiguous() {
                let num_addrs = ceil(self.size, iommu.minimum_contiguity());
                let min_contig = iommu.minimum_contiguity();
                let base = self.mapped_addrs[0];
                Ok((0..num_addrs)
                    .map(|i| base + min_contig * i)
                    .collect())
            } else {
                Ok(self.mapped_addrs.clone())
            }
        } else if contiguous {
            if !self.vmo.is_contiguous() {
                Err(ZxError::INVALID_ARGS)
            } else {
                // vec! is better, but compile error occurs.
                let mut encoded_addrs: Vec<DevVAddr> = Vec::new();
                encoded_addrs.push(self.mapped_addrs[0]);
                Ok(encoded_addrs)
            }
        } else {
            let min_contig = 
                if self.vmo.is_contiguous() { self.size } 
                else { iommu.minimum_contiguity()};
            let num_pages = self.size / PAGE_SIZE;
            let mut encoded_addrs: Vec<DevVAddr> = Vec::new();
            for base in &self.mapped_addrs {
                let mut addr = *base;
                while addr < base + min_contig && encoded_addrs.len() < num_pages {
                    encoded_addrs.push(addr);
                    addr += PAGE_SIZE; // not sure ...
                }
            }
            Ok(encoded_addrs)
        }
    }

    pub fn unpin_and_remove(&self) -> ZxResult {
        self.unpin()?;
        if let Some(bti) = self.bti.upgrade() {
            bti.release_pmt(self.base.id);
        }
        Ok(())
    }

    fn unpin(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.unpinned && self.vmo.is_paged() {
            self.vmo.unpin(self.offset, self.size)?;
        }
        inner.unpinned = true;
        Ok(())
    }
}
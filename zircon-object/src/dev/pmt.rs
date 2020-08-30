use {
    super::*,
    crate::object::*,
    crate::vm::*,
    alloc::{
        sync::{Arc, Weak},
        vec,
        vec::Vec,
    },
};

/// Pinned Memory Token.
///
/// It will pin memory on construction and unpin on drop.
pub struct PinnedMemoryToken {
    base: KObjectBase,
    bti: Weak<BusTransactionInitiator>,
    vmo: Arc<VmObject>,
    offset: usize,
    size: usize,
    mapped_addrs: Vec<DevVAddr>,
}

impl_kobject!(PinnedMemoryToken);

impl Drop for PinnedMemoryToken {
    fn drop(&mut self) {
        if self.vmo.is_paged() {
            self.vmo.unpin(self.offset, self.size).unwrap();
        }
    }
}

impl PinnedMemoryToken {
    /// Create a `PinnedMemoryToken` by `BusTransactionInitiator`.
    pub(crate) fn create(
        bti: &Arc<BusTransactionInitiator>,
        vmo: Arc<VmObject>,
        perms: IommuPerms,
        offset: usize,
        size: usize,
    ) -> ZxResult<Arc<Self>> {
        if vmo.is_paged() {
            vmo.commit(offset, size)?;
            vmo.pin(offset, size)?;
        }
        let mapped_addrs = Self::map_into_iommu(&bti.iommu(), vmo.clone(), offset, size, perms)?;
        Ok(Arc::new(PinnedMemoryToken {
            base: KObjectBase::new(),
            bti: Arc::downgrade(bti),
            vmo,
            offset,
            size,
            mapped_addrs,
        }))
    }

    /// Used during initialization to set up the IOMMU state for this PMT.
    fn map_into_iommu(
        iommu: &Arc<Iommu>,
        vmo: Arc<VmObject>,
        offset: usize,
        size: usize,
        perms: IommuPerms,
    ) -> ZxResult<Vec<DevVAddr>> {
        if vmo.is_contiguous() {
            let (vaddr, _mapped_len) = iommu.map_contiguous(vmo, offset, size, perms)?;
            Ok(vec![vaddr])
        } else {
            assert_eq!(size % iommu.minimum_contiguity(), 0);
            let mut mapped_addrs: Vec<DevVAddr> = Vec::new();
            let mut remaining = size;
            let mut cur_offset = offset;
            while remaining > 0 {
                let (mut vaddr, mapped_len) =
                    iommu.map(vmo.clone(), cur_offset, remaining, perms)?;
                assert_eq!(mapped_len % iommu.minimum_contiguity(), 0);
                for _ in 0..mapped_len / iommu.minimum_contiguity() {
                    mapped_addrs.push(vaddr);
                    vaddr += iommu.minimum_contiguity();
                }
                remaining -= mapped_len;
                cur_offset += mapped_len;
            }
            Ok(mapped_addrs)
        }
    }

    /// Encode the mapped addresses.
    pub fn encode_addrs(
        &self,
        compress_results: bool,
        contiguous: bool,
    ) -> ZxResult<Vec<DevVAddr>> {
        let iommu = self.bti.upgrade().unwrap().iommu();
        if compress_results {
            if self.vmo.is_contiguous() {
                let num_addrs = ceil(self.size, iommu.minimum_contiguity());
                let min_contig = iommu.minimum_contiguity();
                let base = self.mapped_addrs[0];
                Ok((0..num_addrs).map(|i| base + min_contig * i).collect())
            } else {
                Ok(self.mapped_addrs.clone())
            }
        } else if contiguous {
            if !self.vmo.is_contiguous() {
                Err(ZxError::INVALID_ARGS)
            } else {
                Ok(vec![self.mapped_addrs[0]])
            }
        } else {
            let min_contig = if self.vmo.is_contiguous() {
                self.size
            } else {
                iommu.minimum_contiguity()
            };
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

    /// Unpin pages and revoke device access to them.
    pub fn unpin(&self) {
        if let Some(bti) = self.bti.upgrade() {
            bti.release_pmt(self.base.id);
        }
    }
}

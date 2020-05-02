#![allow(warnings)]
use {
    super::*,
    crate::object::*,
    crate::vm::{DevVAddr, roundup},
    crate::vm::*,
    alloc::{
        sync::{Arc, Weak},
        vec::Vec,
    }
};

// PinnedMemoryToken
#[allow(dead_code)]
pub struct Pmt {
    base: KObjectBase,
    vmo: Arc<VmObject>,
    offset: usize,
    size: usize,
    mapped_addrs: Vec<DevVAddr>,
}

impl_kobject!(Pmt);

impl Drop for Pmt {
    fn drop(&mut self) {
        // TODO: unpin pages
    }
}

impl Pmt {
    pub fn create(
        bti: &Bti,
        vmo: Arc<VmObject>,
        perms: IommuPerms,
        offset: usize,
        size: usize,
    ) -> ZxResult<Arc<Self>> {
        if vmo.is_paged() {
            vmo.commit(offset, size)?;
            vmo.pin(offset, size)?;
        }
        
        let num_addrs: usize = if vmo.is_contiguous() {
            1
        } else {
            roundup(size, bti.minimum_contiguity())
        };
        
        let mapped_addrs = Pmt::mapped_into_iommu(num_addrs)?;
        Ok(Arc::new(Pmt {
            base: KObjectBase::new(),
            vmo,
            offset,
            size,
            mapped_addrs,
        }))
    }

    pub fn mapped_into_iommu(_num_addrs: usize) -> ZxResult<Vec<DevVAddr>> {
        Err(ZxError::NOT_SUPPORTED)
    }

    pub fn encode_addrs(
        &self,
        _compress_results: bool,
        _contiguous: bool,
        _addrs_count: usize,
    ) -> ZxResult<Vec<DevVAddr>> {
        Err(ZxError::NOT_SUPPORTED)
    }
}
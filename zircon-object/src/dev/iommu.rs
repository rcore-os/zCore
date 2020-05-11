use {crate::object::*, crate::vm::*, alloc::sync::Arc, bitflags::bitflags};

/// Iommu refers to DummyIommu in zircon.
///
/// A dummy implementation, do not take it serious.
pub struct Iommu {
    base: KObjectBase,
}

impl_kobject!(Iommu);

impl Iommu {
    /// Create a new `IOMMU`.
    pub fn create() -> Arc<Self> {
        Arc::new(Iommu {
            base: KObjectBase::new(),
        })
    }

    pub fn is_valid_bus_txn_id(&self) -> bool {
        true
    }

    pub fn minimum_contiguity(&self) -> usize {
        PAGE_SIZE as usize
    }

    pub fn aspace_size(&self) -> usize {
        usize::MAX
    }

    pub fn map(
        &self,
        vmo: Arc<VmObject>,
        offset: usize,
        size: usize,
        perms: IommuPerms,
    ) -> ZxResult<(DevVAddr, usize)> {
        if perms == IommuPerms::empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        if offset + size > vmo.len() {
            return Err(ZxError::INVALID_ARGS);
        }
        let p_addr = vmo.commit_page(offset, MMUFlags::empty())?;
        if vmo.is_paged() {
            Ok((p_addr, PAGE_SIZE))
        } else {
            Ok((p_addr, pages(size)))
        }
    }

    pub fn map_contiguous(
        &self,
        vmo: Arc<VmObject>,
        offset: usize,
        size: usize,
        perms: IommuPerms,
    ) -> ZxResult<(DevVAddr, usize)> {
        if perms == IommuPerms::empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        if offset + size > vmo.len() {
            return Err(ZxError::INVALID_ARGS);
        }
        let p_addr = vmo.commit_page(offset, MMUFlags::empty())?;
        if vmo.is_paged() {
            Ok((p_addr, PAGE_SIZE))
        } else {
            Ok((p_addr, pages(size) * PAGE_SIZE))
        }
    }
}

bitflags! {
    pub struct IommuPerms: u32 {
        #[allow(clippy::identity_op)]
        const PERM_READ             = 1 << 0;
        const PERM_WRITE            = 1 << 1;
        const PERM_EXECUTE          = 1 << 2;
    }
}

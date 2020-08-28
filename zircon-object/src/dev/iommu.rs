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

    /// Check if a `bus_txn_id` is valid for this IOMMU.
    pub fn is_valid_bus_txn_id(&self) -> bool {
        true
    }

    /// Returns the number of bytes that Map() can guarantee, upon success, to find
    /// a contiguous address range for.
    pub fn minimum_contiguity(&self) -> usize {
        PAGE_SIZE as usize
    }

    /// The number of bytes in the address space (UINT64_MAX if 2^64).
    pub fn aspace_size(&self) -> usize {
        usize::MAX
    }

    /// Grant a device access to the range of pages given by [offset, offset + size) in `vmo`.
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
        let mut flags = MMUFlags::empty();
        if perms.contains(IommuPerms::PERM_READ) {
            flags |= MMUFlags::READ;
        }
        if perms.contains(IommuPerms::PERM_WRITE) {
            flags |= MMUFlags::WRITE;
        }
        if perms.contains(IommuPerms::PERM_EXECUTE) {
            flags |= MMUFlags::EXECUTE;
        }
        let p_addr = vmo.commit_page(offset / PAGE_SIZE, flags)?;
        if vmo.is_paged() {
            Ok((p_addr, PAGE_SIZE))
        } else {
            Ok((p_addr, pages(size)))
        }
    }

    /// Same as `map`, but with additional guarantee that this will never return a
    /// partial mapping.  It will either return a single contiguous mapping or
    /// return a failure.
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
    /// IOMMU permission flags.
    pub struct IommuPerms: u32 {
        #[allow(clippy::identity_op)]
        /// Read Permission.
        const PERM_READ             = 1 << 0;
        /// Write Permission.
        const PERM_WRITE            = 1 << 1;
        /// Execute Permission.
        const PERM_EXECUTE          = 1 << 2;
    }
}

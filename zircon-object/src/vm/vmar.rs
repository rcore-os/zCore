use {
    super::*, crate::hal::PageTable, crate::object::*, crate::vm::vmo::VMObject, alloc::sync::Arc,
    alloc::vec::Vec, spin::Mutex,
};

/// Virtual Memory Address Regions
pub struct VmAddressRegion {
    base: KObjectBase,
    addr: VirtAddr,
    size: usize,
    parent: Option<Arc<VmAddressRegion>>,
    page_table: Arc<Mutex<PageTable>>,
    /// If inner is None, this region is destroyed, all operations are invalid.
    inner: Mutex<Option<VmarInner>>,
}

impl_kobject!(VmAddressRegion);

/// The mutable part of `VmAddressRegion`.
#[derive(Default)]
struct VmarInner {
    children: Vec<Arc<VmAddressRegion>>,
    mappings: Vec<VmMapping>,
}

impl VmAddressRegion {
    /// Create a new root VMAR.
    pub fn new_root() -> Arc<Self> {
        Arc::new(VmAddressRegion {
            base: KObjectBase::new(),
            addr: 0,
            size: 0x8000_00000000,
            parent: None,
            page_table: Arc::new(Mutex::new(hal::PageTable::new())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a child VMAR at `offset` with `len`.
    ///
    /// The `offset` and `len` should be page aligned,
    /// or an `INVALID_ARGS` error will be returned.
    pub fn create_child(self: &Arc<Self>, offset: usize, len: usize) -> ZxResult<Arc<Self>> {
        if !page_aligned(offset) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        if !self.test_map(&inner, offset, len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let child = Arc::new(VmAddressRegion {
            base: KObjectBase::new(),
            addr: self.addr + offset,
            size: len,
            parent: Some(self.clone()),
            page_table: self.page_table.clone(),
            inner: Mutex::new(Some(VmarInner::default())),
        });
        inner.children.push(child.clone());
        Ok(child)
    }

    /// Map the `vmo` into this VMAR.
    pub fn map(
        &self,
        offset: usize,
        vmo: Arc<dyn VMObject>,
        vmo_offset: usize,
        len: usize,
    ) -> ZxResult<()> {
        if !page_aligned(offset)
            || !page_aligned(vmo_offset)
            || !page_aligned(len)
            || vmo_offset + len > vmo.len()
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        if !self.test_map(&inner, offset, len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mapping = VmMapping {
            addr: self.addr + offset,
            size: len,
            vmo,
            vmo_offset,
            page_table: self.page_table.clone(),
        };
        mapping.map();
        inner.mappings.push(mapping);
        Ok(())
    }

    pub fn unmap(&self, addr: VirtAddr, len: usize) -> ZxResult<()> {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;

        let begin = addr;
        let end = addr + len;
        // check partial overlapped sub-regions
        for vmar in inner.children.iter() {
            if vmar.partial_overlap(begin, end) {
                return Err(ZxError::INVALID_ARGS);
            }
        }
        // FIXME: split partial-overlapped VmMappings
        inner.mappings.drain_filter(|map| map.overlap(begin, end));
        for vmar in inner.children.drain_filter(|vmar| vmar.within(begin, end)) {
            vmar.destroy_internal()?;
        }
        Ok(())
    }

    pub fn protect(&self) {
        unimplemented!()
    }

    /// Unmap all mappings within the VMAR, and destroy all sub-regions of the region.
    pub fn destroy(self: &Arc<Self>) -> ZxResult<()> {
        self.destroy_internal()?;
        // remove from parent
        if let Some(parent) = &self.parent {
            let mut guard = parent.inner.lock();
            let inner = guard.as_mut().unwrap();
            inner.children.retain(|vmar| !Arc::ptr_eq(self, vmar));
        }
        Ok(())
    }

    /// Destroy but do not remove self from parent.
    fn destroy_internal(&self) -> ZxResult<()> {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        for vmar in inner.children.iter() {
            vmar.destroy_internal()?;
        }
        *guard = None;
        Ok(())
    }

    pub fn addr(&self) -> usize {
        self.addr
    }

    pub fn is_dead(&self) -> bool {
        self.inner.lock().is_none()
    }

    pub fn is_alive(&self) -> bool {
        !self.is_dead()
    }

    /// Test if can create a new mapping at `offset` with `len`.
    fn test_map(&self, inner: &VmarInner, offset: usize, len: usize) -> bool {
        debug_assert!(page_aligned(offset));
        debug_assert!(page_aligned(len));
        let begin = self.addr + offset;
        let end = begin + len;
        if end > self.addr + self.size {
            return false;
        }
        // brute force
        for vmar in inner.children.iter() {
            if vmar.overlap(begin, end) {
                return false;
            }
        }
        for map in inner.mappings.iter() {
            if map.overlap(begin, end) {
                return false;
            }
        }
        true
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        !(self.addr >= end || self.addr + self.size <= begin)
    }

    fn within(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        begin <= self.addr && self.addr + self.size <= end
    }

    fn partial_overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        self.overlap(begin, end) && !self.within(begin, end)
    }
}

/// Virtual Memory Mapping
pub struct VmMapping {
    addr: VirtAddr,
    size: usize,
    vmo: Arc<dyn VMObject>,
    vmo_offset: usize,
    page_table: Arc<Mutex<PageTable>>,
}

impl VmMapping {
    fn map(&self) {
        let mut page_table = self.page_table.lock();
        self.vmo
            .map_to(&mut page_table, self.addr, self.vmo_offset, self.size);
    }

    fn unmap(&self) {
        let mut page_table = self.page_table.lock();
        self.vmo
            .unmap_from(&mut page_table, self.addr, self.vmo_offset, self.size);
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        !(self.addr >= end || self.addr + self.size <= begin)
    }
}

impl Drop for VmMapping {
    fn drop(&mut self) {
        self.unmap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::vmo::VMObjectPaged;

    #[test]
    fn create_child() {
        let root_vmar = VmAddressRegion::new_root();
        let child = root_vmar
            .create_child(0, 0x2000)
            .expect("failed to create child VMAR");

        // test invalid argument
        assert_eq!(
            root_vmar.create_child(0x2001, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child(0x2000, 1).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child(0, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            child.create_child(0x1000, 0x2000).err(),
            Some(ZxError::INVALID_ARGS)
        );
    }

    /// A valid virtual address base to mmap.
    const VBASE: VirtAddr = 0x2_00000000;
    const VSIZE: VirtAddr = 0x1_00000;
    const MAGIC: usize = 0xdead_beaf;

    #[test]
    #[allow(unsafe_code)]
    fn map() {
        let root_vmar = VmAddressRegion::new_root();
        let vmar = root_vmar.create_child(VBASE, VSIZE).unwrap();
        let vmo = VMObjectPaged::new(4);

        // invalid argument
        assert_eq!(
            vmar.map(0, vmo.clone(), 0x4000, 0x1000),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map(0, vmo.clone(), 0, 0x5000),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map(0, vmo.clone(), 0x1000, 1),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map(0, vmo.clone(), 1, 0x1000),
            Err(ZxError::INVALID_ARGS)
        );

        vmar.map(0, vmo.clone(), 0, 0x4000).unwrap();
        vmar.map(0x12000, vmo.clone(), 0x2000, 0x1000).unwrap();

        unsafe {
            ((VBASE + 0x2000) as *mut usize).write(MAGIC);
            assert_eq!(((VBASE + 0x12000) as *const usize).read(), MAGIC);
        }
    }

    /// ```text
    /// +--------+--------+--------+--------+
    /// |           root              ....  |
    /// +--------+--------+--------+--------+
    /// |      child1     | child2 |
    /// +--------+--------+--------+
    /// | g-son1 | g-son2 |
    /// +--------+--------+
    /// ```
    struct Sample {
        root: Arc<VmAddressRegion>,
        child1: Arc<VmAddressRegion>,
        child2: Arc<VmAddressRegion>,
        grandson1: Arc<VmAddressRegion>,
        grandson2: Arc<VmAddressRegion>,
    }

    impl Sample {
        fn new() -> Self {
            let root = VmAddressRegion::new_root();
            let child1 = root.create_child(0, 0x2000).unwrap();
            let child2 = root.create_child(0x2000, 0x1000).unwrap();
            let grandson1 = child1.create_child(0, 0x1000).unwrap();
            let grandson2 = child1.create_child(0x1000, 0x1000).unwrap();
            Sample {
                root,
                child1,
                child2,
                grandson1,
                grandson2,
            }
        }
    }

    #[test]
    fn unmap() {
        let s = Sample::new();
        s.child1.unmap(0, 0x1000).unwrap();
        assert!(s.grandson1.is_dead());
        assert!(s.grandson2.is_alive());

        // partial overlap sub-region should fail.
        let s = Sample::new();
        assert_eq!(s.root.unmap(0x1000, 0x2000), Err(ZxError::INVALID_ARGS));

        // unmap nothing should success.
        let s = Sample::new();
        s.child1.unmap(0x8000, 0x1000).unwrap();
    }

    #[test]
    fn destroy() {
        let s = Sample::new();
        s.child1.destroy().unwrap();
        assert!(s.child1.is_dead());
        assert!(s.grandson1.is_dead());
        assert!(s.grandson2.is_dead());
        assert!(s.child2.is_alive());
        // address space should be released
        assert!(s.root.create_child(0, 0x1000).is_ok());
    }
}

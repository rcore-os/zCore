use {
    super::*, crate::object::*, crate::vm::vmo::VMObject, alloc::sync::Arc, alloc::vec::Vec,
    kernel_hal::PageTable, spin::Mutex,
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
        const BASE: usize = 0x2_00000000;
        Arc::new(VmAddressRegion {
            base: KObjectBase::new(),
            addr: BASE,
            size: usize::max_value() - 0xfff - BASE,
            parent: None,
            page_table: Arc::new(Mutex::new(kernel_hal::PageTable::new())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a child VMAR at given `offset`.
    pub fn create_child_at(self: &Arc<Self>, offset: usize, len: usize) -> ZxResult<Arc<Self>> {
        self.create_child(Some(offset), len)
    }

    /// Create a child VMAR at `offset` with `len`.
    ///
    /// The `offset` and `len` should be page aligned,
    /// or an `INVALID_ARGS` error will be returned.
    pub fn create_child(
        self: &Arc<Self>,
        offset: Option<usize>,
        len: usize,
    ) -> ZxResult<Arc<Self>> {
        if !page_aligned(offset.unwrap_or(0)) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        let offset = self.determine_offset(inner, offset, len)?;
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

    /// Map the `vmo` into this VMAR at given `offset`.
    pub fn map_at(
        &self,
        offset: usize,
        vmo: Arc<dyn VMObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<()> {
        self.map(Some(offset), vmo, vmo_offset, len, flags)?;
        Ok(())
    }

    /// Map the `vmo` into this VMAR.
    pub fn map(
        &self,
        offset: Option<usize>,
        vmo: Arc<dyn VMObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<VirtAddr> {
        if !page_aligned(offset.unwrap_or(0))
            || !page_aligned(vmo_offset)
            || !page_aligned(len)
            || vmo_offset + len > vmo.len()
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        let offset = self.determine_offset(inner, offset, len)?;
        let addr = self.addr + offset;
        let mapping = VmMapping {
            addr,
            size: len,
            flags,
            vmo,
            vmo_offset,
            page_table: self.page_table.clone(),
        };
        mapping.map();
        inner.mappings.push(mapping);
        Ok(addr)
    }

    /// Unmaps all VMO mappings and destroys all sub-regions within the absolute range
    /// including `addr` and ending before exclusively at `addr + len`.
    /// Any sub-region that is in the range must be fully in the range
    /// (i.e. partial overlaps are an error).
    /// If a mapping is only partially in the range, the mapping is split and the requested
    /// portion is unmapped.
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
        let mut new_maps = Vec::new();
        inner.mappings.drain_filter(|map| {
            if let Some(new) = map.cut(begin, end) {
                new_maps.push(new);
            }
            map.size == 0
        });
        inner.mappings.extend(new_maps);
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

    /// Unmap all mappings and destroy all sub-regions of VMAR.
    pub fn clear(&self) -> ZxResult<()> {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        for vmar in inner.children.drain(..) {
            vmar.destroy_internal()?;
        }
        inner.mappings.clear();
        Ok(())
    }

    /// Get physical address of the underlying page table.
    pub fn table_phys(&self) -> PhysAddr {
        self.page_table.lock().table_phys()
    }

    /// Get start address of this VMAR.
    pub fn addr(&self) -> usize {
        self.addr
    }

    pub fn is_dead(&self) -> bool {
        self.inner.lock().is_none()
    }

    pub fn is_alive(&self) -> bool {
        !self.is_dead()
    }

    /// Determine final address with given input `offset` and `len`.
    fn determine_offset(
        &self,
        inner: &VmarInner,
        offset: Option<usize>,
        len: usize,
    ) -> ZxResult<VirtAddr> {
        if let Some(offset) = offset {
            if self.test_map(&inner, offset, len) {
                Ok(offset)
            } else {
                Err(ZxError::INVALID_ARGS)
            }
        } else {
            match self.find_free_area(&inner, 0, len) {
                Some(offset) => Ok(offset),
                None => Err(ZxError::NO_MEMORY),
            }
        }
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
        if inner.children.iter().any(|vmar| vmar.overlap(begin, end)) {
            return false;
        }
        if inner.mappings.iter().any(|map| map.overlap(begin, end)) {
            return false;
        }
        true
    }

    /// Find a free area with `len`.
    fn find_free_area(&self, inner: &VmarInner, offset_hint: usize, len: usize) -> Option<usize> {
        // TODO: randomize
        debug_assert!(page_aligned(offset_hint));
        debug_assert!(page_aligned(len));
        // brute force:
        // try each area's end address as the start
        core::iter::once(offset_hint)
            .chain(inner.children.iter().map(|map| map.end_addr() - self.addr))
            .chain(inner.mappings.iter().map(|map| map.end_addr() - self.addr))
            .find(|&offset| self.test_map(inner, offset, len))
    }

    fn end_addr(&self) -> VirtAddr {
        self.addr + self.size
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        !(self.addr >= end || self.end_addr() <= begin)
    }

    fn within(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        begin <= self.addr && self.end_addr() <= end
    }

    fn partial_overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        self.overlap(begin, end) && !self.within(begin, end)
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        inner.mappings.len() + inner.children.len()
    }

    #[cfg(test)]
    fn used_size(&self) -> usize {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        let map_size: usize = inner.mappings.iter().map(|map| map.size).sum();
        let vmar_size: usize = inner.children.iter().map(|vmar| vmar.size).sum();
        map_size + vmar_size
    }
}

/// Virtual Memory Mapping
pub struct VmMapping {
    addr: VirtAddr,
    size: usize,
    flags: MMUFlags,
    vmo: Arc<dyn VMObject>,
    vmo_offset: usize,
    page_table: Arc<Mutex<PageTable>>,
}

impl VmMapping {
    fn map(&self) {
        let mut page_table = self.page_table.lock();
        self.vmo.map_to(
            &mut page_table,
            self.addr,
            self.vmo_offset,
            self.size,
            self.flags,
        );
    }

    fn unmap(&self) {
        let mut page_table = self.page_table.lock();
        self.vmo
            .unmap_from(&mut page_table, self.addr, self.vmo_offset, self.size);
    }

    /// Cut and unmap regions in `[begin, end)`.
    ///
    /// If it will be split, return another one.
    fn cut(&mut self, begin: VirtAddr, end: VirtAddr) -> Option<Self> {
        if !self.overlap(begin, end) {
            return None;
        }
        if self.addr >= begin && self.end_addr() <= end {
            // subset: [xxxxxxxxxx]
            self.unmap();
            self.size = 0;
            None
        } else if self.addr >= begin && self.addr < end {
            // prefix: [xxxx------]
            let cut_len = end - self.addr;
            let mut page_table = self.page_table.lock();
            self.vmo
                .unmap_from(&mut page_table, self.addr, self.vmo_offset, cut_len);
            self.addr = end;
            self.size -= cut_len;
            self.vmo_offset += cut_len;
            None
        } else if self.end_addr() <= end && self.end_addr() > begin {
            // postfix: [------xxxx]
            let cut_len = self.end_addr() - begin;
            let new_len = begin - self.addr;
            let mut page_table = self.page_table.lock();
            self.vmo
                .unmap_from(&mut page_table, begin, self.vmo_offset + new_len, cut_len);
            self.size = new_len;
            None
        } else {
            // superset: [---xxxx---]
            let cut_len = end - begin;
            let new_len1 = begin - self.addr;
            let new_len2 = self.end_addr() - end;
            let mut page_table = self.page_table.lock();
            self.vmo
                .unmap_from(&mut page_table, begin, self.vmo_offset + new_len1, cut_len);
            self.size = new_len1;
            Some(VmMapping {
                addr: end,
                size: new_len2,
                flags: self.flags,
                vmo: self.vmo.clone(),
                vmo_offset: self.vmo_offset + (end - self.addr),
                page_table: self.page_table.clone(),
            })
        }
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        !(self.addr >= end || self.end_addr() <= begin)
    }

    fn end_addr(&self) -> VirtAddr {
        self.addr + self.size
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
            .create_child_at(0, 0x2000)
            .expect("failed to create child VMAR");

        // test invalid argument
        assert_eq!(
            root_vmar.create_child_at(0x2001, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child_at(0x2000, 1).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child_at(0, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            child.create_child_at(0x1000, 0x2000).err(),
            Some(ZxError::INVALID_ARGS)
        );
    }

    /// A valid virtual address base to mmap.
    const MAGIC: usize = 0xdead_beaf;

    #[test]
    #[allow(unsafe_code)]
    fn map() {
        let vmar = VmAddressRegion::new_root();
        let vmo = VMObjectPaged::new(4);
        let flags = MMUFlags::READ | MMUFlags::WRITE;

        // invalid argument
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0x4000, 0x1000, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0, 0x5000, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0x1000, 1, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 1, 0x1000, flags),
            Err(ZxError::INVALID_ARGS)
        );

        vmar.map_at(0, vmo.clone(), 0, 0x4000, flags).unwrap();
        vmar.map_at(0x12000, vmo.clone(), 0x2000, 0x1000, flags)
            .unwrap();

        unsafe {
            ((vmar.addr() + 0x2000) as *mut usize).write(MAGIC);
            assert_eq!(((vmar.addr() + 0x12000) as *const usize).read(), MAGIC);
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
            let child1 = root.create_child_at(0, 0x2000).unwrap();
            let child2 = root.create_child_at(0x2000, 0x1000).unwrap();
            let grandson1 = child1.create_child_at(0, 0x1000).unwrap();
            let grandson2 = child1.create_child_at(0x1000, 0x1000).unwrap();
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
    fn unmap_vmar() {
        let s = Sample::new();
        let base = s.root.addr();
        s.child1.unmap(base, 0x1000).unwrap();
        assert!(s.grandson1.is_dead());
        assert!(s.grandson2.is_alive());

        // partial overlap sub-region should fail.
        let s = Sample::new();
        assert_eq!(
            s.root.unmap(base + 0x1000, 0x2000),
            Err(ZxError::INVALID_ARGS)
        );

        // unmap nothing should success.
        let s = Sample::new();
        s.child1.unmap(base + 0x8000, 0x1000).unwrap();
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
        assert!(s.root.create_child_at(0, 0x1000).is_ok());
    }

    #[test]
    fn unmap_mapping() {
        //   +--------+--------+--------+--------+--------+
        // 1 [--------------------------|xxxxxxxx|--------]
        // 2 [xxxxxxxx|-----------------]
        // 3          [--------|xxxxxxxx]
        // 4          [xxxxxxxx]
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VMObjectPaged::new(5);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x5000, flags).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x5000);

        // 0. unmap none.
        vmar.unmap(base + 0x5000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x5000);

        // 1. unmap middle.
        vmar.unmap(base + 0x3000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x4000);

        // 2. unmap prefix.
        vmar.unmap(base, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x3000);

        // 3. unmap postfix.
        vmar.unmap(base + 0x2000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x2000);

        // 4. unmap all.
        vmar.unmap(base + 0x1000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x1000);
    }
}

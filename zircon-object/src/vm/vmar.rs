use {
    super::*,
    crate::object::*,
    alloc::{sync::Arc, vec, vec::Vec},
    bitflags::bitflags,
    kernel_hal::vm::{
        GenericPageTable, IgnoreNotMappedErr, Page, PageSize, PageTable, PagingError, PagingResult,
    },
    lock::Mutex,
};

bitflags! {
    /// Creation flags for VmAddressRegion.
    pub struct VmarFlags: u32 {
        #[allow(clippy::identity_op)]
        /// When randomly allocating subregions, reduce sprawl by placing allocations
        /// near each other.
        const COMPACT               = 1 << 0;
        /// Request that the new region be at the specified offset in its parent region.
        const SPECIFIC              = 1 << 1;
        /// Like SPECIFIC, but permits overwriting existing mappings.  This
        /// flag will not overwrite through a subregion.
        const SPECIFIC_OVERWRITE    = 1 << 2;
        /// Allow VmMappings to be created inside the new region with the SPECIFIC or
        /// OFFSET_IS_UPPER_LIMIT flag.
        const CAN_MAP_SPECIFIC      = 1 << 3;
        /// Allow VmMappings to be created inside the region with read permissions.
        const CAN_MAP_READ          = 1 << 4;
        /// Allow VmMappings to be created inside the region with write permissions.
        const CAN_MAP_WRITE         = 1 << 5;
        /// Allow VmMappings to be created inside the region with execute permissions.
        const CAN_MAP_EXECUTE       = 1 << 6;
        /// Require that VMO backing the mapping is non-resizable.
        const REQUIRE_NON_RESIZABLE = 1 << 7;
        /// Treat the offset as an upper limit when allocating a VMO or child VMAR.
        const ALLOW_FAULTS          = 1 << 8;

        /// Allow VmMappings to be created inside the region with read, write and execute permissions.
        const CAN_MAP_RXW           = Self::CAN_MAP_READ.bits | Self::CAN_MAP_EXECUTE.bits | Self::CAN_MAP_WRITE.bits;
        /// Creation flags for root VmAddressRegion
        const ROOT_FLAGS            = Self::CAN_MAP_RXW.bits | Self::CAN_MAP_SPECIFIC.bits;
    }
}

/// Virtual Memory Address Regions
pub struct VmAddressRegion {
    flags: VmarFlags,
    base: KObjectBase,
    _counter: CountHelper,
    addr: VirtAddr,
    size: usize,
    parent: Option<Arc<VmAddressRegion>>,
    page_table: Arc<Mutex<dyn GenericPageTable>>,
    /// If inner is None, this region is destroyed, all operations are invalid.
    inner: Mutex<Option<VmarInner>>,
}

impl_kobject!(VmAddressRegion);
define_count_helper!(VmAddressRegion);

/// The mutable part of `VmAddressRegion`.
#[derive(Default)]
struct VmarInner {
    children: Vec<Arc<VmAddressRegion>>,
    mappings: Vec<Arc<VmMapping>>,
}

impl VmAddressRegion {
    /// Create a new root VMAR.
    pub fn new_root() -> Arc<Self> {
        #[cfg(feature = "aspace-separate")]
        let (addr, size) = {
            use core::sync::atomic::*;
            static VMAR_ID: AtomicUsize = AtomicUsize::new(0);
            let i = VMAR_ID.fetch_add(1, Ordering::SeqCst);
            (0x2_0000_0000 + 0x100_0000_0000 * i, 0x100_0000_0000)
        };
        #[cfg(not(feature = "aspace-separate"))]
        let (addr, size) = (USER_ASPACE_BASE as usize, USER_ASPACE_SIZE as usize);
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr,
            size,
            parent: None,
            page_table: Arc::new(Mutex::new(PageTable::from_current().clone_kernel())), //hal PageTable
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a kernel root VMAR.
    pub fn new_kernel() -> Arc<Self> {
        let kernel_vmar_base = KERNEL_ASPACE_BASE as usize; // Sorry i hard code because i'm lazy
        let kernel_vmar_size = KERNEL_ASPACE_SIZE as usize;
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr: kernel_vmar_base,
            size: kernel_vmar_size,
            parent: None,
            page_table: Arc::new(Mutex::new(PageTable::from_current().clone_kernel())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a VMAR for guest physical memory.
    #[cfg(feature = "hypervisor")]
    pub fn new_guest() -> Arc<Self> {
        let guest_vmar_base = crate::hypervisor::GUEST_PHYSICAL_ASPACE_BASE as usize;
        let guest_vmar_size = crate::hypervisor::GUEST_PHYSICAL_ASPACE_SIZE as usize;
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr: guest_vmar_base,
            size: guest_vmar_size,
            parent: None,
            page_table: Arc::new(Mutex::new(crate::hypervisor::VmmPageTable::new())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a child VMAR at the `offset`.
    pub fn allocate_at(
        self: &Arc<Self>,
        offset: usize,
        len: usize,
        flags: VmarFlags,
        align: usize,
    ) -> ZxResult<Arc<Self>> {
        self.allocate(Some(offset), len, flags, align)
    }

    /// Create a child VMAR with optional `offset`.
    pub fn allocate(
        self: &Arc<Self>,
        offset: Option<usize>,
        len: usize,
        flags: VmarFlags,
        align: usize,
    ) -> ZxResult<Arc<Self>> {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        let offset = self.determine_offset(inner, offset, len, align)?;
        let child = Arc::new(VmAddressRegion {
            flags,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
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
        vmar_offset: usize,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<VirtAddr> {
        self.map(Some(vmar_offset), vmo, vmo_offset, len, flags)
    }

    /// Map the `vmo` into this VMAR.
    pub fn map(
        &self,
        vmar_offset: Option<usize>,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<VirtAddr> {
        self.map_ext(
            vmar_offset,
            vmo,
            vmo_offset,
            len,
            MMUFlags::RXW,
            flags,
            false,
            true,
        )
    }

    /// Map the `vmo` into this VMAR.
    #[allow(clippy::too_many_arguments)]
    pub fn map_ext(
        &self,
        vmar_offset: Option<usize>,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        permissions: MMUFlags,
        flags: MMUFlags,
        overwrite: bool,
        map_range: bool,
    ) -> ZxResult<VirtAddr> {
        if !page_aligned(vmo_offset) || !page_aligned(len) || vmo_offset.overflowing_add(len).1 {
            return Err(ZxError::INVALID_ARGS);
        }
        if !permissions.contains(flags & MMUFlags::RXW) {
            return Err(ZxError::ACCESS_DENIED);
        }
        // TODO: allow the mapping extends past the end of vmo
        if vmo_offset > vmo.len() || len > vmo.len() - vmo_offset {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        let offset = self.determine_offset(inner, vmar_offset, len, PAGE_SIZE)?;
        let addr = self.addr + offset;
        let mut flags = flags;
        // if vmo != 0
        {
            flags |= MMUFlags::from_bits_truncate(vmo.cache_policy() as usize);
        }
        // align = 1K? 2K? 4K? 8K? ...
        if !self.test_map(inner, offset, len, PAGE_SIZE) {
            if overwrite {
                self.unmap_inner(addr, len, inner)?;
            } else {
                return Err(ZxError::NO_MEMORY);
            }
        }
        // TODO: Fix map_range bugs and remove this line
        let map_range = map_range || vmo.name() != "";
        let mapping = VmMapping::new(
            addr,
            len,
            vmo,
            vmo_offset,
            permissions,
            flags,
            self.page_table.clone(),
        );
        if map_range {
            mapping.map()?;
        }
        inner.mappings.push(mapping);
        Ok(addr)
    }

    /// Unmaps all VMO mappings and destroys all sub-regions within the absolute range
    /// including `addr` and ending before exclusively at `addr + len`.
    /// Any sub-region that is in the range must be fully in the range
    /// (i.e. partial overlaps are an error).
    /// If a mapping is only partially in the range, the mapping is split and the requested
    /// portion is unmapped.
    pub fn unmap(&self, addr: VirtAddr, len: usize) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        self.unmap_inner(addr, len, inner)
    }

    /// Must hold self.inner.lock() before calling.
    fn unmap_inner(&self, addr: VirtAddr, len: usize, inner: &mut VmarInner) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }

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
            map.size() == 0
        });
        inner.mappings.extend(new_maps);
        for vmar in inner.children.drain_filter(|vmar| vmar.within(begin, end)) {
            vmar.destroy_internal()?;
        }
        Ok(())
    }

    /// Change protections on a subset of the region of memory in the containing
    /// address space.  If the requested range overlaps with a subregion,
    /// protect() will fail.
    pub fn protect(&self, addr: usize, len: usize, flags: MMUFlags) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        let end_addr = addr + len;
        // check if there are overlapping subregion
        if inner
            .children
            .iter()
            .any(|child| child.end_addr() >= addr && child.addr() <= end_addr)
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let length: usize = inner
            .mappings
            .iter()
            .filter_map(|map| {
                if map.end_addr() >= addr && map.addr() <= end_addr {
                    Some(end_addr.min(map.end_addr()) - addr.max(map.addr()))
                } else {
                    None
                }
            })
            .sum();
        if length != len {
            return Err(ZxError::NOT_FOUND);
        }
        // check if protect flags is valid
        if inner
            .mappings
            .iter()
            .filter(|map| map.end_addr() >= addr && map.addr() <= end_addr) // get mappings in range: [addr, end_addr]
            .any(|map| !map.is_valid_mapping_flags(flags))
        {
            return Err(ZxError::ACCESS_DENIED);
        }
        inner
            .mappings
            .iter()
            .filter(|map| map.end_addr() >= addr && map.addr() <= end_addr)
            .for_each(|map| {
                let start_index = pages(addr.max(map.addr()) - map.addr());
                let end_index = pages(end_addr.min(map.end_addr()) - map.addr());
                map.protect(flags, start_index, end_index);
            });
        Ok(())
    }

    /// Unmap all mappings within the VMAR, and destroy all sub-regions of the region.
    pub fn destroy(self: &Arc<Self>) -> ZxResult {
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
    fn destroy_internal(&self) -> ZxResult {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        for vmar in inner.children.drain(..) {
            vmar.destroy_internal()?;
        }
        for mapping in inner.mappings.drain(..) {
            drop(mapping);
        }
        *guard = None;
        Ok(())
    }

    /// Unmap all mappings and destroy all sub-regions of VMAR.
    pub fn clear(&self) -> ZxResult {
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

    /// Whether this VMAR is dead.
    pub fn is_dead(&self) -> bool {
        self.inner.lock().is_none()
    }

    /// Whether this VMAR is alive.
    pub fn is_alive(&self) -> bool {
        !self.is_dead()
    }

    /// Get flags of vaddr
    pub fn get_vaddr_flags(&self, vaddr: usize) -> PagingResult<MMUFlags> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        if !self.contains(vaddr) {
            return Err(PagingError::NotMapped);
        }
        if let Some(child) = inner.children.iter().find(|ch| ch.contains(vaddr)) {
            return child.get_vaddr_flags(vaddr);
        }
        if let Some(mapping) = inner.mappings.iter().find(|map| map.contains(vaddr)) {
            return mapping.query_vaddr(vaddr).map(|(_, flags, _)| flags);
        }
        Err(PagingError::NoMemory)
    }

    /// Determine final address with given input `offset` and `len`.
    fn determine_offset(
        &self,
        inner: &VmarInner,
        offset: Option<usize>,
        len: usize,
        align: usize,
    ) -> ZxResult<VirtAddr> {
        if !check_aligned(len, align) {
            Err(ZxError::INVALID_ARGS)
        } else if let Some(offset) = offset {
            if check_aligned(offset, align) && self.test_map(inner, offset, len, align) {
                Ok(offset)
            } else {
                Err(ZxError::INVALID_ARGS)
            }
        } else if len > self.size {
            Err(ZxError::INVALID_ARGS)
        } else {
            match self.find_free_area(inner, 0, len, align) {
                Some(offset) => Ok(offset),
                None => Err(ZxError::NO_MEMORY),
            }
        }
    }

    /// Test if can create a new mapping at `offset` with `len`.
    fn test_map(&self, inner: &VmarInner, offset: usize, len: usize, align: usize) -> bool {
        debug_assert!(check_aligned(offset, align));
        debug_assert!(check_aligned(len, align));
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
    fn find_free_area(
        &self,
        inner: &VmarInner,
        offset_hint: usize,
        len: usize,
        align: usize,
    ) -> Option<usize> {
        // TODO: randomize
        debug_assert!(check_aligned(offset_hint, align));
        debug_assert!(check_aligned(len, align));
        // brute force:
        // try each area's end address as the start
        core::iter::once(offset_hint)
            .chain(inner.children.iter().map(|map| map.end_addr() - self.addr))
            .chain(inner.mappings.iter().map(|map| map.end_addr() - self.addr))
            .find(|&offset| self.test_map(inner, offset, len, align))
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

    /// return true if vmar contains vaddr, or return false.
    pub fn contains(&self, vaddr: VirtAddr) -> bool {
        self.addr <= vaddr && vaddr < self.end_addr()
    }

    /// Get information of this VmAddressRegion
    pub fn get_info(&self) -> VmarInfo {
        // pub fn get_info(&self, va: usize) -> VmarInfo {
        // let _r = self.page_table.lock().query(va);
        VmarInfo {
            base: self.addr(),
            len: self.size,
            // pg_token: self.page_table.lock().table_phys(),
        }
    }

    /// Get VmarFlags of this VMAR.
    pub fn get_flags(&self) -> VmarFlags {
        self.flags
    }

    /// Dump all mappings recursively.
    pub fn dump(&self) {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        for map in inner.mappings.iter() {
            debug!("{:#x?}", map);
        }
        for child in inner.children.iter() {
            child.dump();
        }
    }

    /// Get base address of vdso.
    pub fn vdso_base_addr(&self) -> Option<usize> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        for map in inner.mappings.iter() {
            if map.vmo.name().starts_with("vdso") && map.inner.lock().vmo_offset == 0x7000 {
                return Some(map.addr());
            }
        }
        for vmar in inner.children.iter() {
            if let Some(addr) = vmar.vdso_base_addr() {
                return Some(addr);
            }
        }
        None
    }

    /// Handle page fault happened on this VMAR.
    ///
    /// The fault virtual address is `vaddr` and the reason is in `flags`.
    pub fn handle_page_fault(&self, vaddr: VirtAddr, flags: MMUFlags) -> ZxResult {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        if !self.contains(vaddr) {
            return Err(ZxError::NOT_FOUND);
        }
        if let Some(child) = inner.children.iter().find(|ch| ch.contains(vaddr)) {
            return child.handle_page_fault(vaddr, flags);
        }
        if let Some(mapping) = inner.mappings.iter().find(|map| map.contains(vaddr)) {
            return mapping.handle_page_fault(vaddr, flags);
        }
        Err(ZxError::NOT_FOUND)
    }

    fn for_each_mapping(&self, f: &mut impl FnMut(&Arc<VmMapping>)) {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        for map in inner.mappings.iter() {
            f(map);
        }
        for child in inner.children.iter() {
            child.for_each_mapping(f);
        }
    }

    /// Clone the entire address space and VMOs from source VMAR. (For Linux fork)
    pub fn fork_from(&self, src: &Arc<Self>) -> ZxResult {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        inner.fork_from(src, &self.page_table)
    }

    /// Returns statistics about memory used by a task.
    pub fn get_task_stats(&self) -> TaskStatsInfo {
        let mut task_stats = TaskStatsInfo::default();
        self.for_each_mapping(&mut |map| map.fill_in_task_status(&mut task_stats));
        task_stats
    }

    /// Read from address space.
    ///
    /// Return the actual number of bytes read.
    pub fn read_memory(&self, vaddr: usize, buf: &mut [u8]) -> ZxResult<usize> {
        // TODO: support multiple VMOs
        let map = self.find_mapping(vaddr).ok_or(ZxError::NO_MEMORY)?;
        let map_inner = map.inner.lock();
        let vmo_offset = vaddr - map_inner.addr + map_inner.vmo_offset;
        let size_limit = map_inner.addr + map_inner.size - vaddr;
        let actual_size = buf.len().min(size_limit);
        map.vmo.read(vmo_offset, &mut buf[0..actual_size])?;
        Ok(actual_size)
    }

    /// Write to address space.
    ///
    /// Return the actual number of bytes written.
    pub fn write_memory(&self, vaddr: usize, buf: &[u8]) -> ZxResult<usize> {
        // TODO: support multiple VMOs
        let map = self.find_mapping(vaddr).ok_or(ZxError::NO_MEMORY)?;
        let map_inner = map.inner.lock();
        let vmo_offset = vaddr - map_inner.addr + map_inner.vmo_offset;
        let size_limit = map_inner.addr + map_inner.size - vaddr;
        let actual_size = buf.len().min(size_limit);
        map.vmo.write(vmo_offset, &buf[0..actual_size])?;
        Ok(actual_size)
    }

    /// Find mapping of vaddr
    pub fn find_mapping(&self, vaddr: usize) -> Option<Arc<VmMapping>> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        if let Some(mapping) = inner.mappings.iter().find(|map| map.contains(vaddr)) {
            return Some(mapping.clone());
        }
        if let Some(child) = inner.children.iter().find(|ch| ch.contains(vaddr)) {
            return child.find_mapping(vaddr);
        }
        None
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
        let map_size: usize = inner.mappings.iter().map(|map| map.size()).sum();
        let vmar_size: usize = inner.children.iter().map(|vmar| vmar.size).sum();
        map_size + vmar_size
    }
}

impl VmarInner {
    /// Clone the entire address space and VMOs from source VMAR. (For Linux fork)
    fn fork_from(
        &mut self,
        src: &Arc<VmAddressRegion>,
        page_table: &Arc<Mutex<dyn GenericPageTable>>,
    ) -> ZxResult {
        let src_guard = src.inner.lock();
        let src_inner = src_guard.as_ref().unwrap();
        for child in src_inner.children.iter() {
            self.fork_from(child, page_table)?;
        }
        for map in src_inner.mappings.iter() {
            let mapping = map.clone_map(page_table.clone())?;
            mapping.map()?;
            self.mappings.push(mapping);
        }
        Ok(())
    }
}

/// Information of a VmAddressRegion.
#[repr(C)]
#[derive(Debug)]
pub struct VmarInfo {
    base: usize,
    len: usize,
    // pg_token: usize,
}

/// Virtual Memory Mapping
pub struct VmMapping {
    /// The permission limitation of the vmar
    permissions: MMUFlags,
    vmo: Arc<VmObject>,
    page_table: Arc<Mutex<dyn GenericPageTable>>,
    inner: Mutex<VmMappingInner>,
}

#[derive(Debug, Clone)]
struct VmMappingInner {
    /// The actual flags used in the mapping of each page
    flags: Vec<MMUFlags>,
    addr: VirtAddr,
    size: usize,
    vmo_offset: usize,
}

/// Statistics about resources (e.g., memory) used by a task.
#[repr(C)]
#[derive(Default)]
pub struct TaskStatsInfo {
    mapped_bytes: u64,
    private_bytes: u64,
    shared_bytes: u64,
    scaled_shared_bytes: u64,
}

impl core::fmt::Debug for VmMapping {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("VmMapping")
            .field("addr", &inner.addr)
            .field("size", &inner.size)
            .field("permissions", &self.permissions)
            // .field("flags", &inner.flags)
            .field("vmo_id", &self.vmo.id())
            .field("vmo_offset", &inner.vmo_offset)
            .finish()
    }
}

impl VmMapping {
    fn new(
        addr: VirtAddr,
        size: usize,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        permissions: MMUFlags,
        flags: MMUFlags,
        page_table: Arc<Mutex<dyn GenericPageTable>>,
    ) -> Arc<Self> {
        let mapping = Arc::new(VmMapping {
            inner: Mutex::new(VmMappingInner {
                flags: vec![flags; pages(size)],
                addr,
                size,
                vmo_offset,
            }),
            permissions,
            page_table,
            vmo: vmo.clone(),
        });
        vmo.append_mapping(Arc::downgrade(&mapping));
        mapping
    }

    /// Map range and commit.
    /// Commit pages to vmo, and map those to frames in page_table.
    /// Temporarily used for development. A standard procedure for
    /// vmo is: create_vmo, op_range(commit), map
    fn map(self: &Arc<Self>) -> ZxResult {
        self.vmo.commit_pages_with(&mut |commit| {
            let inner = self.inner.lock();
            let mut page_table = self.page_table.lock();
            let page_num = inner.size / PAGE_SIZE;
            let vmo_offset = inner.vmo_offset / PAGE_SIZE;
            for i in 0..page_num {
                let paddr = commit(vmo_offset + i, inner.flags[i])?;
                //通过GenericPageTable的hal_pt_map进行页表映射
                page_table
                    .map(
                        Page::new_aligned(inner.addr + i * PAGE_SIZE, PageSize::Size4K),
                        paddr,
                        inner.flags[i],
                    )
                    .expect("failed to map");
            }
            Ok(())
        })
    }

    fn unmap(&self) {
        let inner = self.inner.lock();
        // TODO inner.vmo_offset unused?
        self.page_table
            .lock()
            .unmap_cont(inner.addr, inner.size)
            .expect("failed to unmap")
    }

    fn fill_in_task_status(&self, task_stats: &mut TaskStatsInfo) {
        let (start_idx, end_idx) = {
            let inner = self.inner.lock();
            let start_idx = inner.vmo_offset / PAGE_SIZE;
            (start_idx, start_idx + inner.size / PAGE_SIZE)
        };
        task_stats.mapped_bytes += self.vmo.len() as u64;
        let committed_pages = self.vmo.committed_pages_in_range(start_idx, end_idx);
        let share_count = self.vmo.share_count();
        if share_count == 1 {
            task_stats.private_bytes += (committed_pages * PAGE_SIZE) as u64;
        } else {
            task_stats.shared_bytes += (committed_pages * PAGE_SIZE) as u64;
            task_stats.scaled_shared_bytes += (committed_pages * PAGE_SIZE / share_count) as u64;
        }
    }

    /// Cut and unmap regions in `[begin, end)`.
    ///
    /// If it will be split, return another one.
    fn cut(&self, begin: VirtAddr, end: VirtAddr) -> Option<Arc<Self>> {
        if !self.overlap(begin, end) {
            return None;
        }
        let mut inner = self.inner.lock();
        let mut page_table = self.page_table.lock();
        if inner.addr >= begin && inner.end_addr() <= end {
            // subset: [xxxxxxxxxx]
            page_table
                .unmap_cont(inner.addr, inner.size)
                .expect("failed to unmap");
            inner.size = 0;
            inner.flags.clear();
            None
        } else if inner.addr >= begin && inner.addr < end {
            // prefix: [xxxx------]
            let cut_len = end - inner.addr;
            page_table
                .unmap_cont(inner.addr, cut_len)
                .expect("failed to unmap");
            inner.addr = end;
            inner.size -= cut_len;
            inner.vmo_offset += cut_len;
            inner.flags.drain(0..pages(cut_len));
            None
        } else if inner.end_addr() <= end && inner.end_addr() > begin {
            // postfix: [------xxxx]
            let cut_len = inner.end_addr() - begin;
            let new_len = begin - inner.addr;
            page_table
                .unmap_cont(begin, cut_len)
                .expect("failed to unmap");
            inner.size = new_len;
            inner.flags.truncate(new_len);
            None
        } else {
            // superset: [---xxxx---]
            let cut_len = end - begin;
            let new_len1 = begin - inner.addr;
            let new_len2 = inner.end_addr() - end;
            page_table
                .unmap_cont(begin, cut_len)
                .expect("failed to unmap");
            let new_flags_range = (pages(inner.size) - pages(new_len2))..pages(inner.size);
            let new_mapping = Arc::new(VmMapping {
                permissions: self.permissions,
                vmo: self.vmo.clone(),
                page_table: self.page_table.clone(),
                inner: Mutex::new(VmMappingInner {
                    flags: inner.flags.drain(new_flags_range).collect(),
                    addr: end,
                    size: new_len2,
                    vmo_offset: inner.vmo_offset + (end - inner.addr),
                }),
            });
            inner.size = new_len1;
            inner.flags.truncate(new_len1);
            Some(new_mapping)
        }
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        let inner = self.inner.lock();
        !(inner.addr >= end || inner.end_addr() <= begin)
    }

    fn contains(&self, vaddr: VirtAddr) -> bool {
        let inner = self.inner.lock();
        inner.addr <= vaddr && vaddr < inner.end_addr()
    }

    fn is_valid_mapping_flags(&self, flags: MMUFlags) -> bool {
        self.permissions.contains(flags & MMUFlags::RXW)
    }

    fn protect(&self, flags: MMUFlags, start_index: usize, end_index: usize) {
        let mut inner = self.inner.lock();
        let mut pg_table = self.page_table.lock();
        for i in start_index..end_index {
            let mut new_flags = inner.flags[i];
            new_flags.remove(MMUFlags::RXW);
            new_flags.insert(flags & MMUFlags::RXW);
            inner.flags[i] = new_flags;
            pg_table
                .update(inner.addr + i * PAGE_SIZE, None, Some(new_flags))
                .ignore()
                .unwrap();
        }
    }

    fn size(&self) -> usize {
        self.inner.lock().size
    }

    fn addr(&self) -> VirtAddr {
        self.inner.lock().addr
    }

    fn end_addr(&self) -> VirtAddr {
        self.inner.lock().end_addr()
    }

    /// Get MMUFlags of this VmMapping.
    pub fn get_flags(&self, vaddr: usize) -> ZxResult<MMUFlags> {
        if self.contains(vaddr) {
            let page_id = (vaddr - self.addr()) / PAGE_SIZE;
            Ok(self.inner.lock().flags[page_id])
        } else {
            Err(ZxError::NO_MEMORY)
        }
    }

    /// query vaddr's PhysAddr, PhysAddr, PageSize.
    pub fn query_vaddr(&self, vaddr: usize) -> PagingResult<(PhysAddr, MMUFlags, PageSize)> {
        self.page_table.lock().query(vaddr)
    }

    /// Remove WRITE flag from the mappings for Copy-on-Write.
    pub(super) fn range_change(&self, offset: usize, len: usize, op: RangeChangeOp) {
        let inner = self.inner.try_lock();
        // If we are already locked, we are handling page fault/map range
        // In this case we can just ignore the operation since we will update the mapping later
        if let Some(inner) = inner {
            let start = offset.max(inner.vmo_offset);
            let end = (inner.vmo_offset + inner.size / PAGE_SIZE).min(offset + len);
            if !(start..end).is_empty() {
                let mut pg_table = self.page_table.lock();
                for i in (start - inner.vmo_offset)..(end - inner.vmo_offset) {
                    match op {
                        RangeChangeOp::RemoveWrite => {
                            let mut new_flag = inner.flags[i];
                            new_flag.remove(MMUFlags::WRITE);
                            pg_table
                                .update(inner.addr + i * PAGE_SIZE, None, Some(new_flag))
                                .ignore()
                                .unwrap();
                        }
                        RangeChangeOp::Unmap => {
                            pg_table.unmap(inner.addr + i * PAGE_SIZE).ignore().unwrap();
                        }
                    };
                }
            }
        }
    }

    /// Handle page fault happened on this VmMapping.
    pub(crate) fn handle_page_fault(&self, vaddr: VirtAddr, access_flags: MMUFlags) -> ZxResult {
        let vaddr = round_down_pages(vaddr);
        let (vmo_offset, mut flags) = {
            let inner = self.inner.lock();
            let offset = vaddr - inner.addr;
            (offset + inner.vmo_offset, inner.flags[offset / PAGE_SIZE])
        };
        // error!("page fault: addr = {:x}, access_flag = {:?}, flags = {:?}", vaddr, access_flags, flags);
        if !flags.contains(access_flags) {
            return Err(ZxError::ACCESS_DENIED);
        }
        // 当 PF 发生的时候，如果只要求读权限，则即便可写也现只给读权限。
        // 这是由于 COW 会去掉写权限来在写的时候触发 PF，如果直接把 flags 放进去，会导致 COW 的这个操作失效。
        if !access_flags.contains(MMUFlags::WRITE) {
            flags.remove(MMUFlags::WRITE);
        }
        let paddr = self.vmo.commit_page(vmo_offset / PAGE_SIZE, access_flags)?;
        // error!("paddr = {:x}", paddr);
        let mut pg_table = self.page_table.lock();
        let mut res = pg_table.map(Page::new_aligned(vaddr, PageSize::Size4K), paddr, flags);
        if let Err(PagingError::AlreadyMapped) = res {
            res = pg_table.update(vaddr, Some(paddr), Some(flags)).map(|_| ());
        }
        res.map_err(|_| ZxError::ACCESS_DENIED)?;
        Ok(())
    }

    /// Clone VMO and map it to a new page table. (For Linux)
    fn clone_map(&self, page_table: Arc<Mutex<dyn GenericPageTable>>) -> ZxResult<Arc<Self>> {
        //这里调用 hal protect 后, protect() 好像会破坏页表
        let new_vmo = self.vmo.create_child(false, 0, self.vmo.len())?;
        let mapping = Arc::new(VmMapping {
            inner: Mutex::new(self.inner.lock().clone()),
            permissions: self.permissions,
            page_table,
            vmo: new_vmo.clone(),
        });
        new_vmo.append_mapping(Arc::downgrade(&mapping));
        Ok(mapping)
    }
}

impl VmMappingInner {
    fn end_addr(&self) -> VirtAddr {
        self.addr + self.size
    }
}

impl Drop for VmMapping {
    fn drop(&mut self) {
        self.unmap();
    }
}

/// The base of kernel address space
/// In x86 fuchsia this is 0xffff_ff80_0000_0000 instead
pub const KERNEL_ASPACE_BASE: u64 = 0xffff_ff02_0000_0000;
/// The size of kernel address space
pub const KERNEL_ASPACE_SIZE: u64 = 0x0000_0080_0000_0000;
/// The base of user address space
pub const USER_ASPACE_BASE: u64 = 0;
// pub const USER_ASPACE_BASE: u64 = 0x0000_0000_0100_0000;
/// The size of user address space
pub const USER_ASPACE_SIZE: u64 = (1u64 << 47) - 4096 - USER_ASPACE_BASE;
/// The default number of user stack pages
pub const USER_STACK_PAGES: usize = 128;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_child() {
        let root_vmar = VmAddressRegion::new_root();
        let child = root_vmar
            .allocate_at(0, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .expect("failed to create child VMAR");

        // test invalid argument
        assert_eq!(
            root_vmar
                .allocate_at(0x2001, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar
                .allocate_at(0x2000, 1, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar
                .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            child
                .allocate_at(0x1000, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
    }

    /// A valid virtual address base to mmap.
    const MAGIC: usize = 0xdead_beaf;

    #[test]
    #[allow(unsafe_code)]
    fn map() {
        let vmar = VmAddressRegion::new_root();
        let vmo = VmObject::new_paged(4);
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
            let child1 = root
                .allocate_at(0, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let child2 = root
                .allocate_at(0x2000, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let grandson1 = child1
                .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let grandson2 = child1
                .allocate_at(0x1000, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
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
        let base = s.root.addr();
        assert_eq!(
            s.root.unmap(base + 0x1000, 0x2000),
            Err(ZxError::INVALID_ARGS)
        );

        // unmap nothing should success.
        let s = Sample::new();
        let base = s.root.addr();
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
        assert!(s
            .root
            .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .is_ok());
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
        let vmo = VmObject::new_paged(5);
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

    #[test]
    #[allow(unsafe_code)]
    fn copy_on_write_update_mapping() {
        let vmar = VmAddressRegion::new_root();
        let vmo = VmObject::new_paged(1);
        vmo.test_write(0, 1);
        vmar.map_at(0, vmo.clone(), 0, PAGE_SIZE, MMUFlags::RXW)
            .unwrap();
        let child_vmo = vmo.create_child(false, 0, 1 * PAGE_SIZE).unwrap();
        // The clone was created after the map, so the two vmo share pages.
        assert_eq!(
            vmo.commit_page(0, MMUFlags::READ),
            child_vmo.commit_page(0, MMUFlags::READ)
        );
        assert_eq!(vmo.test_read(0), 1);
        assert_eq!(child_vmo.test_read(0), 1);
        unsafe {
            assert_eq!((vmar.addr() as *const u8).read(), 1);
        }
        vmo.test_write(0, 2);
        // Here, since the page was copied on write, the actual page used in the vmo should be changed.
        assert_ne!(
            vmo.commit_page(0, MMUFlags::READ),
            child_vmo.commit_page(0, MMUFlags::READ)
        );
        assert_eq!(vmo.test_read(0), 2);
        assert_eq!(child_vmo.test_read(0), 1);
        // The mapping should update to reflect this change.
        // Since we do not have page fault handler in the libOS,
        // so manually simulate the page fault before read to it
        vmar.handle_page_fault(vmar.addr(), MMUFlags::READ).unwrap();
        unsafe {
            assert_eq!((vmar.addr() as *const u8).read(), 2);
        }
    }
}

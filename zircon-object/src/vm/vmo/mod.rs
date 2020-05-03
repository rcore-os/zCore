use {
    self::{paged::*, physical::*},
    super::*,
    crate::object::*,
    alloc::{
        sync::{Arc, Weak},
        vec::Vec,
    },
    bitflags::bitflags,
    core::ops::Deref,
    kernel_hal::{CachePolicy, PageTable},
    spin::Mutex,
};

mod paged;
mod physical;

kcounter!(VMO_PAGE_ALLOC, "vmo.page_alloc");
kcounter!(VMO_PAGE_DEALLOC, "vmo.page_dealloc");

pub fn vmo_page_bytes() -> usize {
    (VMO_PAGE_ALLOC.get() - VMO_PAGE_DEALLOC.get()) * PAGE_SIZE
}

/// Virtual Memory Object Trait
#[allow(clippy::len_without_is_empty)]
pub trait VMObjectTrait: Sync + Send {
    /// Read memory to `buf` from VMO at `offset`.
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult;

    /// Write memory from `buf` to VMO at `offset`.
    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult;

    /// Get the length of VMO.
    fn len(&self) -> usize;

    /// Set the length of VMO.
    fn set_len(&self, len: usize) -> ZxResult;

    /// Unmap physical memory from `page_table`.
    fn unmap_from(&self, page_table: &mut PageTable, vaddr: VirtAddr, _offset: usize, len: usize) {
        // TODO _offset unused?
        let pages = len / PAGE_SIZE;
        page_table
            .unmap_cont(vaddr, pages)
            .expect("failed to unmap")
    }

    /// Commit a page.
    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr>;

    /// Commit allocating physical memory.
    fn commit(&self, offset: usize, len: usize) -> ZxResult;

    /// Decommit allocated physical memory.
    fn decommit(&self, offset: usize, len: usize) -> ZxResult;

    /// Create a child VMO.
    fn create_child(
        &self,
        offset: usize,
        len: usize,
        user_id: KoID,
    ) -> ZxResult<Arc<dyn VMObjectTrait>>;

    fn create_slice(
        self: Arc<Self>,
        id: KoID,
        offset: usize,
        len: usize,
    ) -> ZxResult<Arc<dyn VMObjectTrait>>;

    fn append_mapping(&self, mapping: Weak<VmMapping>);

    fn remove_mapping(&self, mapping: Weak<VmMapping>);

    fn complete_info(&self, info: &mut ZxInfoVmo);

    fn get_cache_policy(&self) -> CachePolicy;

    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult;

    fn share_count(&self) -> usize;

    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize;

    fn pin(&self, _offset: usize, _len: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    fn unpin(&self, _offset: usize, _len:usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    fn is_contiguous(&self) -> bool {
        return false;
    }

    fn is_paged(&self) -> bool {
        return false;
    }
}

pub struct VmObject {
    base: KObjectBase,
    parent: Mutex<Weak<VmObject>>, // Parent could be changed
    children: Mutex<Vec<Weak<VmObject>>>,
    _counter: CountHelper,
    resizable: bool,
    inner: Arc<dyn VMObjectTrait>,
}

impl_kobject!(VmObject);
define_count_helper!(VmObject);

impl VmObject {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new_paged(pages: usize) -> Arc<Self> {
        Self::new_paged_with_resizable(false, pages)
    }

    pub fn new_paged_with_resizable(resizable: bool, pages: usize) -> Arc<Self> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        Arc::new(VmObject {
            parent: Mutex::new(Default::default()),
            children: Mutex::new(Vec::new()),
            resizable,
            _counter: CountHelper::new(),
            inner: VMObjectPaged::new(base.id, pages),
            base,
        })
    }

    /// Create a new VMO representing a piece of contiguous physical memory.
    ///
    /// # Safety
    ///
    /// You must ensure nobody has the ownership of this piece of memory yet.
    #[allow(unsafe_code)]
    pub unsafe fn new_physical(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            parent: Mutex::new(Default::default()),
            children: Mutex::new(Vec::new()),
            resizable: true,
            _counter: CountHelper::new(),
            inner: VMObjectPhysical::new(paddr, pages),
        })
    }

    /// Create a child VMO.
    pub fn create_child(
        self: &Arc<Self>,
        resizable: bool,
        offset: usize,
        len: usize,
    ) -> ZxResult<Arc<Self>> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        base.set_name(&self.base.name());
        let inner = self.inner.create_child(offset, len, base.id)?;
        let child = Arc::new(VmObject {
            parent: Mutex::new(Arc::downgrade(self)),
            children: Mutex::new(Vec::new()),
            resizable,
            _counter: CountHelper::new(),
            inner: inner,
            base,
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Create a child slice as an VMO
    pub fn create_slice(self: &Arc<Self>, offset: usize, p_size: usize) -> ZxResult<Arc<Self>> {
        let size = roundup_pages(p_size);
        if size < p_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        // child slice must be wholly contained
        let parrent_size = self.inner.len();
        if !page_aligned(offset) {
            return Err(ZxError::INVALID_ARGS);
        }
        if offset > parrent_size || size > parrent_size - offset {
            return Err(ZxError::INVALID_ARGS);
        }
        if self.resizable {
            return Err(ZxError::NOT_SUPPORTED);
        }
        let base = KObjectBase::with(&self.base.name(), Signal::VMO_ZERO_CHILDREN);
        let inner = self.inner.clone().create_slice(base.id, offset, size)?;
        let child = Arc::new(VmObject {
            parent: Mutex::new(Arc::downgrade(self)),
            children: Mutex::new(Vec::new()),
            resizable: false,
            _counter: CountHelper::new(),
            inner,
            base,
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Add child to the list and signal if ZeroChildren signal is active.
    /// If the number of children turns 0 to 1, signal it
    pub fn add_child(&self, child: &Arc<VmObject>) {
        let mut children = self.children.lock();
        children.retain(|x| x.strong_count() != 0);
        children.push(Arc::downgrade(child));
        if children.len() == 1 {
            self.base.signal_clear(Signal::VMO_ZERO_CHILDREN);
        }
    }

    /// Set the length of this VMO if resizable.
    pub fn set_len(&self, len: usize) -> ZxResult {
        let size = roundup_pages(len);
        if size < len {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if self.resizable {
            self.inner.set_len(size)
        } else {
            Err(ZxError::UNAVAILABLE)
        }
    }

    /// Get information of this VMO.
    pub fn get_info(&self) -> ZxInfoVmo {
        let mut ret = ZxInfoVmo {
            koid: self.base.id,
            name: {
                let mut arr = [0u8; 32];
                let name = self.base.name();
                let length = name.len().min(32);
                arr[..length].copy_from_slice(&name.as_bytes()[..length]);
                arr
            },
            size: self.inner.len() as u64,
            parent_koid: self.parent.lock().upgrade().map(|p| p.id()).unwrap_or(0),
            flags: if self.resizable {
                VmoInfoFlags::RESIZABLE
            } else {
                VmoInfoFlags::empty()
            },
            ..Default::default()
        };
        self.inner.complete_info(&mut ret);
        ret
    }

    pub fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult {
        self.inner.set_cache_policy(policy)
    }

    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    pub fn is_contiguous(&self) -> bool {
        self.inner.is_contiguous()
    }
}

impl Deref for VmObject {
    type Target = Arc<dyn VMObjectTrait>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Drop for VmObject {
    fn drop(&mut self) {
        if let Some(parent) = self.parent.lock().upgrade() {
            let mut my_children = {
                let mut my_children = self.children.lock();
                for ch in &mut (*my_children) {
                    if let Some(ch) = ch.upgrade() {
                        let mut ch_parent = ch.parent.lock();
                        *ch_parent = Arc::downgrade(&parent);
                    }
                }
                let mut res: Vec<Weak<VmObject>> = Vec::new();
                res.append(&mut (*my_children));
                res
            };
            let mut children = parent.children.lock();
            children.append(&mut my_children);
            children.retain(|c| c.strong_count() != 0);
            // Non-zero to zero?
            if children.is_empty() {
                parent.base.signal_set(Signal::VMO_ZERO_CHILDREN);
            }
        }
    }
}

/// Describes a VMO.
#[repr(C)]
#[derive(Default)]
pub struct ZxInfoVmo {
    /// The koid of this VMO.
    koid: KoID,
    /// The name of this VMO.
    name: [u8; 32],
    /// The size of this VMO; i.e., the amount of virtual address space it
    /// would consume if mapped.
    size: u64,
    /// If this VMO is a clone, the koid of its parent. Otherwise, zero.
    parent_koid: KoID,
    /// The number of clones of this VMO, if any.
    num_children: u64,
    /// The number of times this VMO is currently mapped into VMARs.
    num_mappings: u64,
    /// The number of unique address space we're mapped into.
    share_count: u64,
    /// Flags.
    pub flags: VmoInfoFlags,
    /// Padding.
    padding1: [u8; 4],
    /// If the type is `PAGED`, the amount of
    /// memory currently allocated to this VMO; i.e., the amount of physical
    /// memory it consumes. Undefined otherwise.
    committed_bytes: u64,
    /// If `flags & ZX_INFO_VMO_VIA_HANDLE`, the handle rights.
    /// Undefined otherwise.
    pub rights: Rights,
    /// VMO mapping cache policy.
    cache_policy: u32,
}

bitflags! {
    #[derive(Default)]
    pub struct VmoInfoFlags: u32 {
        const TYPE_PHYSICAL = 0;
        #[allow(clippy::identity_op)]
        const TYPE_PAGED    = 1 << 0;
        const RESIZABLE     = 1 << 1;
        const IS_COW_CLONE  = 1 << 2;
        const VIA_HANDLE    = 1 << 3;
        const VIA_MAPPING   = 1 << 4;
        const PAGER_BACKED  = 1 << 5;
        const CONTIGUOUS    = 1 << 6;
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum RangeChangeOp {
    Unmap,
    RemoveWrite,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn read_write(vmo: &VmObject) {
        let mut buf = [0u8; 4];
        vmo.write(0, &[0, 1, 2, 3]).unwrap();
        vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[0, 1, 2, 3]);
    }
}

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
    fn set_len(&self, len: usize);

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
        is_slice: bool,
        offset: usize,
        len: usize,
        user_id: KoID,
    ) -> Arc<dyn VMObjectTrait>;

    fn append_mapping(&self, mapping: Weak<VmMapping>);

    fn remove_mapping(&self, mapping: Weak<VmMapping>);

    fn complete_info(&self, info: &mut ZxInfoVmo);

    fn get_cache_policy(&self) -> CachePolicy;

    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult;

    fn share_count(&self) -> usize;

    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize;

    fn zero(&self, offset: usize, len: usize) -> ZxResult;
}

pub struct VmObject {
    base: KObjectBase,
    parent: Weak<VmObject>,
    children: Mutex<Vec<Weak<VmObject>>>,
    _counter: CountHelper,
    resizable: bool,
    is_slice: bool,
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
            parent: Default::default(),
            children: Mutex::new(Vec::new()),
            resizable,
            is_slice: false,
            _counter: CountHelper::new(),
            inner: VMObjectPaged::new(base.id, pages),
            base,
        })
    }

    /// Create a new VMO representing a piece of contiguous physical memory.
    /// You must ensure nobody has the ownership of this piece of memory yet.
    pub fn new_physical(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            parent: Default::default(),
            children: Mutex::new(Vec::new()),
            resizable: true,
            is_slice: false,
            _counter: CountHelper::new(),
            inner: VMObjectPhysical::new(paddr, pages),
        })
    }

    /// Create a child VMO.
    pub fn create_child(
        self: &Arc<Self>,
        is_slice: bool,
        resizable: bool,
        offset: usize,
        len: usize,
    ) -> Arc<Self> {
        assert!(!(is_slice && resizable));
        if self.is_slice {
            assert!(is_slice, "create a not-slice child for a slice parent!!!");
        }
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        base.set_name(&self.base.name());
        let child = Arc::new(VmObject {
            parent: if is_slice && self.is_slice {
                self.parent.clone()
            } else {
                Arc::downgrade(self)
            },
            children: Mutex::new(Vec::new()),
            resizable,
            is_slice,
            _counter: CountHelper::new(),
            inner: self.inner.create_child(is_slice, offset, len, base.id),
            base,
        });
        if self.is_slice {
            let arc_parent = self.parent.upgrade().unwrap();
            arc_parent.children.lock().push(Arc::downgrade(&child));
        } else {
            self.children.lock().push(Arc::downgrade(&child));
        }
        self.base.signal_clear(Signal::VMO_ZERO_CHILDREN);
        child
    }

    /// Set the length of this VMO if resizable.
    pub fn set_len(&self, len: usize) -> ZxResult {
        let size = roundup_pages(len);
        if size < len {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if self.resizable {
            self.inner.set_len(size);
            Ok(())
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
            parent_koid: self.parent.upgrade().map(|p| p.id()).unwrap_or(0),
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

    pub fn is_slice(&self) -> bool {
        self.is_slice
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
        if let Some(parent) = self.parent.upgrade() {
            let mut children = parent.children.lock();
            children.retain(|c| c.strong_count() != 0);
            children.iter().for_each(|child| {
                let arc_child = child.upgrade().unwrap();
                let mut locked_children = arc_child.children.lock();
                locked_children.retain(|c| c.strong_count() != 0);
                if locked_children.is_empty() {
                    arc_child.base.signal_set(Signal::VMO_ZERO_CHILDREN);
                }
            });
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

use {
    self::{paged::*, physical::*, slice::*},
    super::*,
    crate::object::*,
    alloc::{
        sync::{Arc, Weak},
        vec::Vec,
    },
    bitflags::bitflags,
    core::ops::Deref,
    kernel_hal::CachePolicy,
    lock::{Mutex, MutexGuard},
};

mod paged;
mod physical;
mod slice;

kcounter!(VMO_PAGE_ALLOC, "vmo.page_alloc");
kcounter!(VMO_PAGE_DEALLOC, "vmo.page_dealloc");

/// The amount of memory committed to VMOs.
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

    /// Resets the range of bytes in the VMO from `offset` to `offset+len` to 0.
    fn zero(&self, offset: usize, len: usize) -> ZxResult;

    /// Get the length of VMO.
    fn len(&self) -> usize;

    /// Set the length of VMO.
    fn set_len(&self, len: usize) -> ZxResult;

    /// Commit a page.
    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr>;

    /// Commit pages with an external function f.
    /// the vmo is internally locked before it calls f,
    /// allowing `VmMapping` to avoid deadlock
    fn commit_pages_with(
        &self,
        f: &mut dyn FnMut(&mut dyn FnMut(usize, MMUFlags) -> ZxResult<PhysAddr>) -> ZxResult,
    ) -> ZxResult;

    /// Commit allocating physical memory.
    fn commit(&self, offset: usize, len: usize) -> ZxResult;

    /// Decommit allocated physical memory.
    fn decommit(&self, offset: usize, len: usize) -> ZxResult;

    /// Create a child VMO.
    fn create_child(&self, offset: usize, len: usize) -> ZxResult<Arc<dyn VMObjectTrait>>;

    /// Append a mapping to the VMO's mapping list.
    fn append_mapping(&self, _mapping: Weak<VmMapping>) {}

    /// Remove a mapping from the VMO's mapping list.
    fn remove_mapping(&self, _mapping: Weak<VmMapping>) {}

    /// Complete the VmoInfo.
    fn complete_info(&self, info: &mut VmoInfo);

    /// Get the cache policy.
    fn cache_policy(&self) -> CachePolicy;

    /// Set the cache policy.
    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult;

    /// Count committed pages of the VMO.
    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize;

    /// Pin the given range of the VMO.
    fn pin(&self, _offset: usize, _len: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Unpin the given range of the VMO.
    fn unpin(&self, _offset: usize, _len: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Returns true if the object is backed by a contiguous range of physical memory.
    fn is_contiguous(&self) -> bool {
        false
    }

    /// Returns true if the object is backed by RAM.
    fn is_paged(&self) -> bool {
        false
    }

    /// If contiguous, transmute vmo to a mutable buffer
    fn as_mut_buf(&self) -> ZxResult<(MutexGuard<()>, &mut [u8])> {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Mark as not contiguous
    fn unset_contiguous(&self) {}
}

/// Virtual memory containers
///
/// ## SYNOPSIS
///
/// A Virtual Memory Object (VMO) represents a contiguous region of virtual memory
/// that may be mapped into multiple address spaces.
pub struct VmObject {
    base: KObjectBase,
    _counter: CountHelper,
    resizable: bool,
    trait_: Arc<dyn VMObjectTrait>,
    inner: Mutex<VmObjectInner>,
}

impl_kobject!(VmObject);
define_count_helper!(VmObject);

#[derive(Default)]
struct VmObjectInner {
    parent: Weak<VmObject>,
    children: Vec<Weak<VmObject>>,
    mapping_count: usize,
    content_size: usize,
}

impl VmObject {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new_paged(pages: usize) -> Arc<Self> {
        Self::new_paged_with_resizable(false, pages)
    }

    /// Create a new VMO, which can be resizable, backing on physical memory allocated in pages.
    pub fn new_paged_with_resizable(resizable: bool, pages: usize) -> Arc<Self> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        Arc::new(VmObject {
            resizable,
            _counter: CountHelper::new(),
            trait_: VMObjectPaged::new(pages),
            inner: Mutex::new(VmObjectInner::default()),
            base,
        })
    }

    /// Create a new VMO representing a piece of contiguous physical memory.
    pub fn new_physical(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            trait_: VMObjectPhysical::new(paddr, pages),
            inner: Mutex::new(VmObjectInner::default()),
        })
    }

    /// Create a VM object referring to a specific contiguous range of physical frame.  
    pub fn new_contiguous(pages: usize, align_log2: usize) -> ZxResult<Arc<Self>> {
        let vmo = Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            trait_: VMObjectPaged::new_contiguous(pages, align_log2)?,
            inner: Mutex::new(VmObjectInner::default()),
        });
        Ok(vmo)
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
        let trait_ = self.trait_.create_child(offset, len)?;
        let child = Arc::new(VmObject {
            base,
            resizable,
            _counter: CountHelper::new(),
            trait_,
            inner: Mutex::new(VmObjectInner {
                parent: Arc::downgrade(self),
                ..VmObjectInner::default()
            }),
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Create a child slice as an VMO
    pub fn create_slice(self: &Arc<Self>, offset: usize, p_size: usize) -> ZxResult<Arc<Self>> {
        let size = roundup_pages(p_size);
        // why 32 * PAGE_SIZE? Refered to zircon source codes
        if size < p_size || size > usize::MAX & !(32 * PAGE_SIZE) {
            return Err(ZxError::OUT_OF_RANGE);
        }
        // child slice must be wholly contained
        let parent_size = self.trait_.len();
        if !page_aligned(offset) {
            return Err(ZxError::INVALID_ARGS);
        }
        if offset > parent_size || size > parent_size - offset {
            return Err(ZxError::INVALID_ARGS);
        }
        if self.resizable {
            return Err(ZxError::NOT_SUPPORTED);
        }
        if self.trait_.cache_policy() != CachePolicy::Cached && !self.trait_.is_contiguous() {
            return Err(ZxError::BAD_STATE);
        }
        let child = Arc::new(VmObject {
            base: KObjectBase::with(&self.base.name(), Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            trait_: VMObjectSlice::new(self.trait_.clone(), offset, size),
            inner: Mutex::new(VmObjectInner {
                parent: Arc::downgrade(self),
                ..VmObjectInner::default()
            }),
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Add child to the list and signal if ZeroChildren signal is active.
    /// If the number of children turns 0 to 1, signal it
    fn add_child(&self, child: &Arc<VmObject>) {
        let mut inner = self.inner.lock();
        inner.children.retain(|x| x.strong_count() != 0);
        inner.children.push(Arc::downgrade(child));
        if inner.children.len() == 1 {
            self.base.signal_clear(Signal::VMO_ZERO_CHILDREN);
        }
    }

    /// Set the length of this VMO if resizable.
    pub fn set_len(&self, len: usize) -> ZxResult {
        let size = roundup_pages(len);
        if size < len {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if !self.resizable {
            return Err(ZxError::UNAVAILABLE);
        }
        self.trait_.set_len(size)
    }

    /// Set the size of the content stored in the VMO in bytes, resize vmo if needed
    pub fn set_content_size_and_resize(
        &self,
        size: usize,
        zero_until_offset: usize,
    ) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        let content_size = inner.content_size;
        let len = self.trait_.len();
        if size < content_size {
            return Ok(content_size);
        }
        let required_len = roundup_pages(size);
        let new_content_size = if required_len > len && self.set_len(required_len).is_err() {
            len
        } else {
            size
        };
        let zero_until_offset = zero_until_offset.min(new_content_size);
        if zero_until_offset > content_size {
            self.trait_
                .zero(content_size, zero_until_offset - content_size)?;
        }
        inner.content_size = new_content_size;
        Ok(new_content_size)
    }

    /// Get the size of the content stored in the VMO in bytes.
    pub fn content_size(&self) -> usize {
        let inner = self.inner.lock();
        inner.content_size
    }

    /// Get the size of the content stored in the VMO in bytes.
    pub fn set_content_size(&self, size: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        inner.content_size = size;
        Ok(())
    }

    /// Get information of this VMO.
    pub fn get_info(&self) -> VmoInfo {
        let inner = self.inner.lock();
        let mut ret = VmoInfo {
            koid: self.base.id,
            name: {
                let mut arr = [0u8; 32];
                let name = self.base.name();
                let length = name.len().min(32);
                arr[..length].copy_from_slice(&name.as_bytes()[..length]);
                arr
            },
            size: self.trait_.len() as u64,
            parent_koid: inner.parent.upgrade().map(|p| p.id()).unwrap_or(0),
            num_children: inner.children.len() as u64,
            flags: if self.resizable {
                VmoInfoFlags::RESIZABLE
            } else {
                VmoInfoFlags::empty()
            },
            cache_policy: self.trait_.cache_policy() as u32,
            share_count: inner.mapping_count as u64,
            ..Default::default()
        };
        self.trait_.complete_info(&mut ret);
        ret
    }

    /// Set the cache policy.
    pub fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult {
        let inner = self.inner.lock();
        if !inner.children.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        if inner.mapping_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        self.trait_.set_cache_policy(policy)
    }

    /// Append a mapping to the VMO's mapping list.
    pub fn append_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mapping_count += 1;
        self.trait_.append_mapping(mapping);
    }

    /// Remove a mapping from the VMO's mapping list.
    pub fn remove_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mapping_count -= 1;
        self.trait_.remove_mapping(mapping);
    }

    /// Returns an estimate of the number of unique VmAspaces that this object
    /// is mapped into.
    pub fn share_count(&self) -> usize {
        let inner = self.inner.lock();
        inner.mapping_count
    }

    /// Returns true if the object size can be changed.
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Returns true if the object is backed by a contiguous range of physical memory.
    pub fn is_contiguous(&self) -> bool {
        self.trait_.is_contiguous()
    }
}

impl Deref for VmObject {
    type Target = Arc<dyn VMObjectTrait>;

    fn deref(&self) -> &Self::Target {
        &self.trait_
    }
}

impl Drop for VmObject {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        let parent = match inner.parent.upgrade() {
            Some(parent) => parent,
            None => return,
        };
        for child in inner.children.iter() {
            if let Some(child) = child.upgrade() {
                child.inner.lock().parent = Arc::downgrade(&parent);
            }
        }
        let mut parent_inner = parent.inner.lock();
        let children = &mut parent_inner.children;
        children.append(&mut inner.children);
        children.retain(|c| c.strong_count() != 0);
        for child in children.iter() {
            let child = child.upgrade().unwrap();
            let mut inner = child.inner.lock();
            inner.children.retain(|c| c.strong_count() != 0);
            if inner.children.is_empty() {
                child.base.signal_set(Signal::VMO_ZERO_CHILDREN);
            }
        }
        // Non-zero to zero?
        if children.is_empty() {
            parent.base.signal_set(Signal::VMO_ZERO_CHILDREN);
        }
    }
}

/// Describes a VMO.
#[repr(C)]
#[derive(Default)]
pub struct VmoInfo {
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
    /// Values used by ZX_INFO_PROCESS_VMOS.
    pub struct VmoInfoFlags: u32 {
        /// The VMO points to a physical address range, and does not consume memory.
        /// Typically used to access memory-mapped hardware.
        /// Mutually exclusive with TYPE_PAGED.
        const TYPE_PHYSICAL = 0;

        #[allow(clippy::identity_op)]
        /// The VMO is backed by RAM, consuming memory.
        /// Mutually exclusive with TYPE_PHYSICAL.
        const TYPE_PAGED    = 1 << 0;

        /// The VMO is resizable.
        const RESIZABLE     = 1 << 1;

        /// The VMO is a child, and is a copy-on-write clone.
        const IS_COW_CLONE  = 1 << 2;

        /// When reading a list of VMOs pointed to by a process, indicates that the
        /// process has a handle to the VMO, which isn't necessarily mapped.
        const VIA_HANDLE    = 1 << 3;

        /// When reading a list of VMOs pointed to by a process, indicates that the
        /// process maps the VMO into a VMAR, but doesn't necessarily have a handle to
        /// the VMO.
        const VIA_MAPPING   = 1 << 4;

        /// The VMO is a pager owned VMO created by zx_pager_create_vmo or is
        /// a clone of a VMO with this flag set. Will only be set on VMOs with
        /// the ZX_INFO_VMO_TYPE_PAGED flag set.
        const PAGER_BACKED  = 1 << 5;

        /// The VMO is contiguous.
        const CONTIGUOUS    = 1 << 6;
    }
}

/// Different operations that `range_change` can perform against any VmMappings that are found.
#[allow(dead_code)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum RangeChangeOp {
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

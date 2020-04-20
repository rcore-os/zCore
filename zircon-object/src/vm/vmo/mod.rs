use {super::*, crate::object::*, alloc::sync::Arc, kernel_hal::PageTable};

mod paged;
mod physical;

use self::{paged::*, physical::*};
use core::ops::Deref;

/// Virtual Memory Objects
#[allow(clippy::len_without_is_empty)]
pub trait VMObjectTrait: Sync + Send {
    /// Read memory to `buf` from VMO at `offset`.
    fn read(&self, offset: usize, buf: &mut [u8]);

    /// Write memory from `buf` to VMO at `offset`.
    fn write(&self, offset: usize, buf: &[u8]);

    /// Get the length of VMO.
    fn len(&self) -> usize;

    /// Set the length of VMO.
    fn set_len(&self, len: usize);

    /// Map physical memory to `page_table`.
    fn map_to(
        &self,
        mapping: Arc<VmMapping>,
        vaddr: VirtAddr,
        offset: usize,
        len: usize,
        flags: MMUFlags,
    );

    /// Unmap physical memory from `page_table`.
    fn unmap_from(&self, page_table: &mut PageTable, vaddr: VirtAddr, _offset: usize, len: usize) {
        // TODO _offset unused?
        let pages = len / PAGE_SIZE;
        page_table
            .unmap_cont(vaddr, pages)
            .expect("failed to unmap")
    }

    /// Commit allocating physical memory.
    fn commit(&self, offset: usize, len: usize);

    /// Decommit allocated physical memory.
    fn decommit(&self, offset: usize, len: usize);

    /// Create a child vmo
    fn create_child(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait>;

    fn create_clone(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait>;

    fn append_mapping(&self, mapping: Arc<VmMapping>);
}

pub struct VmObject {
    base: KObjectBase,
    _counter: CountHelper,
    inner: Arc<dyn VMObjectTrait>,
}

impl_kobject!(VmObject);
define_count_helper!(VmObject);

impl VmObject {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new_paged(pages: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
            inner: VMObjectPaged::new(pages),
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
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
            inner: VMObjectPhysical::new(paddr, pages),
        })
    }

    pub fn create_clone(&self, offset: usize, len: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
            inner: self.inner.create_clone(offset, len),
        })
    }

    pub fn create_child(&self, offset: usize, len: usize) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
            inner: self.inner.create_child(offset, len),
        })
    }
}

impl Deref for VmObject {
    type Target = Arc<dyn VMObjectTrait>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn read_write(vmo: &VmObject) {
        let mut buf = [0u8; 4];
        vmo.write(0, &[0, 1, 2, 3]);
        vmo.read(0, &mut buf);
        assert_eq!(&buf, &[0, 1, 2, 3]);
    }
}

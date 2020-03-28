use {super::*, crate::object::*, alloc::sync::Arc, kernel_hal::PageTable};

mod paged;
mod physical;

pub use self::{paged::*, physical::*};
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
        page_table: &mut PageTable,
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
}

pub struct VmObject {
    base: KObjectBase,
    inner: Arc<dyn VMObjectTrait>,
}

impl VmObject {
    pub fn new(inner: Arc<dyn VMObjectTrait>) -> Arc<Self> {
        Arc::new(VmObject {
            base: KObjectBase::default(),
            inner,
        })
    }
}

impl Deref for VmObject {
    type Target = Arc<dyn VMObjectTrait>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl_kobject!(VmObject);

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

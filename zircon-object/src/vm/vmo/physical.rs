use {super::*, alloc::sync::Arc, spin::Mutex};

/// VMO representing a physical range of memory.
pub struct VMObjectPhysical {
    base: KObjectBase,
    paddr: PhysAddr,
    pages: usize,
    /// Lock this when access physical memory.
    data_lock: Mutex<()>,
}

impl_kobject!(VMObjectPhysical);

impl VMObjectPhysical {
    /// Create a new VMO representing a piece of contiguous physical memory.
    ///
    /// It's unsafe since you must ensure nobody has the ownership of
    /// this piece of memory yet.
    #[allow(unsafe_code)]
    pub unsafe fn new(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        assert!(page_aligned(paddr));
        Arc::new(VMObjectPhysical {
            base: KObjectBase::default(),
            paddr,
            pages,
            data_lock: Mutex::default(),
        })
    }
}

impl VMObject for VMObjectPhysical {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        let _ = self.data_lock.lock();
        assert!(offset + buf.len() <= self.len());
        hal::pmem_read(self.paddr + offset, buf);
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        let _ = self.data_lock.lock();
        assert!(offset + buf.len() <= self.len());
        hal::pmem_write(self.paddr + offset, buf);
    }

    fn len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    fn set_len(&self) {
        unimplemented!()
    }

    fn map_to(&self, page_table: &mut PageTable, vaddr: usize, offset: usize, len: usize) {
        let flags = 0; // FIXME
        let pages = len / PAGE_SIZE;
        page_table
            .map_cont(vaddr, self.paddr + offset, pages, flags)
            .expect("failed to map")
    }

    // TODO empty function should be denied
    fn commit(&self, _offset: usize, _len: usize) {}
}

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)]
    use super::*;

    #[test]
    fn read_write() {
        let vmo = unsafe { VMObjectPhysical::new(0x1000, 2) };
        super::super::tests::read_write(&*vmo);
    }
}

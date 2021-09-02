use {super::*, alloc::sync::Arc, spin::Mutex};

/// VMO representing a physical range of memory.
pub struct VMObjectPhysical {
    paddr: PhysAddr,
    pages: usize,
    /// Lock this when access physical memory.
    data_lock: Mutex<()>,
    inner: Mutex<VMObjectPhysicalInner>,
}

struct VMObjectPhysicalInner {
    cache_policy: CachePolicy,
}

impl VMObjectPhysicalInner {
    pub fn new() -> VMObjectPhysicalInner {
        VMObjectPhysicalInner {
            cache_policy: CachePolicy::Uncached,
        }
    }
}

impl VMObjectPhysical {
    /// Create a new VMO representing a piece of contiguous physical memory.
    /// You must ensure nobody has the ownership of this piece of memory yet.
    pub fn new(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        assert!(page_aligned(paddr));
        Arc::new(VMObjectPhysical {
            paddr,
            pages,
            data_lock: Mutex::default(),
            inner: Mutex::new(VMObjectPhysicalInner::new()),
        })
    }
}

impl VMObjectTrait for VMObjectPhysical {
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult {
        let _ = self.data_lock.lock();
        assert!(offset + buf.len() <= self.len());
        kernel_hal::mem::pmem_read(self.paddr + offset, buf);
        Ok(())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        let _ = self.data_lock.lock();
        assert!(offset + buf.len() <= self.len());
        kernel_hal::mem::pmem_write(self.paddr + offset, buf);
        Ok(())
    }

    fn zero(&self, offset: usize, len: usize) -> ZxResult {
        let _ = self.data_lock.lock();
        assert!(offset + len <= self.len());
        kernel_hal::mem::pmem_zero(self.paddr + offset, len);
        Ok(())
    }

    fn len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    fn set_len(&self, _len: usize) -> ZxResult {
        unimplemented!()
    }

    fn commit_page(&self, page_idx: usize, _flags: MMUFlags) -> ZxResult<PhysAddr> {
        Ok(self.paddr + page_idx * PAGE_SIZE)
    }

    fn commit_pages_with(
        &self,
        f: &mut dyn FnMut(&mut dyn FnMut(usize, MMUFlags) -> ZxResult<PhysAddr>) -> ZxResult,
    ) -> ZxResult {
        f(&mut |page_idx, _flags| Ok(self.paddr + page_idx * PAGE_SIZE))
    }

    fn commit(&self, _offset: usize, _len: usize) -> ZxResult {
        // do nothing
        Ok(())
    }

    fn decommit(&self, _offset: usize, _len: usize) -> ZxResult {
        // do nothing
        Ok(())
    }

    fn create_child(&self, _offset: usize, _len: usize) -> ZxResult<Arc<dyn VMObjectTrait>> {
        Err(ZxError::NOT_SUPPORTED)
    }

    fn complete_info(&self, _info: &mut VmoInfo) {
        warn!("VmoInfo for physical is unimplemented");
    }

    fn cache_policy(&self) -> CachePolicy {
        let inner = self.inner.lock();
        inner.cache_policy
    }

    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult {
        let mut inner = self.inner.lock();
        inner.cache_policy = policy;
        Ok(())
    }

    fn committed_pages_in_range(&self, _start_idx: usize, _end_idx: usize) -> usize {
        0
    }

    fn is_contiguous(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)]
    use super::*;
    use kernel_hal::CachePolicy;

    #[test]
    fn read_write() {
        let vmo = VmObject::new_physical(0x1000, 2);
        assert_eq!(vmo.cache_policy(), CachePolicy::Uncached);
        super::super::tests::read_write(&vmo);
    }
}

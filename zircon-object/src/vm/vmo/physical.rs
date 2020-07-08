use {
    super::*,
    alloc::sync::{Arc, Weak},
    spin::Mutex,
};

/// VMO representing a physical range of memory.
pub struct VMObjectPhysical {
    paddr: PhysAddr,
    pages: usize,
    /// Lock this when access physical memory.
    data_lock: Mutex<()>,
    inner: Mutex<VMObjectPhysicalInner>,
}

struct VMObjectPhysicalInner {
    mapping_count: u32,
    cache_policy: CachePolicy,
    content_size: usize,
}

impl VMObjectPhysicalInner {
    pub fn new() -> VMObjectPhysicalInner {
        VMObjectPhysicalInner {
            mapping_count: 0,
            cache_policy: CachePolicy::Uncached,
            content_size: 0,
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
        kernel_hal::pmem_read(self.paddr + offset, buf);
        Ok(())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        let _ = self.data_lock.lock();
        assert!(offset + buf.len() <= self.len());
        kernel_hal::pmem_write(self.paddr + offset, buf);
        Ok(())
    }

    fn len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    fn set_len(&self, _len: usize) -> ZxResult {
        unimplemented!()
    }

    fn content_size(&self) -> usize {
        let inner = self.inner.lock();
        inner.content_size
    }

    fn set_content_size(&self, size: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        inner.content_size = size;
        Ok(())
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

    fn create_child(
        &self,
        _offset: usize,
        _len: usize,
        _user_id: KoID,
    ) -> ZxResult<Arc<dyn VMObjectTrait>> {
        Err(ZxError::NOT_SUPPORTED)
    }

    fn append_mapping(&self, _mapping: Weak<VmMapping>) {
        // TODO this function is only used when physical-vmo supports create_child
        let mut inner = self.inner.lock();
        inner.mapping_count += 1;
    }

    fn remove_mapping(&self, _mapping: Weak<VmMapping>) {
        let mut inner = self.inner.lock();
        inner.mapping_count -= 1;
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
        if inner.cache_policy == policy {
            Ok(())
        } else {
            // if (mapping_list_len_ != 0 || children_list_len_ != 0 || parent_)
            if inner.mapping_count != 0 {
                return Err(ZxError::BAD_STATE);
            }
            inner.cache_policy = policy;
            Ok(())
        }
    }

    fn share_count(&self) -> usize {
        self.inner.lock().mapping_count as usize
    }

    fn committed_pages_in_range(&self, _start_idx: usize, _end_idx: usize) -> usize {
        0
    }

    fn zero(&self, _offset: usize, _len: usize) -> ZxResult {
        unimplemented!()
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
        let vmphy = vmo.inner.clone();
        assert_eq!(vmphy.cache_policy(), CachePolicy::Uncached);
        super::super::tests::read_write(&vmo);
    }
}

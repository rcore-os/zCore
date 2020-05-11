use super::*;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct VMObjectSlice {
    /// Parent node.
    parent: Arc<dyn VMObjectTrait>,
    /// The offset from parent.
    offset: usize,
    /// The size in bytes.
    size: usize,
    /// Mapping count.
    mapping_count: AtomicUsize,
}

impl VMObjectSlice {
    pub fn new(parent: Arc<dyn VMObjectTrait>, offset: usize, size: usize) -> Arc<Self> {
        Arc::new(VMObjectSlice {
            parent,
            offset,
            size,
            mapping_count: AtomicUsize::new(0),
        })
    }

    fn check_range(&self, offset: usize, len: usize) -> ZxResult {
        if offset + len >= self.size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        Ok(())
    }
}

impl VMObjectTrait for VMObjectSlice {
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult {
        self.check_range(offset, buf.len())?;
        self.parent.read(offset + self.offset, buf)
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        self.check_range(offset, buf.len())?;
        self.parent.write(offset + self.offset, buf)
    }

    fn len(&self) -> usize {
        self.size
    }

    fn set_len(&self, _len: usize) -> ZxResult {
        unimplemented!()
    }

    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<usize> {
        self.parent
            .commit_page(page_idx + self.offset / PAGE_SIZE, flags)
    }

    fn commit(&self, offset: usize, len: usize) -> ZxResult {
        self.parent.commit(offset + self.offset, len)
    }

    fn decommit(&self, offset: usize, len: usize) -> ZxResult {
        self.parent.decommit(offset + self.offset, len)
    }

    fn create_child(
        &self,
        _offset: usize,
        _len: usize,
        _user_id: u64,
    ) -> ZxResult<Arc<dyn VMObjectTrait>> {
        Err(ZxError::NOT_SUPPORTED)
    }

    fn append_mapping(&self, _mapping: Weak<VmMapping>) {
        self.mapping_count.fetch_add(1, Ordering::SeqCst);
    }

    fn remove_mapping(&self, _mapping: Weak<VmMapping>) {
        self.mapping_count.fetch_sub(1, Ordering::SeqCst);
    }

    fn complete_info(&self, info: &mut VmoInfo) {
        self.parent.complete_info(info);
    }

    fn cache_policy(&self) -> CachePolicy {
        self.parent.cache_policy()
    }

    fn set_cache_policy(&self, _policy: CachePolicy) -> ZxResult {
        Ok(())
    }

    fn share_count(&self) -> usize {
        self.mapping_count.load(Ordering::SeqCst)
    }

    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize {
        let po = pages(self.offset);
        self.parent
            .committed_pages_in_range(start_idx + po, end_idx + po)
    }

    fn pin(&self, offset: usize, len: usize) -> ZxResult {
        self.check_range(offset, len)?;
        self.parent.pin(offset + self.offset, len)
    }

    fn unpin(&self, offset: usize, len: usize) -> ZxResult {
        self.check_range(offset, len)?;
        self.parent.unpin(offset + self.offset, len)
    }

    fn is_contiguous(&self) -> bool {
        self.parent.is_contiguous()
    }

    fn is_paged(&self) -> bool {
        self.parent.is_paged()
    }

    fn zero(&self, offset: usize, len: usize) -> ZxResult {
        self.check_range(offset, len)?;
        self.parent.zero(offset + self.offset, len)
    }
}

use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::sync::Arc,
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::{PhysFrame, PAGE_SIZE},
    spin::Mutex,
};

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Mutex<VMObjectPagedInner>,
}

/// The mutable part of `VMObjectPaged`.
#[derive(Default)]
struct VMObjectPagedInner {
    /// Physical frames of this VMO.
    frames: Vec<PhysFrame>,
    /// Cache Policy
    cache_policy: CachePolicy,
    /// Is contiguous
    contiguous: bool,
    /// Sum of pin_count
    pin_count: usize,
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = Vec::new();
        frames.resize_with(pages, || PhysFrame::alloc_zeroed().unwrap());
        Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                frames,
                ..Default::default()
            }),
        })
    }

    /// Create a list of contiguous pages
    pub fn new_contiguous(pages: usize, align_log2: usize) -> ZxResult<Arc<Self>> {
        let frames = PhysFrame::alloc_contiguous_zeroed(pages, align_log2 - PAGE_SIZE_LOG2);
        if frames.is_empty() {
            return Err(ZxError::NO_MEMORY);
        }
        Ok(Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                frames,
                contiguous: true,
                ..Default::default()
            }),
        }))
    }
}

impl VMObjectTrait for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.cache_policy != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        inner.for_each_page(offset, buf.len(), |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        });
        Ok(())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.cache_policy != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        inner.for_each_page(offset, buf.len(), |paddr, buf_range| {
            kernel_hal::pmem_write(paddr, &buf[buf_range]);
        });
        Ok(())
    }

    fn zero(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.cache_policy != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        inner.for_each_page(offset, len, |paddr, buf_range| {
            kernel_hal::pmem_zero(paddr, buf_range.len());
        });
        Ok(())
    }

    fn len(&self) -> usize {
        let inner = self.inner.lock();
        inner.frames.len() * PAGE_SIZE
    }

    fn set_len(&self, len: usize) -> ZxResult {
        assert!(page_aligned(len));
        let mut inner = self.inner.lock();
        inner.frames.resize_with(len / PAGE_SIZE, || {
            PhysFrame::alloc().ok_or(ZxError::NO_MEMORY).unwrap()
        });
        Ok(())
    }

    fn commit_page(&self, page_idx: usize, _flags: MMUFlags) -> ZxResult<PhysAddr> {
        let inner = self.inner.lock();
        Ok(inner.frames[page_idx].addr())
    }

    fn commit_pages_with(
        &self,
        f: &mut dyn FnMut(&mut dyn FnMut(usize, MMUFlags) -> ZxResult<PhysAddr>) -> ZxResult,
    ) -> ZxResult {
        let inner = self.inner.lock();
        f(&mut |page_idx, _| Ok(inner.frames[page_idx].addr()))
    }

    fn commit(&self, _offset: usize, _len: usize) -> ZxResult {
        Ok(())
    }

    fn decommit(&self, _offset: usize, _len: usize) -> ZxResult {
        Ok(())
    }

    fn create_child(&self, offset: usize, len: usize) -> ZxResult<Arc<dyn VMObjectTrait>> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let mut inner = self.inner.lock();
        let child = inner.create_child(offset, len)?;
        Ok(child)
    }

    fn complete_info(&self, info: &mut VmoInfo) {
        let inner = self.inner.lock();
        info.flags |= VmoInfoFlags::TYPE_PAGED;
        inner.complete_info(info);
    }

    fn cache_policy(&self) -> CachePolicy {
        let inner = self.inner.lock();
        inner.cache_policy
    }

    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult {
        // conditions for allowing the cache policy to be set:
        // 1) vmo either has no pages committed currently or is transitioning from being cached
        // 2) vmo has no pinned pages
        // 3) vmo has no mappings
        // 4) vmo has no children (TODO)
        // 5) vmo is not a child
        let mut inner = self.inner.lock();
        if !inner.frames.is_empty() && inner.cache_policy != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        if inner.pin_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        if inner.cache_policy == CachePolicy::Cached && policy != CachePolicy::Cached {
            for frame in inner.frames.iter() {
                kernel_hal::frame_flush(frame.addr());
            }
        }
        inner.cache_policy = policy;
        Ok(())
    }

    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize {
        end_idx - start_idx
    }

    fn pin(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        if offset + len > inner.frames.len() * PAGE_SIZE {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if len == 0 {
            return Ok(());
        }
        inner.pin_count += pages(len);
        Ok(())
    }

    fn unpin(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        if offset + len > inner.frames.len() * PAGE_SIZE {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if len == 0 {
            return Ok(());
        }
        inner.pin_count -= pages(len);
        Ok(())
    }

    fn is_contiguous(&self) -> bool {
        let inner = self.inner.lock();
        inner.contiguous
    }

    fn is_paged(&self) -> bool {
        true
    }
}

impl VMObjectPagedInner {
    /// Helper function to split range into sub-ranges within pages.
    ///
    /// ```text
    /// VMO range:
    /// |----|----|----|----|----|
    ///
    /// buf:
    ///            [====len====]
    /// |--offset--|
    ///
    /// sub-ranges:
    ///            [===]
    ///                [====]
    ///                     [==]
    /// ```
    ///
    /// `f` is a function to process in-page ranges.
    /// It takes 2 arguments:
    /// * `paddr`: the start physical address of the in-page range.
    /// * `buf_range`: the range in view of the input buffer.
    fn for_each_page(
        &mut self,
        offset: usize,
        buf_len: usize,
        mut f: impl FnMut(PhysAddr, Range<usize>),
    ) {
        let iter = BlockIter {
            begin: offset,
            end: offset + buf_len,
            block_size_log2: 12,
        };
        for block in iter {
            let paddr = self.frames[block.block].addr();
            let buf_range = block.origin_begin() - offset..block.origin_end() - offset;
            f(paddr + block.begin, buf_range);
        }
    }

    /// Create a snapshot child VMO.
    fn create_child(&mut self, offset: usize, len: usize) -> ZxResult<Arc<VMObjectPaged>> {
        // clone contiguous vmo is no longer permitted
        // https://fuchsia.googlesource.com/fuchsia/+/e6b4c6751bbdc9ed2795e81b8211ea294f139a45
        if self.contiguous {
            return Err(ZxError::INVALID_ARGS);
        }
        if self.cache_policy != CachePolicy::Cached || self.pin_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        let mut frames = Vec::with_capacity(pages(len));
        for _ in 0..pages(len) {
            frames.push(PhysFrame::alloc().ok_or(ZxError::NO_MEMORY)?);
        }
        for (i, frame) in frames.iter().enumerate() {
            if let Some(src_frame) = self.frames.get(pages(offset) + i) {
                kernel_hal::frame_copy(src_frame.addr(), frame.addr())
            } else {
                kernel_hal::pmem_zero(frame.addr(), PAGE_SIZE);
            }
        }
        // create child VMO
        let child = Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                frames,
                ..Default::default()
            }),
        });
        Ok(child)
    }

    fn complete_info(&self, info: &mut VmoInfo) {
        if self.contiguous {
            info.flags |= VmoInfoFlags::CONTIGUOUS;
        }
        // info.num_children = if self.type_.is_hidden() { 2 } else { 0 };
        info.committed_bytes = (self.frames.len() * PAGE_SIZE) as u64;
    }
}

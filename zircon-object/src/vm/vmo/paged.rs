use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::sync::Arc,
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::{PageTable, PhysFrame},
    spin::Mutex,
};

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    base: KObjectBase,
    inner: Mutex<VMObjectPagedInner>,
}

/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    frames: Vec<Option<PhysFrame>>,
}

impl_kobject!(VMObjectPaged);

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = Vec::new();
        frames.resize_with(pages, Default::default);

        Arc::new(VMObjectPaged {
            base: {
                let mut res = KObjectBase::default();
                res.obj_type = OBJ_TYPE_VMO;
                res
            },
            inner: Mutex::new(VMObjectPagedInner { frames }),
        })
    }
}

impl VMObject for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        self.inner
            .lock()
            .for_each_page(offset, buf.len(), |paddr, buf_range| {
                kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
            });
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        self.inner
            .lock()
            .for_each_page(offset, buf.len(), |paddr, buf_range| {
                kernel_hal::pmem_write(paddr, &buf[buf_range]);
            });
    }

    fn len(&self) -> usize {
        self.inner.lock().frames.len() * PAGE_SIZE
    }

    fn set_len(&self, len: usize) {
        // FIXME parent and children? len < old_len?
        let old_len = self.inner.lock().frames.len();
        warn!("old_len: {:#x}, len: {:#x}", old_len, len);
        if old_len < len {
            self.inner.lock().frames.resize_with(len, Default::default);
            self.commit(old_len, len - old_len);
        } else {
            unimplemented!()
        }
    }

    fn map_to(
        &self,
        page_table: &mut PageTable,
        vaddr: usize,
        offset: usize,
        len: usize,
        flags: MMUFlags,
    ) {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        for i in 0..pages {
            let frame = inner.commit(start_page + i);
            page_table
                .map(vaddr + i * PAGE_SIZE, frame.addr(), flags)
                .expect("failed to map");
        }
    }

    fn commit(&self, offset: usize, len: usize) {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        for i in 0..pages {
            inner.commit(start_page + i);
        }
    }
}

impl VMObjectPagedInner {
    /// Helper function to split range into sub-ranges within pages.
    ///
    /// All covered pages will be committed implicitly.
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
            self.commit(block.block);
            let paddr = self.frames[block.block].as_ref().unwrap().addr();
            let buf_range = block.origin_begin() - offset..block.origin_end() - offset;
            f(paddr + block.begin, buf_range);
        }
    }

    fn commit(&mut self, page_idx: usize) -> &PhysFrame {
        self.frames[page_idx]
            .get_or_insert_with(|| PhysFrame::alloc().expect("failed to alloc frame"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write() {
        let vmo = VMObjectPaged::new(2);
        super::super::tests::read_write(&*vmo);
    }
}

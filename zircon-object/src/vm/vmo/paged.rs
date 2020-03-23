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
    inner: Mutex<VMObjectPagedInner>,
}

/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    parent: Option<Arc<VMObjectPaged>>,
    parent_offset: usize,
    frames: Vec<Option<PhysFrame>>,
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = Vec::new();
        frames.resize_with(pages, Default::default);

        Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                parent: None,
                parent_offset: 0usize,
                frames,
            }),
        })
    }

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
        &self,
        offset: usize,
        buf_len: usize,
        for_write: bool,
        mut f: impl FnMut(PhysAddr, Range<usize>),
    ) {
        let iter = BlockIter {
            begin: offset,
            end: offset + buf_len,
            block_size_log2: 12,
        };
        for block in iter {
            let paddr = self.inner.lock().get_page(block.block, for_write);
            let buf_range = block.origin_begin() - offset..block.origin_end() - offset;
            f(paddr + block.begin, buf_range);
        }
    }

    fn get_page(&self, page_idx: usize, for_write: bool) -> PhysAddr {
        self.inner.lock().get_page(page_idx, for_write)
    }
}

impl VMObjectTrait for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        self.for_each_page(offset, buf.len(), false, |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        });
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        self.for_each_page(offset, buf.len(), true, |paddr, buf_range| {
            kernel_hal::pmem_write(paddr, &buf[buf_range]);
        });
    }

    fn len(&self) -> usize {
        self.inner.lock().frames.len() * PAGE_SIZE
    }

    fn set_len(&self, len: usize) {
        assert!(page_aligned(len));
        // FIXME parent and children? len < old_len?
        let mut inner = self.inner.lock();
        let old_pages = inner.frames.len();
        let new_pages = len / PAGE_SIZE;
        warn!("old_pages: {:#x}, new_pages: {:#x}", old_pages, new_pages);
        if old_pages < new_pages {
            inner.frames.resize_with(len, Default::default);
            (old_pages..new_pages).for_each(|idx| {
                inner.commit(idx);
            });
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
            let paddr = inner.get_page(start_page + i, true);
            page_table
                .map(vaddr + i * PAGE_SIZE, paddr, flags)
                .expect("failed to map");
        }
    }

    fn commit(&self, offset: usize, len: usize) {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        warn!("start_page: {:#x}, pages: {:#x}", start_page, pages);
        for i in 0..pages {
            inner.commit(start_page + i);
        }
    }

    fn decommit(&self, offset: usize, len: usize) {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        for i in 0..pages {
            inner.decommit(start_page + i);
        }
    }

    fn create_child(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let mut frames = Vec::new();
        let pages = self.len() / PAGE_SIZE;
        let mut inner = self.inner.lock();
        frames.append(&mut inner.frames);
        let old_parent = inner.parent.take();

        // construct hidden_vmo as shared parent
        let hidden_vmo = Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                parent: old_parent,
                parent_offset: 0usize,
                frames,
            }),
        });

        // change current vmo's parent
        inner.parent = Some(hidden_vmo.clone());
        inner.frames.resize_with(pages, Default::default);

        // create hidden_vmo's another child as result
        let mut child_frames = Vec::new();
        child_frames.resize_with(len / PAGE_SIZE, Default::default);
        Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                parent: Some(hidden_vmo),
                parent_offset: offset,
                frames: child_frames,
            }),
        })
    }

    fn create_clone(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let frames_offset = pages(offset);
        let clone_size = pages(len);
        let mut frames = Vec::new();
        frames.resize_with(clone_size, || {
            Some(PhysFrame::alloc().expect("faild to alloc frame"))
        });
        let inner = self.inner.lock();
        for i in 0..clone_size {
            if inner.frames[frames_offset + i].is_some() {
                kernel_hal::frame_copy(
                    inner.frames[frames_offset + i].as_ref().unwrap().addr(),
                    frames[i].as_ref().unwrap().addr(),
                );
            }
        }
        Arc::new(VMObjectPaged {
            inner: Mutex::new(VMObjectPagedInner {
                parent: None,
                parent_offset: offset,
                frames,
            }),
        })
    }
}

impl VMObjectPagedInner {
    fn commit(&mut self, page_idx: usize) -> &PhysFrame {
        self.frames[page_idx]
            .get_or_insert_with(|| PhysFrame::alloc().expect("failed to alloc frame"))
    }

    fn decommit(&mut self, page_idx: usize) {
        self.frames[page_idx] = None;
    }

    fn get_page(&mut self, page_idx: usize, for_write: bool) -> PhysAddr {
        if let Some(frame) = &self.frames[page_idx] {
            return frame.addr();
        }
        let parent_idx_offset = self.parent_offset / PAGE_SIZE;
        if for_write {
            let target_addr = self.commit(page_idx).addr();
            if let Some(parent) = &self.parent {
                // copy on write
                kernel_hal::frame_copy(
                    parent.get_page(parent_idx_offset + page_idx, false),
                    target_addr,
                );
            } else {
                // zero the page
                kernel_hal::pmem_write(target_addr, &[0u8; PAGE_SIZE]);
            }
            target_addr
        } else if let Some(parent) = &self.parent {
            parent.get_page(parent_idx_offset + page_idx, false)
        } else {
            self.commit(page_idx).addr()
        }
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

    #[test]
    fn create_child() {
        let vmo = VMObjectPaged::new(10);
        vmo.write(0, &[1, 2, 3, 4]);
        let mut buf = [0u8; 4];
        vmo.read(0, &mut buf);
        assert_eq!(&buf, &[1, 2, 3, 4]);
        let child_vmo = vmo.create_child(0, 4 * 4096);
        child_vmo.read(0, &mut buf);
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.write(0, &[6, 7, 8, 9]);
        vmo.read(0, &mut buf);
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.read(0, &mut buf);
        assert_eq!(&buf, &[6, 7, 8, 9]);
    }
}

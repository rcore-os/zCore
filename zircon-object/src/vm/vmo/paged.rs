use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::sync::Arc,
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::PhysFrame,
    spin::Mutex,
};

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Arc<Mutex<VMObjectPagedInner>>,
}

#[allow(dead_code)]
/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    parent: Option<Arc<Mutex<VMObjectPagedInner>>>,
    parent_offset: usize,
    frames: Vec<Option<PhysFrame>>,
    mappings: Vec<Arc<VmMapping>>,
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = Vec::new();
        frames.resize_with(pages, Default::default);

        Arc::new(VMObjectPaged {
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                parent: None,
                parent_offset: 0usize,
                frames,
                mappings: Vec::new(),
            })),
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
        flags: MMUFlags,
        mut f: impl FnMut(PhysAddr, Range<usize>),
    ) {
        let iter = BlockIter {
            begin: offset,
            end: offset + buf_len,
            block_size_log2: 12,
        };
        for block in iter {
            let paddr = self.inner.lock().get_page(block.block, flags);
            let buf_range = block.origin_begin() - offset..block.origin_end() - offset;
            f(paddr + block.begin, buf_range);
        }
    }
}

impl VMObjectTrait for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        self.for_each_page(offset, buf.len(), MMUFlags::READ, |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        });
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        self.for_each_page(offset, buf.len(), MMUFlags::WRITE, |paddr, buf_range| {
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
        if old_pages < new_pages {
            inner.frames.resize_with(new_pages, Default::default);
            (old_pages..new_pages).for_each(|idx| {
                inner.commit(idx);
            });
        } else if inner.parent.is_none() {
            inner.frames.resize_with(new_pages, Default::default);
            (old_pages..new_pages).for_each(|idx| {
                inner.get_page(idx, MMUFlags::WRITE);
            });
        } else {
            unimplemented!()
        }
    }

    fn get_page(&self, page_idx: usize, flags: MMUFlags) -> PhysAddr {
        self.inner.lock().get_page(page_idx, flags)
    }

    fn commit(&self, offset: usize, len: usize) {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
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
        let page_num = self.len() / PAGE_SIZE;
        let mut inner = self.inner.lock();
        frames.append(&mut inner.frames);
        let old_parent = inner.parent.take();

        // construct hidden_vmo as shared parent
        let hidden_vmo = Arc::new(VMObjectPaged {
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                parent: old_parent,
                parent_offset: inner.parent_offset,
                frames,
                mappings: Vec::new(),
            })),
        });

        // change current vmo's parent
        inner.parent = Some(hidden_vmo.inner.clone());
        inner.parent_offset = 0usize;
        inner.frames.resize_with(page_num, Default::default);

        inner.mappings.iter().for_each(|map| map.remove_write_flag(pages(offset), pages(len)));

        // create hidden_vmo's another child as result
        let mut child_frames = Vec::new();
        child_frames.resize_with(len / PAGE_SIZE, Default::default);
        Arc::new(VMObjectPaged {
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                parent: Some(hidden_vmo.inner.clone()),
                parent_offset: offset,
                frames: child_frames,
                mappings: Vec::new(),
            })),
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
        // copy physical memory
        for (i, new_frame) in frames.iter().enumerate() {
            if let Some(frame) = &inner.frames[frames_offset + i] {
                kernel_hal::frame_copy(frame.addr(), new_frame.as_ref().unwrap().addr());
            }
        }
        Arc::new(VMObjectPaged {
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                parent: None,
                parent_offset: offset,
                frames,
                mappings: Vec::new(),
            })),
        })
    }

    fn append_mapping(&self, mapping: Arc<VmMapping>) {
        self.inner.lock().mappings.push(mapping);
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

    fn get_page(&mut self, page_idx: usize, flags: MMUFlags) -> PhysAddr {
        // check if it is in current frames list
        let mut res: PhysAddr = 0;
        if let Some(frame) = &self.frames[page_idx] {
            res = frame.addr();
        } else if self.parent.is_none() {
            // reach top of the tree
            let target_frame = PhysFrame::alloc().unwrap();
            res = target_frame.addr();
            kernel_hal::pmem_write(target_frame.addr(), &[0u8; PAGE_SIZE]);
            self.frames[page_idx] = Some(target_frame);
        } else {
            let mut current = self.parent.as_ref().cloned();
            let mut current_idx = page_idx + self.parent_offset / PAGE_SIZE;
            while let Some(locked_) = current {
                let locked_cur = locked_.lock();
                if let Some(frame) = &locked_cur.frames[current_idx] { // find it !
                    if !flags.contains(MMUFlags::WRITE) { // read-only
                        res = frame.addr();
                    } else {
                        let target_frame = PhysFrame::alloc().unwrap();
                        res = target_frame.addr();
                        kernel_hal::frame_copy(frame.addr(), target_frame.addr());
                        self.frames[page_idx] = Some(target_frame);
                    }
                    break;
                }
                current_idx += locked_cur.parent_offset / PAGE_SIZE;
                current = locked_cur.parent.as_ref().cloned();
            }
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write() {
        let vmo = VmObject::new_paged(2);
        super::super::tests::read_write(&*vmo);
    }

    #[test]
    fn create_child() {
        let vmo = VmObject::new_paged(10);
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

use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::PhysFrame,
    spin::Mutex,
    alloc::collections::BTreeMap,
};

#[derive(PartialEq, Eq, Debug)]
enum VMOType {
    Hidden,
    Snapshot,
    Origin,
}

#[allow(dead_code)]
#[derive(PartialEq, Eq, Debug)]
enum PageOrMarkerState {
    Init,
    RightSplit,
    LeftSplit,
}

struct PageOrMarker {
    pub inner: Option<PhysFrame>,
    state: PageOrMarkerState
}

#[allow(dead_code)]
impl PageOrMarker {
    /// This page is provided by current vmo, but its not concrete
    fn is_marker(&self) -> bool {
        self.inner.is_none()
    }

    /// This page is provided by current vmo, a concrete page
    fn is_page(&self) -> bool {
        self.inner.is_some()
    }

    /// This cow page has been forked, now we can see it as commited
    fn is_splited(&self) -> bool {
        self.state != PageOrMarkerState::Init
    }
}

impl Default for PageOrMarker{
    fn default() -> Self {
        PageOrMarker {
            inner: None,
            state: PageOrMarkerState::Init
        }
    }
}

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    global_mtx: Arc<Mutex<()>>,
    inner: Arc<Mutex<VMObjectPagedInner>>,
}

#[allow(dead_code)]
/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    _type: VMOType,
    parent: Option<Arc<Mutex<VMObjectPagedInner>>>,
    children: Vec<Weak<Mutex<VMObjectPagedInner>>>,
    parent_offset: usize,
    parent_limit: usize,
    size: usize,
    frames: BTreeMap<usize, PageOrMarker>,
    mappings: Vec<Arc<VmMapping>>,
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = BTreeMap::new();
        for i in 0..pages {
            frames.insert(i, PageOrMarker::default());
        }

        Arc::new(VMObjectPaged {
            global_mtx: Arc::new(Mutex::new(())),
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                _type: VMOType::Origin,
                parent: None,
                children: Vec::new(),
                parent_offset: 0usize,
                parent_limit: 0usize,
                size: pages * PAGE_SIZE,
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
        let guard = self.global_mtx.lock();
        self.for_each_page(offset, buf.len(), MMUFlags::READ, |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        });
        drop(guard);
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        let guard = self.global_mtx.lock();
        self.for_each_page(offset, buf.len(), MMUFlags::WRITE, |paddr, buf_range| {
            kernel_hal::pmem_write(paddr, &buf[buf_range]);
        });
        drop(guard);
    }

    fn len(&self) -> usize {
        let guard = self.global_mtx.lock();
        let ret = self.inner.lock().size;
        drop(guard);
        ret
    }

    fn set_len(&self, len: usize) {
        assert!(page_aligned(len));
        let guard = self.global_mtx.lock();
        // FIXME parent and children? len < old_len?
        let mut inner = self.inner.lock();
        inner.size = len;
        drop(guard);
    }

    fn get_page(&self, page_idx: usize, flags: MMUFlags) -> PhysAddr {
        let guard = self.global_mtx.lock();
        let ret = self.inner.lock().get_page(page_idx, flags);
        drop(guard);
        ret
    }

    fn commit(&self, offset: usize, len: usize) {
        let guard = self.global_mtx.lock();
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        for i in 0..pages {
            inner.commit(start_page + i);
        }
        drop(guard);
    }

    fn decommit(&self, offset: usize, len: usize) -> ZxResult {
        let guard = self.global_mtx.lock();
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        if !inner.children.is_empty() || inner.parent.is_some() {
            drop(guard);
            Err(ZxError::NOT_SUPPORTED)
        } else {
            for i in 0..pages {
                inner.decommit(start_page + i);
            }
            drop(guard);
            Ok(())
        }
    }

    fn create_child(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        let guard = self.global_mtx.lock();
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let child_inner = self.inner.lock().create_child(&self.inner, offset, len);
        let ret = Arc::new(VMObjectPaged {
            global_mtx: self.global_mtx.clone(),
            inner: child_inner,
        });
        drop(guard);
        ret
    }

    fn create_clone(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        let guard = self.global_mtx.lock();
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let mut frames = BTreeMap::new();
        let inner = self.inner.lock();
        // copy physical memory
        for (i, frame) in inner.frames.iter() {
            let value = if frame.is_page() {
                let new_frame = PhysFrame::alloc().expect("failed to alloc frame");
                kernel_hal::frame_copy(frame.inner.as_ref().unwrap().addr(), new_frame.addr());
                PageOrMarker{
                    inner: Some(new_frame),
                    state: PageOrMarkerState::Init,
                }
            } else {
                PageOrMarker::default()
            };
            frames.insert(i.clone(), value);
        }
        let ret = Arc::new(VMObjectPaged {
            global_mtx: self.global_mtx.clone(),
            inner: Arc::new(Mutex::new(VMObjectPagedInner {
                _type: VMOType::Snapshot,
                parent: None,
                children: Vec::new(),
                parent_offset: offset,
                parent_limit: offset + len,
                size: len,
                frames,
                mappings: Vec::new(),
            })),
        });
        drop(guard);
        ret
    }

    fn append_mapping(&self, mapping: Arc<VmMapping>) {
        let guard = self.global_mtx.lock();
        self.inner.lock().mappings.push(mapping);
        drop(guard);
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        info.flags |= VmoInfoFlags::TYPE_PAGED.bits();
        self.inner.lock().complete_info(info);
    }
}

impl VMObjectPagedInner {
    fn commit(&mut self, page_idx: usize) -> &PhysFrame {
        if let Some(value) = self.frames.get_mut(&page_idx) {
            value.inner.get_or_insert_with(|| PhysFrame::alloc().expect("failed to alloc frame"))
        } else {
            unimplemented!()
        }
    }

    fn decommit(&mut self, page_idx: usize) {
        if let Some(value) = self.frames.get_mut(&page_idx) {
            value.inner = None;
        }
    }

    fn get_page(&mut self, page_idx: usize, flags: MMUFlags) -> PhysAddr {
        // check if it is in current frames list
        let mut res: PhysAddr = 0;
        if let Some(_frame) = self.frames.get(&page_idx) {
            if let Some(frame) = &_frame.inner {
                return frame.addr();
            }
        }
        let mut current = self.parent.as_ref().cloned();
        let mut current_idx = page_idx + self.parent_offset / PAGE_SIZE;
        while let Some(locked_) = current {
            let mut locked_cur = locked_.lock();
            if let Some(_frame) = locked_cur.frames.get_mut(&current_idx) {
                if let Some(frame) = &_frame.inner {
                    if !flags.contains(MMUFlags::WRITE) { // read-only
                        res = frame.addr();
                    } else {
                        _frame.state = PageOrMarkerState::LeftSplit;
                        let target_frame = PhysFrame::alloc().unwrap();
                        res = target_frame.addr();
                        kernel_hal::frame_copy(frame.addr(), target_frame.addr());
                        self.frames.insert(
                            page_idx,
                            PageOrMarker{
                                inner: Some(target_frame),
                                state: PageOrMarkerState::Init
                        });
                    }
                    break;
                }
            }
            current_idx += locked_cur.parent_offset / PAGE_SIZE;
            current = locked_cur.parent.as_ref().cloned();
        }
        if res == 0 {
            let target_frame = PhysFrame::alloc().unwrap();
            res = target_frame.addr();
            kernel_hal::pmem_write(target_frame.addr(), &[0u8; PAGE_SIZE]);
            self.frames.insert(
                page_idx,
                PageOrMarker {
                    inner: Some(target_frame),
                    state: PageOrMarkerState::Init,
            });
        }
        assert_ne!(res, 0);
        res
    }

    fn attributed_pages(&self) -> u64 {
        let mut count: u64 = 0;
        for i in 0..self.size/PAGE_SIZE {
            if self.frames.contains_key(&i) {
                count += 1;
            } else {
                if self.parent_limit <= i * PAGE_SIZE {
                    continue;
                }
                let mut current = self.parent.as_ref().cloned();
                let mut current_idx = i + self.parent_offset / PAGE_SIZE;
                while let Some(locked_) = current {
                    let locked_cur = locked_.lock();
                    if let Some(frame) = locked_cur.frames.get(&current_idx) {
                        if frame.is_splited() {
                            count += 1;
                            break;
                        }
                    }
                    current_idx += locked_cur.parent_offset / PAGE_SIZE;
                    if current_idx >= locked_cur.parent_limit / PAGE_SIZE {
                        break;
                    }
                    current = locked_cur.parent.as_ref().cloned();
                }
            }
        }
        count
    }

    fn remove_child(&mut self, to_remove: &Weak<Mutex<Self>>) {
        self.children.retain(|child| child.strong_count() != 0 && !child.ptr_eq(to_remove));
        if self._type == VMOType::Hidden {
            assert!(self.children.len() == 1, "children num {:#x}", self.children.len());
            if self.children.is_empty() { self.frames.clear();return; }
            let weak_child = self.children.remove(0);
            let locked_child = weak_child.upgrade().unwrap();
            let mut child = locked_child.lock();
            let start = child.parent_offset / PAGE_SIZE;
            let end = child.parent_limit / PAGE_SIZE;
            debug!("from {:#x} to {:#x}", start, end);
            for (&key, value) in self.frames.range_mut(start..end) {
                let idx = key - start;
                debug!("merge idx {:#x} from {:#x} {:#x}", idx, key, start);
                if let Some(frame) = child.frames.get_mut(&idx) {
                    if frame.inner.is_some() {
                        continue;
                    }
                    frame.inner = value.inner.take();
                } else {
                    child.frames.insert(
                        idx,
                        PageOrMarker {
                            inner: value.inner.take(),
                            state: PageOrMarkerState::Init,
                        }
                    );
                }
            }
            self.frames.clear();
            let option_parent = self.parent.take();
            if let Some(parent) = &option_parent {
                parent.lock().children.push(weak_child);
            }
            child.parent = option_parent;
            child.parent_offset += self.parent_offset;
            child.parent_limit += self.parent_offset;
        }
    }

    fn create_child(&mut self, myself: &Arc<Mutex<VMObjectPagedInner>>, offset: usize, len: usize) -> Arc<Mutex<VMObjectPagedInner>> {
        let frames = core::mem::take(&mut self.frames);
        let old_parent = self.parent.take();

        // construct hidden_vmo as shared parent
        let hidden_vmo = Arc::new(Mutex::new(VMObjectPagedInner {
                _type: VMOType::Hidden,
                parent: old_parent.as_ref().cloned(),
                children: [Arc::downgrade(myself), Weak::new()].to_vec(),  // one of they will be changed below
                parent_offset: self.parent_offset,
                parent_limit: self.parent_limit,
                size: self.size,
                frames,
                mappings: Vec::new(),
        }));

        let weak_myself = Arc::downgrade(myself);
        if let Some(parent) = old_parent {
            parent.lock().children.iter_mut().for_each(|child| {
                if child.ptr_eq(&weak_myself) {
                    *child = Arc::downgrade(&hidden_vmo);
                }
            });
        }

        // change current vmo's parent
        self.parent = Some(hidden_vmo.clone());
        self.parent_offset = 0usize;
        self.parent_limit = self.size;

        self.mappings.iter().for_each(|map| map.remove_write_flag(pages(offset), pages(len)));

        // create hidden_vmo's another child as result
        let child_frames = BTreeMap::new();
        let child = Arc::new(Mutex::new(VMObjectPagedInner {
                _type: VMOType::Snapshot,
                parent: Some(hidden_vmo.clone()),
                children: Vec::new(),
                parent_offset: offset,
                parent_limit: offset + len,
                size: len,
                frames: child_frames,
                mappings: Vec::new(),
            }));
        hidden_vmo.lock().children[1] = Arc::downgrade(&child);
        child
    }

    fn destroy(&mut self, myself: &Weak<Mutex<Self>>) {
        assert_ne!(self._type, VMOType::Hidden);
        assert_eq!(self.children.len(), 0);
        match self._type {
            VMOType::Snapshot | VMOType::Origin => {
                if let Some(parent) = self.parent.as_ref() {
                    let mut p = parent.lock();
                    p.remove_child(myself);
                }
            }
            _ => {}
        }
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        if self._type == VMOType::Snapshot {
            info.flags |= VmoInfoFlags::IS_COW_CLONE.bits();
        }
        info.num_children = self.children.len() as u64;
        info.num_mappings = self.mappings.len() as u64;
        info.share_count  = self.mappings.len() as u64; // FIXME share_count should be the count of unique aspace
        info.commited_bytes = self.attributed_pages() * PAGE_SIZE as u64;
        // TODO cache_policy should be set up.
    }
}

impl Drop for VMObjectPagedInner {
    fn drop(&mut self) {
        match self._type {
            VMOType::Hidden => {
                assert_eq!(self.children.len(), 0);
                assert_eq!(self.frames.len(), 0);
            }
            VMOType::Snapshot | VMOType::Origin => {
                assert_eq!(self.children.len(), 0);
            }
        }
    }
}

impl Drop for VMObjectPaged {
    fn drop(&mut self) {
        let guard = self.global_mtx.lock();
        if Arc::strong_count(&self.inner) == 1 {
            self.inner.lock().destroy(&Arc::downgrade(&self.inner));
        }
        drop(guard);
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

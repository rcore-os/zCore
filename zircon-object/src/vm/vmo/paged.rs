use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::collections::BTreeMap,
    alloc::sync::{Arc, Weak},
    alloc::vec,
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::PhysFrame,
    spin::Mutex,
};

#[derive(PartialEq, Eq, Debug)]
enum VMOType {
    /// The original node.
    Origin,
    /// A snapshot of the parent node.
    Snapshot,
    /// Internal non-leaf node for snapshot.
    ///
    /// ```text
    ///    v---create_child
    ///    O       H <--- hidden node
    ///   /   =>  / \
    ///  S       O   S
    /// ```
    Hidden,
}

#[allow(dead_code)]
#[derive(PartialEq, Eq, Debug)]
enum PageOrMarkerState {
    Init,
    RightSplit,
    LeftSplit,
}

/// Page state in VMO.
///
/// It has 3 states:
/// - Zero:      The page will be lazily allocated.
/// - Allocated: The page is allocated to one frame.
/// - Parent:    The page is equal to parent.
struct PageOrMarker {
    pub inner: Option<PhysFrame>,
    state: PageOrMarkerState,
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

    /// This cow page has been forked, now we can see it as committed
    fn is_splited(&self) -> bool {
        self.state != PageOrMarkerState::Init
    }
}

impl Default for PageOrMarker {
    fn default() -> Self {
        PageOrMarker {
            inner: None,
            state: PageOrMarkerState::Init,
        }
    }
}

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Mutex<VMObjectPagedInner>,
    /// A weak reference to myself.
    self_ref: Weak<VMObjectPaged>,
}

#[allow(dead_code)]
/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    type_: VMOType,
    parent: Option<Arc<VMObjectPaged>>,
    children: Vec<Weak<VMObjectPaged>>,
    /// The offset from parent.
    parent_offset: usize,
    /// The size in bytes.
    size: usize,
    /// Physical frames of this VMO.
    frames: BTreeMap<usize, PageOrMarker>,
    /// All mappings to this VMO.
    mappings: Vec<Arc<VmMapping>>,
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        let mut frames = BTreeMap::new();
        for i in 0..pages {
            frames.insert(i, PageOrMarker::default());
        }
        VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Origin,
            parent: None,
            children: Vec::new(),
            parent_offset: 0usize,
            size: pages * PAGE_SIZE,
            frames,
            mappings: Vec::new(),
        })
    }

    /// Internal: Wrap an inner struct to object.
    fn wrap(inner: VMObjectPagedInner) -> Arc<Self> {
        let mut obj = Arc::new(VMObjectPaged {
            inner: Mutex::new(inner),
            self_ref: Weak::default(),
        });
        #[allow(unsafe_code)]
        unsafe {
            Arc::get_mut_unchecked(&mut obj).self_ref = Arc::downgrade(&obj);
        }
        obj
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
        self.inner.lock().size
    }

    fn set_len(&self, len: usize) {
        assert!(page_aligned(len));
        // FIXME parent and children? len < old_len?
        let mut inner = self.inner.lock();
        inner.size = len;
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

    fn decommit(&self, offset: usize, len: usize) -> ZxResult {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        if !inner.children.is_empty() || inner.parent.is_some() {
            return Err(ZxError::NOT_SUPPORTED);
        }
        for i in 0..pages {
            inner.decommit(start_page + i);
        }
        Ok(())
    }

    fn create_child(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let myself = self.self_ref.upgrade().unwrap();
        self.inner.lock().create_child(&myself, offset, len)
    }

    fn append_mapping(&self, mapping: Arc<VmMapping>) {
        self.inner.lock().mappings.push(mapping);
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        info.flags |= VmoInfoFlags::TYPE_PAGED;
        self.inner.lock().complete_info(info);
    }
}

impl VMObjectPagedInner {
    fn commit(&mut self, page_idx: usize) -> &PhysFrame {
        if let Some(value) = self.frames.get_mut(&page_idx) {
            value
                .inner
                .get_or_insert_with(|| PhysFrame::alloc().expect("failed to alloc frame"))
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
        if let Some(frame) = self.frames.get(&page_idx) {
            if let Some(frame) = &frame.inner {
                return frame.addr();
            }
        }
        let mut current = self.parent.clone();
        let mut current_idx = page_idx + self.parent_offset / PAGE_SIZE;
        while let Some(locked_) = current {
            let mut locked_cur = locked_.inner.lock();
            if let Some(_frame) = locked_cur.frames.get_mut(&current_idx) {
                if let Some(frame) = &_frame.inner {
                    if !flags.contains(MMUFlags::WRITE) {
                        // read-only
                        res = frame.addr();
                    } else {
                        _frame.state = PageOrMarkerState::LeftSplit;
                        let target_frame = PhysFrame::alloc().unwrap();
                        res = target_frame.addr();
                        kernel_hal::frame_copy(frame.addr(), target_frame.addr());
                        self.frames.insert(
                            page_idx,
                            PageOrMarker {
                                inner: Some(target_frame),
                                state: PageOrMarkerState::Init,
                            },
                        );
                    }
                    break;
                }
            }
            current_idx += locked_cur.parent_offset / PAGE_SIZE;
            current = locked_cur.parent.clone();
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
                },
            );
        }
        assert_ne!(res, 0);
        res
    }

    fn attributed_pages(&self) -> u64 {
        let mut count: u64 = 0;
        for i in 0..self.size / PAGE_SIZE {
            if self.frames.contains_key(&i) {
                count += 1;
            } else {
                if self.parent_limit() <= i * PAGE_SIZE {
                    continue;
                }
                let mut current = self.parent.clone();
                let mut current_idx = i + self.parent_offset / PAGE_SIZE;
                while let Some(locked_) = current {
                    let locked_cur = locked_.inner.lock();
                    if let Some(frame) = locked_cur.frames.get(&current_idx) {
                        if frame.is_splited() {
                            count += 1;
                            break;
                        }
                    }
                    current_idx += locked_cur.parent_offset / PAGE_SIZE;
                    if current_idx >= locked_cur.parent_limit() / PAGE_SIZE {
                        break;
                    }
                    current = locked_cur.parent.as_ref().cloned();
                }
            }
        }
        count
    }

    fn remove_child(&mut self, to_remove: &Weak<VMObjectPaged>) {
        self.children
            .retain(|child| child.strong_count() != 0 && !child.ptr_eq(to_remove));
        // shrink hidden node
        if self.type_ == VMOType::Hidden {
            assert_eq!(self.children.len(), 1);
            let weak_child = self.children.remove(0);
            let locked_child = weak_child.upgrade().unwrap();
            let mut child = locked_child.inner.lock();
            let start = child.parent_offset / PAGE_SIZE;
            let end = child.parent_limit() / PAGE_SIZE;
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
                        },
                    );
                }
            }
            self.frames.clear();
            let option_parent = self.parent.take();
            if let Some(parent) = &option_parent {
                parent.inner.lock().children.push(weak_child);
            }
            child.parent = option_parent;
            child.parent_offset += self.parent_offset;
        }
    }

    /// Create a snapshot child VMO.
    ///
    /// TODO: explain hidden
    fn create_child(
        &mut self,
        myself: &Arc<VMObjectPaged>,
        offset: usize,
        len: usize,
    ) -> Arc<VMObjectPaged> {
        // construct a hidden VMO as shared parent
        let hidden_vmo = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Hidden,
            parent: self.parent.clone(),
            children: vec![Arc::downgrade(myself), Weak::new()], // the right one will be changed below
            parent_offset: self.parent_offset,
            size: self.size,
            frames: core::mem::take(&mut self.frames),
            mappings: Vec::new(),
        });

        // update parent's children
        let weak_myself = Arc::downgrade(myself);
        if let Some(parent) = self.parent.take() {
            parent.inner.lock().children.iter_mut().for_each(|child| {
                if child.ptr_eq(&weak_myself) {
                    *child = Arc::downgrade(&hidden_vmo);
                }
            });
        }

        // change current vmo's parent
        self.parent = Some(hidden_vmo.clone());
        self.parent_offset = 0;

        for map in self.mappings.iter() {
            map.remove_write_flag(pages(offset), pages(len));
        }

        // create hidden_vmo's another child as result
        let child = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Snapshot,
            parent: Some(hidden_vmo.clone()),
            children: Vec::new(),
            parent_offset: offset,
            size: len,
            frames: BTreeMap::new(),
            mappings: Vec::new(),
        });
        hidden_vmo.inner.lock().children[1] = Arc::downgrade(&child);
        child
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        if self.type_ == VMOType::Snapshot {
            info.flags |= VmoInfoFlags::IS_COW_CLONE;
        }
        info.num_children = self.children.len() as u64;
        info.num_mappings = self.mappings.len() as u64;
        info.share_count = self.mappings.len() as u64; // FIXME share_count should be the count of unique aspace
        info.committed_bytes = self.attributed_pages() * PAGE_SIZE as u64;
        // TODO cache_policy should be set up.
    }

    fn parent_limit(&self) -> usize {
        self.parent_offset + self.size
    }
}

impl Drop for VMObjectPaged {
    fn drop(&mut self) {
        // remove self from parent
        if let Some(parent) = &self.inner.lock().parent {
            parent.inner.lock().remove_child(&self.self_ref);
        }
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
        let child_vmo = vmo.create_child(true, 0, 4 * 4096);
        child_vmo.read(0, &mut buf);
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.write(0, &[6, 7, 8, 9]);
        vmo.read(0, &mut buf);
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.read(0, &mut buf);
        assert_eq!(&buf, &[6, 7, 8, 9]);
    }
}

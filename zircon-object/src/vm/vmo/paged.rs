use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::collections::BTreeMap,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    core::ops::Range,
    kernel_hal::PhysFrame,
    spin::Mutex,
};

fn vmo_frame_alloc() -> ZxResult<PhysFrame> {
    VMO_PAGE_ALLOC.add(1);
    PhysFrame::alloc().ok_or(ZxError::NO_MEMORY)
}

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
    Hidden {
        left: Weak<VMObjectPaged>,
        right: Weak<VMObjectPaged>,
    },
}

impl VMOType {
    fn replace_child(&mut self, old: &Weak<VMObjectPaged>, new: Weak<VMObjectPaged>) {
        match self {
            VMOType::Hidden { left, right } => {
                if left.ptr_eq(old) {
                    *left = new;
                } else if right.ptr_eq(old) {
                    *right = new;
                }
            }
            _ => panic!(),
        }
    }
    fn get_tag_and_other(
        &self,
        child: &Weak<VMObjectPaged>,
    ) -> (PageStateTag, Weak<VMObjectPaged>) {
        match self {
            VMOType::Hidden { left, right } => {
                if left.ptr_eq(child) {
                    (PageStateTag::LeftSplit, right.clone())
                } else if right.ptr_eq(child) {
                    (PageStateTag::RightSplit, left.clone())
                } else {
                    (PageStateTag::Init, Weak::new())
                }
            }
            _ => (PageStateTag::Init, Weak::new()),
        }
    }
}

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Mutex<VMObjectPagedInner>,
    /// A weak reference to myself.
    self_ref: Weak<VMObjectPaged>,
}

/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    type_: VMOType,
    /// The owner of all shared pages in the hidden node.
    page_owner: KoID,
    /// Parent node.
    parent: Option<Arc<VMObjectPaged>>,
    /// The offset from parent.
    parent_offset: usize,
    /// The size in bytes.
    size: usize,
    /// Physical frames of this VMO.
    frames: BTreeMap<usize, PageState>,
    /// All mappings to this VMO.
    mappings: Vec<Arc<VmMapping>>,
}

/// Page state in VMO.
struct PageState {
    frame: PhysFrame,
    tag: PageStateTag,
}

#[derive(PartialEq, Eq, Debug)]
enum PageStateTag {
    Init,
    RightSplit,
    LeftSplit,
}

impl PageState {
    fn new(frame: PhysFrame) -> Self {
        PageState {
            frame,
            tag: PageStateTag::Init,
        }
    }
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize, user_id: KoID) -> Arc<Self> {
        VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Origin,
            page_owner: user_id,
            parent: None,
            parent_offset: 0usize,
            size: pages * PAGE_SIZE,
            frames: BTreeMap::new(),
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
    ) -> ZxResult {
        let iter = BlockIter {
            begin: offset,
            end: offset + buf_len,
            block_size_log2: 12,
        };
        for block in iter {
            let paddr = self.commit_page(block.block, flags)?;
            let buf_range = block.origin_begin() - offset..block.origin_end() - offset;
            f(paddr + block.begin, buf_range);
        }
        Ok(())
    }
}

impl VMObjectTrait for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult {
        self.for_each_page(offset, buf.len(), MMUFlags::READ, |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        })
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        self.for_each_page(offset, buf.len(), MMUFlags::WRITE, |paddr, buf_range| {
            kernel_hal::pmem_write(paddr, &buf[buf_range]);
        })
    }

    fn len(&self) -> usize {
        self.inner.lock().size
    }

    fn set_len(&self, len: usize) {
        assert!(page_aligned(len));
        // FIXME parent and children? len < old_len?
        let mut inner = self.inner.lock();
        inner.size = len;
        inner.frames.split_off(&(len / PAGE_SIZE));
    }

    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr> {
        match self.commit_page_internal(page_idx, flags, &Weak::new())? {
            CommitResult::Ok(paddr) => Ok(paddr),
            _ => unreachable!(),
        }
    }

    fn commit(&self, offset: usize, len: usize) -> ZxResult {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        for i in 0..pages {
            self.commit_page(start_page + i, MMUFlags::WRITE)?;
        }
        Ok(())
    }

    fn decommit(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        // non-slice child VMOs do not support decommit.
        if inner.parent.is_some() {
            return Err(ZxError::NOT_SUPPORTED);
        }
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        for i in 0..pages {
            inner.decommit(start_page + i);
        }
        Ok(())
    }

    fn create_child(&self, offset: usize, len: usize) -> Arc<dyn VMObjectTrait> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        self.inner.lock().create_child(&self.self_ref, offset, len)
    }

    fn append_mapping(&self, mapping: Arc<VmMapping>) {
        self.inner.lock().mappings.push(mapping);
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        info.flags |= VmoInfoFlags::TYPE_PAGED;
        self.inner.lock().complete_info(info);
    }
}

enum CommitResult {
    Ok(PhysAddr),
    CopyOnWrite(PhysFrame),
}

impl VMObjectPaged {
    /// Commit a page recursively.
    fn commit_page_internal(
        &self,
        page_idx: usize,
        flags: MMUFlags,
        child: &Weak<VMObjectPaged>,
    ) -> ZxResult<CommitResult> {
        let mut inner = self.inner.lock();
        // special case
        let out_of_range = page_idx >= inner.size / PAGE_SIZE;
        let no_frame = !inner.frames.contains_key(&page_idx);
        let no_parent = inner.parent.is_none();
        if out_of_range || no_frame && no_parent {
            if !flags.contains(MMUFlags::WRITE) {
                // read-only, just return zero frame
                return Ok(CommitResult::Ok(PhysFrame::zero_frame_addr()));
            }
            // lazy allocate zero frame
            let target_frame = vmo_frame_alloc()?;
            kernel_hal::frame_zero(target_frame.addr());
            inner.frames.insert(page_idx, PageState::new(target_frame));
        } else if no_frame {
            // if page miss on this VMO, recursively commit to parent
            let parent = inner.parent.as_ref().unwrap();
            let parent_idx = page_idx + inner.parent_offset / PAGE_SIZE;
            match parent.commit_page_internal(parent_idx, flags, &self.self_ref)? {
                r @ CommitResult::Ok(_) => return Ok(r),
                CommitResult::CopyOnWrite(frame) => {
                    inner.frames.insert(page_idx, PageState::new(frame));
                }
            }
        }
        // now the page must hit on this VMO
        let (child_tag, _other_child) = inner.type_.get_tag_and_other(child);
        let frame = inner.frames.get_mut(&page_idx).unwrap();
        if frame.tag != PageStateTag::Init {
            // has splitted, take out
            let target_frame = inner.frames.remove(&page_idx).unwrap().frame;
            return Ok(CommitResult::CopyOnWrite(target_frame));
        } else if flags.contains(MMUFlags::WRITE) && child_tag != PageStateTag::Init {
            // copy-on-write
            let target_frame = vmo_frame_alloc()?;
            kernel_hal::frame_copy(frame.frame.addr(), target_frame.addr());
            frame.tag = child_tag;
            return Ok(CommitResult::CopyOnWrite(target_frame));
        }
        // otherwise already committed
        return Ok(CommitResult::Ok(frame.frame.addr()));
    }
}

impl VMObjectPagedInner {
    fn decommit(&mut self, page_idx: usize) {
        self.frames.remove(&page_idx);
    }

    #[allow(dead_code)]
    fn range_change(&self, parent_offset: usize, parent_limit: usize, op: RangeChangeOp) {
        let mut start = self.parent_offset.max(parent_offset);
        let mut end = self.parent_limit().min(parent_limit);
        if start >= end {
            return;
        }
        start -= self.parent_offset;
        end -= self.parent_offset;
        for map in self.mappings.iter() {
            map.range_change(pages(start), pages(end), op);
        }
        if let VMOType::Hidden { left, right } = &self.type_ {
            for child in &[left, right] {
                let child = child.upgrade().unwrap();
                child.inner.lock().range_change(start, end, op);
            }
        }
    }

    /// Count committed pages of the VMO.
    fn committed_pages(&self) -> usize {
        let mut count = 0;
        for i in 0..self.size / PAGE_SIZE {
            if self.frames.contains_key(&i) {
                count += 1;
                continue;
            }
            if self.parent_limit() <= i * PAGE_SIZE {
                continue;
            }
            let mut current = self.parent.clone();
            let mut current_idx = i + self.parent_offset / PAGE_SIZE;
            while let Some(locked) = current {
                let locked_cur = locked.inner.lock();
                if let Some(frame) = locked_cur.frames.get(&current_idx) {
                    if frame.tag != PageStateTag::Init || locked_cur.page_owner == self.page_owner {
                        count += 1;
                        break;
                    }
                }
                current_idx += locked_cur.parent_offset / PAGE_SIZE;
                if current_idx >= locked_cur.parent_limit() / PAGE_SIZE {
                    break;
                }
                current = locked_cur.parent.clone();
            }
        }
        count
    }

    /// Remove one child and contract hidden node.
    ///
    /// ```text
    ///    |         |
    ///    H         |
    ///   / \    =>  |
    ///  A   B       B
    ///  ^remove
    /// ```
    fn remove_child(&mut self, myself: &Weak<VMObjectPaged>, child: &Weak<VMObjectPaged>) {
        let (tag, other_child) = self.type_.get_tag_and_other(child);
        let arc_child = other_child.upgrade().unwrap();
        let mut child = arc_child.inner.lock();
        let start = child.parent_offset / PAGE_SIZE;
        let end = child.parent_limit() / PAGE_SIZE;
        // merge nodes to the child
        for (key, value) in self.frames.split_off(&start) {
            if key >= end {
                break;
            }
            let idx = key - start;
            if !child.frames.contains_key(&idx) && value.tag == tag {
                child.frames.insert(idx, value);
            }
        }
        // connect child to my parent
        if let Some(parent) = &self.parent {
            parent.inner.lock().type_.replace_child(myself, other_child);
        }
        child.parent = self.parent.take();
        child.parent_offset += self.parent_offset;
    }

    /// Create a snapshot child VMO.
    fn create_child(
        &mut self,
        myself: &Weak<VMObjectPaged>,
        offset: usize,
        len: usize,
    ) -> Arc<VMObjectPaged> {
        // create child VMO
        let child = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Snapshot,
            page_owner: 0,
            parent: None, // set later
            parent_offset: offset,
            size: len,
            frames: BTreeMap::new(),
            mappings: Vec::new(),
        });
        // construct a hidden VMO as shared parent
        let hidden = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Hidden {
                left: myself.clone(),
                right: Arc::downgrade(&child),
            },
            page_owner: self.page_owner,
            parent: self.parent.clone(),
            parent_offset: self.parent_offset,
            size: self.size,
            frames: core::mem::take(&mut self.frames),
            mappings: Vec::new(),
        });
        // update parent's child
        if let Some(parent) = self.parent.take() {
            parent
                .inner
                .lock()
                .type_
                .replace_child(myself, Arc::downgrade(&hidden));
        }
        // update children's parent
        self.parent = Some(hidden.clone());
        self.parent_offset = 0;
        child.inner.lock().parent = Some(hidden.clone());
        // update mappings
        for map in self.mappings.iter() {
            map.range_change(pages(offset), pages(len), RangeChangeOp::RemoveWrite);
        }
        child
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        if let VMOType::Snapshot = self.type_ {
            info.flags |= VmoInfoFlags::IS_COW_CLONE;
        }
        info.num_children = match self.type_ {
            VMOType::Hidden { .. } => 2,
            _ => 0,
        };
        info.num_mappings = self.mappings.len() as u64;
        info.share_count = self.mappings.len() as u64; // FIXME share_count should be the count of unique aspace
        info.committed_bytes = (self.committed_pages() * PAGE_SIZE) as u64;
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
            parent
                .inner
                .lock()
                .remove_child(&parent.self_ref, &self.self_ref);
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
        let vmo = VmObject::new_paged(1);
        let child_vmo = vmo.create_child(false, 0, PAGE_SIZE);

        // write to parent and make sure clone doesn't see it
        vmo.test_write(0, 1);
        assert_eq!(vmo.test_read(0), 1);
        assert_eq!(child_vmo.test_read(0), 0);

        // write to clone and make sure parent doesn't see it
        child_vmo.test_write(0, 2);
        assert_eq!(vmo.test_read(0), 1);
        assert_eq!(child_vmo.test_read(0), 2);
    }

    #[test]
    fn committed_pages() {
        let vmo = VmObject::new_paged(1);
        let child_vmo = vmo.create_child(false, 0, PAGE_SIZE);

        // no committed pages
        assert_eq!(vmo.get_info().committed_bytes, 0);
        assert_eq!(child_vmo.get_info().committed_bytes, 0);

        // copy-on-write
        vmo.test_write(0, 1);
        assert_eq!(vmo.get_info().committed_bytes, 0x1000);
        assert_eq!(child_vmo.get_info().committed_bytes, 0x1000);
    }

    #[test]
    #[ignore] // FIXME
    fn zero_page_write() {
        let vmo0 = VmObject::new_paged(1);
        let vmo1 = vmo0.create_child(false, 0, PAGE_SIZE);
        let vmo2 = vmo0.create_child(false, 0, PAGE_SIZE);
        let vmos = [vmo0, vmo1, vmo2];
        let origin = vmo_page_bytes();

        // no committed pages
        for vmo in &vmos {
            assert_eq!(vmo.get_info().committed_bytes, 0);
        }

        // copy-on-write
        for i in 0..3 {
            vmos[i].test_write(0, i as u8);
            for j in 0..3 {
                assert_eq!(vmos[j].test_read(0), if j <= i { j as u8 } else { 0 });
                assert_eq!(
                    vmos[j].get_info().committed_bytes as usize,
                    if j <= i { PAGE_SIZE } else { 0 }
                );
            }
            assert_eq!(vmo_page_bytes() - origin, (i + 1) * PAGE_SIZE);
        }
    }

    impl VmObject {
        fn test_write(&self, page: usize, value: u8) {
            self.write(page * PAGE_SIZE, &[value]).unwrap();
        }

        fn test_read(&self, page: usize) -> u8 {
            let mut buf = [0; 1];
            self.read(page * PAGE_SIZE, &mut buf).unwrap();
            buf[0]
        }
    }
}

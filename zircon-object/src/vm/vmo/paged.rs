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
        /// The left child.
        left: WeakRef,
        /// The right child.
        right: WeakRef,
        /// The owner of all shared pages.
        owner: WeakRef,
        /// The next owner of all shared pages.
        owner1: WeakRef,
    },
}

impl VMOType {
    fn get_tag_and_other(&self, child: &WeakRef) -> (PageStateTag, WeakRef) {
        match self {
            VMOType::Hidden { left, right, .. } => {
                if left.ptr_eq(child) {
                    (PageStateTag::LeftSplit, right.clone())
                } else if right.ptr_eq(child) {
                    (PageStateTag::RightSplit, left.clone())
                } else {
                    (PageStateTag::Owned, Weak::new())
                }
            }
            _ => (PageStateTag::Owned, Weak::new()),
        }
    }

    fn is_hidden(&self) -> bool {
        matches!(self, VMOType::Hidden { .. })
    }
}

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Mutex<VMObjectPagedInner>,
}

type WeakRef = Weak<VMObjectPaged>;

/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    type_: VMOType,
    /// Parent node.
    parent: Option<Arc<VMObjectPaged>>,
    /// The offset from parent.
    parent_offset: usize,
    /// The range limit from parent.
    parent_limit: usize,
    /// The size in bytes.
    size: usize,
    /// Physical frames of this VMO.
    frames: BTreeMap<usize, PageState>,
    /// All mappings to this VMO.
    mappings: Vec<Weak<VmMapping>>,
    /// A weak reference to myself.
    self_ref: WeakRef,
}

/// Page state in VMO.
struct PageState {
    frame: PhysFrame,
    tag: PageStateTag,
}

/// The owner tag of pages in the node.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum PageStateTag {
    /// If the node is hidden, the page is shared by its 2 children.
    /// Otherwise, the page is owned by the node.
    Owned,
    /// The page is split to the left child and now owned by the right child.
    LeftSplit,
    /// The page is split to the right child and now owned by the left child.
    RightSplit,
}

impl PageStateTag {
    fn negate(self) -> Self {
        match self {
            PageStateTag::LeftSplit => PageStateTag::RightSplit,
            PageStateTag::RightSplit => PageStateTag::LeftSplit,
            PageStateTag::Owned => unreachable!(),
        }
    }
    fn is_split(self) -> bool {
        self != PageStateTag::Owned
    }
}

impl PageState {
    fn new(frame: PhysFrame) -> Self {
        VMO_PAGE_ALLOC.add(1);
        PageState {
            frame,
            tag: PageStateTag::Owned,
        }
    }
    #[allow(unsafe_code)]
    fn take(self) -> PhysFrame {
        let frame = unsafe { core::mem::transmute_copy(&self.frame) };
        VMO_PAGE_DEALLOC.add(1);
        core::mem::forget(self);
        frame
    }
}

impl Drop for PageState {
    fn drop(&mut self) {
        VMO_PAGE_DEALLOC.add(1);
    }
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(pages: usize) -> Arc<Self> {
        VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Origin,
            parent: None,
            parent_offset: 0usize,
            parent_limit: 0usize,
            size: pages * PAGE_SIZE,
            frames: BTreeMap::new(),
            mappings: Vec::new(),
            self_ref: Default::default(),
        })
    }

    /// Internal: Wrap an inner struct to object.
    fn wrap(inner: VMObjectPagedInner) -> Arc<Self> {
        let obj = Arc::new(VMObjectPaged {
            inner: Mutex::new(inner),
        });
        obj.inner.lock().self_ref = Arc::downgrade(&obj);
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
            CommitResult::Ref(paddr) => Ok(paddr),
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
        self.inner.lock().create_child(offset, len)
    }

    fn append_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mappings.push(mapping);
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        info.flags |= VmoInfoFlags::TYPE_PAGED;
        self.inner.lock().complete_info(info);
    }
}

enum CommitResult {
    /// A reference to existing page.
    Ref(PhysAddr),
    /// A new page copied-on-write.
    CopyOnWrite(PhysFrame),
    /// A new zero page.
    NewPage(PhysFrame),
}

impl VMObjectPaged {
    /// Commit a page recursively.
    fn commit_page_internal(
        &self,
        page_idx: usize,
        flags: MMUFlags,
        child: &WeakRef,
    ) -> ZxResult<CommitResult> {
        let mut inner = self.inner.lock();
        // special case
        let no_parent = inner.parent.is_none();
        let no_frame = !inner.frames.contains_key(&page_idx);
        let out_of_range = if inner.type_.is_hidden() || inner.parent.is_none() {
            page_idx >= inner.size / PAGE_SIZE
        } else {
            (inner.parent_offset + page_idx * PAGE_SIZE) >= inner.parent_limit
        };
        if no_frame {
            // if out_of_range
            if out_of_range || no_parent {
                if !flags.contains(MMUFlags::WRITE) {
                    // read-only, just return zero frame
                    return Ok(CommitResult::Ref(PhysFrame::zero_frame_addr()));
                }
                // lazy allocate zero frame
                let target_frame = PhysFrame::alloc().ok_or(ZxError::NO_MEMORY)?;
                kernel_hal::frame_zero(target_frame.addr());
                if out_of_range {
                    // can never be a hidden vmo
                    assert!(!inner.type_.is_hidden());
                }
                if inner.type_.is_hidden() {
                    return Ok(CommitResult::NewPage(target_frame));
                }
                inner.frames.insert(page_idx, PageState::new(target_frame));
            } else {
                // recursively find a frame in parent
                let parent = inner.parent.as_ref().unwrap();
                let parent_idx = page_idx + inner.parent_offset / PAGE_SIZE;
                match parent.commit_page_internal(parent_idx, flags, &inner.self_ref)? {
                    CommitResult::NewPage(frame) if !inner.type_.is_hidden() => {
                        inner.frames.insert(page_idx, PageState::new(frame));
                    }
                    CommitResult::CopyOnWrite(frame) => {
                        inner.frames.insert(page_idx, PageState::new(frame));
                    }
                    r => return Ok(r),
                }
            }
        }

        // now the page must hit on this VMO
        let (child_tag, other_child) = inner.type_.get_tag_and_other(child);
        if inner.type_.is_hidden() {
            let arc_other = other_child.upgrade().unwrap();
            let locked_other = arc_other.inner.lock();
            let in_range = {
                let start = locked_other.parent_offset / PAGE_SIZE;
                let end = locked_other.parent_limit / PAGE_SIZE;
                page_idx >= start && page_idx < end
            };
            if !in_range {
                let frame = inner.frames.remove(&page_idx).unwrap().take();
                return Ok(CommitResult::CopyOnWrite(frame));
            }
        }
        let frame = inner.frames.get_mut(&page_idx).unwrap();
        if frame.tag.is_split() {
            // has split, take out
            let target_frame = inner.frames.remove(&page_idx).unwrap().take();
            return Ok(CommitResult::CopyOnWrite(target_frame));
        } else if flags.contains(MMUFlags::WRITE) && child_tag.is_split() {
            // copy-on-write
            let target_frame = PhysFrame::alloc().ok_or(ZxError::NO_MEMORY)?;
            kernel_hal::frame_copy(frame.frame.addr(), target_frame.addr());
            frame.tag = child_tag;
            return Ok(CommitResult::CopyOnWrite(target_frame));
        }
        // otherwise already committed
        return Ok(CommitResult::Ref(frame.frame.addr()));
    }

    /// Replace a child of the hidden node.
    /// `new_start` and `new_end` are in bytes
    fn replace_child(&self, old: &WeakRef, new: WeakRef, new_range: Option<(usize, usize)>) {
        let mut inner = self.inner.lock();
        if let Some((new_start, new_end)) = new_range {
            let (tag, other) = inner.type_.get_tag_and_other(old);
            let arc_other_child = other.upgrade().unwrap();
            let other_child = arc_other_child.inner.lock();
            let other_start = other_child.parent_offset;
            let other_end = other_child.parent_limit;
            let start = new_start.min(other_start) / PAGE_SIZE;
            let end = new_end.max(other_end) / PAGE_SIZE;
            for i in 0..inner.size / PAGE_SIZE {
                if start <= i && end > i {
                    if let Some(frame) = inner.frames.get(&i) {
                        if frame.tag != tag.negate() {
                            continue;
                        }
                    }
                }
                inner.frames.remove(&i);
            }
        }
        // judge direction
        match &mut inner.type_ {
            VMOType::Hidden { left, right, .. } => {
                if left.ptr_eq(old) {
                    *left = new;
                } else if right.ptr_eq(old) {
                    *right = new;
                } else {
                    panic!();
                }
            }
            _ => panic!(),
        }
    }
}

impl VMObjectPagedInner {
    fn decommit(&mut self, page_idx: usize) {
        self.frames.remove(&page_idx);
    }

    #[allow(dead_code)]
    fn range_change(&self, parent_offset: usize, parent_limit: usize, op: RangeChangeOp) {
        let mut start = self.parent_offset.max(parent_offset);
        let mut end = self.parent_limit.min(parent_limit);
        if start >= end {
            return;
        }
        start -= self.parent_offset;
        end -= self.parent_offset;
        for map in self.mappings.iter() {
            if let Some(map) = map.upgrade() {
                map.range_change(pages(start), pages(end), op);
            }
        }
        if let VMOType::Hidden { left, right, .. } = &self.type_ {
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
            if self.parent_limit <= i * PAGE_SIZE {
                continue;
            }
            let mut current = self.parent.clone();
            let mut current_idx = i + self.parent_offset / PAGE_SIZE;
            while let Some(locked) = current {
                let inner = locked.inner.lock();
                if let Some(frame) = inner.frames.get(&current_idx) {
                    if frame.tag.is_split() || inner.is_owned_by(&self.self_ref) {
                        count += 1;
                        break;
                    }
                }
                current_idx += inner.parent_offset / PAGE_SIZE;
                if current_idx >= inner.parent_limit / PAGE_SIZE {
                    break;
                }
                current = inner.parent.clone();
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
    fn remove_child(&mut self, child: &WeakRef) {
        let (tag, other_child) = self.type_.get_tag_and_other(child);
        let arc_child = other_child.upgrade().unwrap();
        let mut child = arc_child.inner.lock();
        let start = child.parent_offset / PAGE_SIZE;
        let end = child.parent_limit / PAGE_SIZE;
        // merge nodes to the child
        for (key, value) in self.frames.split_off(&start) {
            if key >= end {
                break;
            }
            let idx = key - start;
            if !child.frames.contains_key(&idx) && value.tag != tag.negate() {
                child.frames.insert(idx, value);
            }
        }
        // connect child to my parent
        child.parent_offset += self.parent_offset;
        child.parent_limit += self.parent_offset;
        if let Some(parent) = &self.parent {
            parent.replace_child(
                &self.self_ref,
                other_child,
                Some((child.parent_offset, child.parent_limit))
            );
        }
        child.parent = self.parent.take();
    }

    /// Create a snapshot child VMO.
    fn create_child(&mut self, offset: usize, len: usize) -> Arc<VMObjectPaged> {
        // create child VMO
        let child = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Snapshot,
            parent: None, // set later
            parent_offset: offset,
            parent_limit: (offset + len).min(self.size),
            size: len,
            frames: BTreeMap::new(),
            mappings: Vec::new(),
            self_ref: Default::default(),
        });
        // construct a hidden VMO as shared parent
        let hidden = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Hidden {
                left: self.self_ref.clone(),
                right: Arc::downgrade(&child),
                owner: self.self_ref.clone(),
                owner1: Arc::downgrade(&child),
            },
            parent: self.parent.clone(),
            parent_offset: self.parent_offset,
            parent_limit: self.parent_limit,
            size: self.size,
            frames: core::mem::take(&mut self.frames),
            mappings: Vec::new(),
            self_ref: Default::default(),
        });
        // update parent's child
        if let Some(parent) = self.parent.take() {
            parent.replace_child(
                &self.self_ref,
                Arc::downgrade(&hidden),
                None
            );
        }
        // update children's parent
        self.parent = Some(hidden.clone());
        self.parent_offset = 0;
        self.parent_limit = self.size;
        child.inner.lock().parent = Some(hidden.clone());
        // update mappings
        for map in self.mappings.iter() {
            if let Some(map) = map.upgrade() {
                map.range_change(pages(offset), pages(len), RangeChangeOp::RemoveWrite);
            }
        }
        child
    }

    fn complete_info(&self, info: &mut ZxInfoVmo) {
        if let VMOType::Snapshot = self.type_ {
            info.flags |= VmoInfoFlags::IS_COW_CLONE;
        }
        info.num_children = if self.type_.is_hidden() { 2 } else { 0 };
        info.num_mappings = self.mappings.len() as u64; // FIXME remove weak ptr
        info.share_count = self.mappings.len() as u64; // FIXME share_count should be the count of unique aspace
        info.committed_bytes = (self.committed_pages() * PAGE_SIZE) as u64;
        // TODO cache_policy should be set up.
    }

    fn is_owned_by(&self, node: &WeakRef) -> bool {
        match &self.type_ {
            VMOType::Hidden { owner, owner1, .. } => {
                if owner.strong_count() == 0 {
                    owner1.ptr_eq(node)
                } else {
                    owner.ptr_eq(node)
                }
            }
            _ => panic!(),
        }
    }
}

impl Drop for VMObjectPaged {
    fn drop(&mut self) {
        let inner = self.inner.lock();
        // remove self from parent
        if let Some(parent) = &inner.parent {
            parent.inner.lock().remove_child(&inner.self_ref);
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

    #[test]
    fn overflow() {
        let vmo0 = VmObject::new_paged(2);
        vmo0.test_write(0, 1);
        let vmo1 = vmo0.create_child(false, 0, 2 * PAGE_SIZE);
        vmo1.test_write(1, 2);
        let vmo2 = vmo1.create_child(false, 0, 3 * PAGE_SIZE);
        vmo2.test_write(2, 3);
        assert_eq!(vmo0.get_info().committed_bytes as usize, PAGE_SIZE);
        assert_eq!(vmo1.get_info().committed_bytes as usize, PAGE_SIZE);
        assert_eq!(vmo2.get_info().committed_bytes as usize, PAGE_SIZE);
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

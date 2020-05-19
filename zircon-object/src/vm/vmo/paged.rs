use {
    super::*,
    crate::util::block_range::BlockIter,
    alloc::collections::VecDeque,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    core::ops::Range,
    core::sync::atomic::*,
    hashbrown::HashMap,
    kernel_hal::{frame_flush, PhysFrame, PAGE_SIZE},
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
    /// Id of this vmo object
    user_id: KoID,
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
    frames: HashMap<usize, PageState>,
    /// All mappings to this VMO.
    mappings: Vec<Weak<VmMapping>>,
    /// Cache Policy
    cache_policy: CachePolicy,
    /// Is contiguous
    contiguous: bool,
    /// A weak reference to myself.
    self_ref: WeakRef,
    /// Sum of pin_count
    pin_count: usize,
}

/// Page state in VMO.
struct PageState {
    frame: PhysFrame,
    tag: PageStateTag,
    pin_count: u8,
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
            pin_count: 0,
        }
    }
    #[allow(unsafe_code)]
    fn take(self) -> PhysFrame {
        let frame = unsafe { core::mem::transmute_copy(&self.frame) };
        VMO_PAGE_DEALLOC.add(1);
        core::mem::forget(self);
        frame
    }
    fn swap(&mut self, t: &mut Self) {
        core::mem::swap(&mut self.frame, &mut t.frame);
        core::mem::swap(&mut self.pin_count, &mut t.pin_count);
    }
}

impl Drop for PageState {
    fn drop(&mut self) {
        VMO_PAGE_DEALLOC.add(1);
    }
}

impl VMObjectPaged {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new(id: KoID, pages: usize) -> Arc<Self> {
        VMObjectPaged::wrap(VMObjectPagedInner {
            user_id: id,
            type_: VMOType::Origin,
            parent: None,
            parent_offset: 0usize,
            parent_limit: 0usize,
            size: pages * PAGE_SIZE,
            frames: HashMap::new(),
            mappings: Vec::new(),
            cache_policy: CachePolicy::Cached,
            contiguous: false,
            self_ref: Default::default(),
            pin_count: 0,
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
        if self.cache_policy() != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        self.for_each_page(offset, buf.len(), MMUFlags::READ, |paddr, buf_range| {
            kernel_hal::pmem_read(paddr, &mut buf[buf_range]);
        })
    }

    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult {
        if self.cache_policy() != CachePolicy::Cached {
            return Err(ZxError::BAD_STATE);
        }
        self.for_each_page(offset, buf.len(), MMUFlags::WRITE, |paddr, buf_range| {
            kernel_hal::pmem_write(paddr, &buf[buf_range]);
        })
    }

    fn zero(&self, offset: usize, len: usize) -> ZxResult {
        if offset + len > self.inner.lock().size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let iter = BlockIter {
            begin: offset,
            end: offset + len,
            block_size_log2: 12,
        };
        let mut unwanted = VecDeque::new();
        for block in iter {
            //let paddr = self.commit_page(block.block, MMUFlags::READ)?;
            if block.len() == PAGE_SIZE && !self.is_contiguous() {
                let _ = self.commit_page(block.block, MMUFlags::WRITE)?;
                unwanted.push_back(block.block);
                self.inner.lock().frames.remove(&block.block);
            } else if self.committed_pages_in_range(block.block, block.block + 1) != 0 {
                // check whether this page is initialized, otherwise nothing should be done
                let paddr = self.commit_page(block.block, MMUFlags::WRITE)?;
                kernel_hal::frame_zero_in_range(paddr, block.begin, block.end);
            }
        }
        self.inner.lock().release_unwanted_pages(unwanted);
        Ok(())
    }

    fn len(&self) -> usize {
        self.inner.lock().size
    }

    fn set_len(&self, len: usize) -> ZxResult {
        assert!(page_aligned(len));
        let mut inner = self.inner.lock();
        if inner.pin_count > 0 {
            return Err(ZxError::BAD_STATE);
        }
        inner.resize(len);
        Ok(())
    }

    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr> {
        let ret = match self.commit_page_internal(page_idx, flags, &Weak::new())? {
            CommitResult::Ref(paddr) => Ok(paddr),
            _ => unreachable!(),
        };
        // force check conntiguous on each leaf node
        assert!(self.check_contig());
        ret
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

    fn create_child(
        &self,
        offset: usize,
        len: usize,
        user_id: KoID,
    ) -> ZxResult<Arc<dyn VMObjectTrait>> {
        assert!(page_aligned(offset));
        assert!(page_aligned(len));
        let child = self.inner.lock().create_child(offset, len, user_id)?;
        Ok(child)
    }

    fn append_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mappings.push(mapping);
    }

    fn remove_mapping(&self, mapping: Weak<VmMapping>) {
        let mut inner = self.inner.lock();
        inner
            .mappings
            .drain_filter(|x| x.strong_count() == 0 || Weak::ptr_eq(x, &mapping));
    }

    fn complete_info(&self, info: &mut VmoInfo) {
        info.flags |= VmoInfoFlags::TYPE_PAGED;
        self.inner.lock().complete_info(info);
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
        inner.clear_invalild_mappings();
        if !inner.mappings.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        if inner.parent.is_some() {
            return Err(ZxError::BAD_STATE);
        }
        if inner.pin_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        if inner.cache_policy == CachePolicy::Cached && policy != CachePolicy::Cached {
            for (_, value) in inner.frames.iter() {
                frame_flush(value.frame.addr());
            }
        }
        inner.cache_policy = policy;
        Ok(())
    }

    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize {
        let inner = self.inner.lock();
        inner.committed_pages_in_range(start_idx, end_idx)
    }

    fn share_count(&self) -> usize {
        self.inner.lock().mappings.len()
    }

    fn pin(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        if offset + len >= inner.size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if len == 0 {
            return Ok(());
        }
        let start_page = offset / PAGE_SIZE;
        let end_page = pages(offset + len);
        for i in start_page..end_page {
            let frame = inner.frames.get(&i).unwrap();
            if frame.pin_count == VM_PAGE_OBJECT_MAX_PIN_COUNT {
                return Err(ZxError::UNAVAILABLE);
            }
        }
        for i in start_page..end_page {
            inner.frames.get_mut(&i).unwrap().pin_count += 1;
            inner.pin_count += 1;
        }
        Ok(())
    }

    fn unpin(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        if offset + len >= inner.size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if len == 0 {
            return Ok(());
        }
        let start_page = offset / PAGE_SIZE;
        let end_page = pages(offset + len);
        for i in start_page..end_page {
            let frame = inner.frames.get(&i).unwrap();
            if frame.pin_count == 0 {
                return Err(ZxError::UNAVAILABLE);
            }
        }
        assert_ne!(inner.pin_count, 0);
        for i in start_page..end_page {
            inner.frames.get_mut(&i).unwrap().pin_count -= 1;
        }
        Ok(())
    }

    fn is_contiguous(&self) -> bool {
        self.inner.lock().is_contiguous()
    }

    fn is_paged(&self) -> bool {
        true
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
                kernel_hal::frame_zero_in_range(target_frame.addr(), 0, PAGE_SIZE);
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
                        let mut new_frame = PageState::new(frame);
                        // Cloning a contiguous vmo: original frames are stored in hidden parent nodes.
                        // In order to make sure original vmo (now is a child of hidden parent)
                        // owns physically contiguous frames, swap the new frame with the original
                        if inner.contiguous {
                            let mut parent_inner = parent.inner.lock();
                            if let Some(par_frame) = parent_inner.frames.get_mut(&parent_idx) {
                                par_frame.swap(&mut new_frame);
                            }
                        }
                        inner.frames.insert(page_idx, new_frame);
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
        Ok(CommitResult::Ref(frame.frame.addr()))
    }

    /// Replace a child of the hidden node.
    /// `new_start` and `new_end` are in bytes
    fn replace_child(
        &self,
        old: &WeakRef,
        old_id: KoID,
        new: WeakRef,
        new_range: Option<(usize, usize)>,
    ) {
        let mut inner = self.inner.lock();
        let (tag, other) = inner.type_.get_tag_and_other(old);
        let arc_other_child = other.upgrade().unwrap();
        let mut other_child = arc_other_child.inner.lock();
        let mut unwanted = VecDeque::<usize>::new();
        if let Some((new_start, new_end)) = new_range {
            let other_start = other_child.parent_offset / PAGE_SIZE;
            let other_end = other_child.parent_limit / PAGE_SIZE;
            let start = new_start / PAGE_SIZE;
            let end = new_end / PAGE_SIZE;
            for i in 0..inner.size / PAGE_SIZE {
                let not_in_range =
                    !((start <= i && end > i) || (other_start <= i && other_end > i));
                if not_in_range {
                    // if not in this node's range
                    if inner.frames.contains_key(&i) {
                        // if the frame is in our, remove it
                        assert!(inner.frames.remove(&i).is_some());
                    } else {
                        // or it is in our ancestor, tell them we do not need it.
                        unwanted.push_back(i + inner.parent_offset / PAGE_SIZE);
                    }
                } else {
                    // if in this node's range, check if it can be moved
                    if let Some(frame) = inner.frames.get(&i) {
                        if frame.tag.is_split() {
                            let mut new_frame = inner.frames.remove(&i).unwrap();
                            if inner.contiguous
                                && !other_child.contiguous
                                && new_frame.pin_count >= 1
                            {
                                new_frame.pin_count -= 1;
                            }
                            if new_frame.tag == tag && other_start <= i && other_end > i {
                                new_frame.tag = PageStateTag::Owned;
                                let new_key = i - other_child.parent_offset / PAGE_SIZE;
                                other_child.frames.insert(new_key, new_frame);
                            }
                        }
                    }
                }
            }
        }

        inner.release_unwanted_pages(unwanted);

        if old_id == inner.user_id {
            let mut option_parent = inner.parent.clone();
            let mut child = inner.self_ref.clone();
            let mut skip_user_id = old_id;
            while let Some(parent) = option_parent {
                let mut locked_parent = parent.inner.lock();
                if locked_parent.user_id == old_id {
                    let (_, other) = locked_parent.type_.get_tag_and_other(&child);
                    let new_user_id = other.upgrade().unwrap().inner.lock().user_id;
                    child = locked_parent.self_ref.clone();
                    assert_ne!(new_user_id, skip_user_id);
                    locked_parent.user_id = new_user_id;
                    skip_user_id = new_user_id;
                    option_parent = locked_parent.parent.clone();
                } else {
                    break;
                }
            }
        }

        inner.user_id = other_child.user_id;
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

    /// Create a list of contiguous pages
    pub fn create_contiguous(&self, size: usize, align_log2: usize) -> ZxResult {
        assert!(page_aligned(size));
        let size_page = pages(size);
        let mut frames = PhysFrame::alloc_contiguous(size_page, align_log2 - PAGE_SIZE_LOG2);
        if frames.is_empty() {
            return Err(ZxError::NO_MEMORY);
        }
        let mut inner = self.inner.lock();
        inner.contiguous = true;
        for (i, f) in frames.drain(0..).enumerate() {
            kernel_hal::frame_zero_in_range(f.addr(), 0, PAGE_SIZE);
            let mut state = PageState::new(f);
            state.pin_count += 1;
            inner.frames.insert(i, state);
        }
        Ok(())
    }

    /// Check whether it is not physically contiguous when it should be
    fn check_contig(&self) -> bool {
        let inner = self.inner.lock();
        if !inner.contiguous {
            return true;
        }
        let mut base = 0;
        for (key, ps) in inner.frames.iter() {
            let new_base = ps.frame.addr() - key * PAGE_SIZE;
            if base == 0 || new_base == base {
                base = new_base;
            } else {
                return false;
            }
        }
        true
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
    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize {
        assert!(
            start_idx < self.size / PAGE_SIZE || start_idx == 0,
            "start_idx {:#x}, self.size {:#x}",
            start_idx,
            self.size
        );
        assert!(
            end_idx <= self.size / PAGE_SIZE,
            "end_idx {:#x}, self.size {:#x}",
            end_idx,
            self.size
        );
        let mut count = 0;
        for i in start_idx..end_idx {
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
                    if frame.tag.is_split() || inner.user_id == self.user_id {
                        count += 1;
                        break;
                    }
                }
                if inner.user_id != self.user_id {
                    break;
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
        // a child slice do not have to belong to a hidden parent
        if !self.type_.is_hidden() {
            return;
        }
        let (tag, other_child) = self.type_.get_tag_and_other(child);
        let arc_child = other_child.upgrade().unwrap();
        let mut child = arc_child.inner.lock();
        let start = child.parent_offset / PAGE_SIZE;
        let end = child.parent_limit / PAGE_SIZE;
        // merge nodes to the child
        for (key, mut value) in core::mem::take(&mut self.frames) {
            if key < start || key >= end {
                continue;
            }
            if self.contiguous && !child.contiguous && value.pin_count >= 1 {
                value.pin_count -= 1;
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
                self.user_id,
                other_child,
                Some((child.parent_offset, child.parent_limit)),
            );
        }
        child.parent = self.parent.take();
    }

    /// Create a snapshot child VMO.
    fn create_child(
        &mut self,
        offset: usize,
        len: usize,
        user_id: KoID,
    ) -> ZxResult<Arc<VMObjectPaged>> {
        if self.cache_policy != CachePolicy::Cached || self.pin_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        // create child VMO
        let child = VMObjectPaged::wrap(VMObjectPagedInner {
            user_id,
            type_: VMOType::Snapshot,
            parent: None, // set later
            parent_offset: offset,
            parent_limit: (offset + len).min(self.size),
            size: len,
            frames: HashMap::new(),
            mappings: Vec::new(),
            cache_policy: CachePolicy::Cached,
            contiguous: false,
            self_ref: Default::default(),
            pin_count: 0,
        });
        // construct a hidden VMO as shared parent
        let hidden = VMObjectPaged::wrap(VMObjectPagedInner {
            user_id: self.user_id,
            type_: VMOType::Hidden {
                left: self.self_ref.clone(),
                right: Arc::downgrade(&child),
            },
            parent: self.parent.clone(),
            parent_offset: self.parent_offset,
            parent_limit: self.parent_limit,
            size: self.size,
            frames: core::mem::take(&mut self.frames),
            mappings: Vec::new(),
            cache_policy: CachePolicy::Cached,
            contiguous: self.contiguous,
            self_ref: Default::default(),
            pin_count: self.pin_count,
        });
        // update parent's child
        if let Some(parent) = self.parent.take() {
            if let VMOType::Hidden { left, right, .. } = &mut parent.inner.lock().type_ {
                if left.ptr_eq(&self.self_ref) {
                    *left = Arc::downgrade(&hidden);
                } else if right.ptr_eq(&self.self_ref) {
                    *right = Arc::downgrade(&hidden);
                } else {
                    panic!();
                }
            }
        }
        // update children's parent
        self.parent = Some(hidden.clone());
        self.parent_offset = 0;
        self.parent_limit = self.size;
        child.inner.lock().parent = Some(hidden);
        // update mappings
        for map in self.mappings.iter() {
            if let Some(map) = map.upgrade() {
                map.range_change(pages(offset), pages(len), RangeChangeOp::RemoveWrite);
            }
        }
        Ok(child)
    }

    fn complete_info(&self, info: &mut VmoInfo) {
        if let VMOType::Snapshot = self.type_ {
            info.flags |= VmoInfoFlags::IS_COW_CLONE;
        }
        if self.is_contiguous() {
            info.flags |= VmoInfoFlags::CONTIGUOUS;
        }
        // info.num_children = if self.type_.is_hidden() { 2 } else { 0 };
        info.num_mappings = self.mappings.len() as u64; // FIXME remove weak ptr
        info.share_count = self.mappings.len() as u64; // FIXME share_count should be the count of unique aspace
        info.committed_bytes =
            (self.committed_pages_in_range(0, self.size / PAGE_SIZE) * PAGE_SIZE) as u64;
    }

    fn release_unwanted_pages(&mut self, mut unwanted: VecDeque<usize>) {
        let mut option_parent = self.parent.clone();
        let mut child = self.self_ref.clone();
        while let Some(parent) = option_parent {
            let mut locked_parent = parent.inner.lock();
            let (tag, other) = locked_parent.type_.get_tag_and_other(&child);
            let arc_other = other.upgrade().unwrap();
            let mut locked_other = arc_other.inner.lock();
            let start = locked_other.parent_offset / PAGE_SIZE;
            let end = locked_other.parent_limit / PAGE_SIZE;
            for _ in 0..unwanted.len() {
                let idx = unwanted.pop_front().unwrap();
                // if the frame is in locked_other's range, check if it can be move to locked_other
                if start <= idx && idx < end {
                    if locked_parent.frames.contains_key(&idx) {
                        let mut to_insert = locked_parent.frames.remove(&idx).unwrap();
                        if locked_parent.contiguous
                            && !locked_other.contiguous
                            && to_insert.pin_count >= 1
                        {
                            to_insert.pin_count -= 1;
                        }
                        if to_insert.tag != tag.negate() {
                            to_insert.tag = PageStateTag::Owned;
                            locked_other.frames.insert(idx - start, to_insert);
                        }
                        unwanted.push_back(idx + locked_parent.parent_offset / PAGE_SIZE);
                    }
                } else {
                    // otherwise, if it exists in our frames, remove it; if not, push_back it again
                    if locked_parent.frames.contains_key(&idx) {
                        locked_parent.frames.remove(&idx);
                    } else {
                        unwanted.push_back(idx + locked_parent.parent_offset / PAGE_SIZE);
                    }
                }
            }
            child = locked_parent.self_ref.clone();
            option_parent = locked_parent.parent.clone();
            drop(locked_parent);
        }
    }

    fn resize(&mut self, new_size: usize) {
        if new_size == 0 && new_size < self.size {
            self.frames.clear();
            if let Some(parent) = self.parent.as_ref() {
                parent.inner.lock().remove_child(&self.self_ref);
                self.parent = None;
            }
        } else if new_size < self.size {
            let mut unwanted = VecDeque::<usize>::new();
            let parent_end = (self.parent_limit - self.parent_offset) / PAGE_SIZE;
            for i in new_size / PAGE_SIZE..self.size / PAGE_SIZE {
                if parent_end > i {
                    unwanted.push_back(i);
                }
            }
            self.release_unwanted_pages(unwanted);
        }
        self.size = new_size;
    }

    fn is_contiguous(&self) -> bool {
        self.contiguous
    }

    fn clear_invalild_mappings(&mut self) {
        self.mappings.drain_filter(|x| x.strong_count() == 0);
    }
}

impl Drop for VMObjectPaged {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        // remove self from parent
        if let Some(parent) = &inner.parent {
            parent.inner.lock().remove_child(&inner.self_ref);
        }
        let is_conti = inner.is_contiguous();
        for frame in inner.frames.iter_mut() {
            if is_conti {
                // WARN: In fact we do not need this `if`.
                // If this vmo is a child of a contiguous vmo,
                // its pages should also be pinned.
                if frame.1.pin_count >= 1 {
                    frame.1.pin_count -= 1;
                }
            }
            assert_eq!(frame.1.pin_count, 0);
        }
    }
}

#[allow(dead_code)]
/// Generate a owner ID.
fn new_owner_id() -> u64 {
    static OWNER_ID: AtomicU64 = AtomicU64::new(1);
    OWNER_ID.fetch_add(1, Ordering::SeqCst)
}

const VM_PAGE_OBJECT_MAX_PIN_COUNT: u8 = 31;

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
        let child_vmo = vmo.create_child(false, 0, PAGE_SIZE).unwrap();

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
        let vmo1 = vmo0.create_child(false, 0, PAGE_SIZE).unwrap();
        let vmo2 = vmo0.create_child(false, 0, PAGE_SIZE).unwrap();
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
        let vmo1 = vmo0.create_child(false, 0, 2 * PAGE_SIZE).unwrap();
        vmo1.test_write(1, 2);
        let vmo2 = vmo1.create_child(false, 0, 3 * PAGE_SIZE).unwrap();
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

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

fn vmo_frame_alloc() -> ZxResult<PhysFrame> {
    VMO_PAGE_ALLOC.add(1);
    PhysFrame::alloc().ok_or(ZxError::NO_MEMORY)
}

fn vmo_alloc_copy_frame(paddr: PhysAddr) -> ZxResult<PhysFrame> {
    let target = vmo_frame_alloc()?;
    kernel_hal::frame_copy(paddr, target.addr());
    Ok(target)
}

fn vmo_alloc_zero_frame() -> ZxResult<PhysFrame> {
    let target = vmo_frame_alloc()?;
    kernel_hal::frame_zero(target.addr());
    Ok(target)
}

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

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    inner: Mutex<VMObjectPagedInner>,
    /// A weak reference to myself.
    self_ref: Weak<VMObjectPaged>,
}

/// The mutable part of `VMObjectPaged`.
struct VMObjectPagedInner {
    type_: VMOType,
    page_attribution_user_id: KoID,
    parent: Option<Arc<VMObjectPaged>>,
    children: Vec<Weak<VMObjectPaged>>,
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
            page_attribution_user_id: user_id,
            parent: None,
            children: Vec::new(),
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
            let paddr = self.inner.lock().commit_page(block.block, flags)?;
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
        self.inner.lock().commit_page(page_idx, flags)
    }

    fn commit(&self, offset: usize, len: usize) -> ZxResult {
        let start_page = offset / PAGE_SIZE;
        let pages = len / PAGE_SIZE;
        let mut inner = self.inner.lock();
        for i in 0..pages {
            inner.commit_page(start_page + i, MMUFlags::WRITE)?;
        }
        Ok(())
    }

    fn decommit(&self, offset: usize, len: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        // non-slice child VMOs do not support decommit.
        assert_ne!(inner.type_, VMOType::Hidden);
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

    fn set_user_id(&self, user_id: KoID) {
        self.inner.lock().page_attribution_user_id = user_id;
    }
}

impl VMObjectPagedInner {
    /// Commit a page recursively.
    fn commit_page(&mut self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr> {
        // check if it is in current frames list
        if let Some(frame) = self.frames.get(&page_idx) {
            return Ok(frame.frame.addr());
        }
        if self.parent_offset + page_idx * PAGE_SIZE >= self.parent_limit() {
            let new_frame = PageState::new(vmo_frame_alloc()?);
            let paddr = new_frame.frame.addr();
            self.frames.insert(page_idx, new_frame);
            return Ok(paddr);
        }
        if self.parent.is_none() {
            if !flags.contains(MMUFlags::WRITE) {
                return Ok(PhysFrame::zero_frame_addr());
            }
            let target = vmo_alloc_zero_frame()?;
            let paddr = target.addr();
            self.frames.insert(page_idx, PageState::new(target));
            return Ok(paddr);
        }
        let mut frame_owner: Arc<VMObjectPaged>;
        let mut current = self.parent.clone();
        let mut current_idx = page_idx + self.parent_offset / PAGE_SIZE;
        let mut dir_stack = Vec::<usize>::new();
        let mut is_marker = false;
        // 向上，寻找拥有对应frame的vmo。记录沿途的方向。
        loop {
            frame_owner = current.unwrap();
            let locked_cur = frame_owner.inner.lock();
            if locked_cur.frames.contains_key(&current_idx) {
                is_marker = true;
                break;
            }
            current_idx += locked_cur.parent_offset / PAGE_SIZE;
            assert!(current_idx < locked_cur.parent_limit() / PAGE_SIZE);
            current = locked_cur.parent.clone();
            if let Some(parent) = current.as_ref() {
                let locked_parent = parent.inner.lock();
                let item =
                    if Arc::ptr_eq(&locked_parent.children[0].upgrade().unwrap(), &frame_owner) {
                        0
                    } else {
                        1
                    };
                dir_stack.push(item);
            } else {
                unimplemented!() // should never be here
            }
            if current.is_none() {
                break;
            }
        }

        let res: PhysAddr;
        let current = frame_owner;

        // if we just need a read-only page
        if !flags.contains(MMUFlags::WRITE) {
            let owner = current.inner.lock();
            let frame = owner.frames.get(&current_idx).unwrap();
            if !is_marker {
                return Ok(frame.frame.addr());
            } else {
                // return zero page
                return Ok(PhysFrame::zero_frame_addr());
            }
        }
        if is_marker {
            let target = vmo_alloc_zero_frame()?;
            let paddr = target.addr();
            let new_frame = PageState::new(target);
            self.frames.insert(page_idx, new_frame);
            return Ok(paddr);
        }

        // 向下，逐层查看对应frame是否全局可见，如不可见则进行复制，然后向下分发。
        let mut parent = current.clone();
        let mut parent_idx = current_idx;
        let mut cur;
        let mut cur_idx = current_idx;
        while let Some(pos) = dir_stack.pop() {
            let mut locked_parent = parent.inner.lock();
            cur = locked_parent.children[pos].upgrade().unwrap();
            let mut locked_child = cur.inner.lock();
            cur_idx -= locked_child.parent_offset / PAGE_SIZE;
            let frame = locked_parent.frames.get_mut(&parent_idx).unwrap();
            if frame.tag != PageStateTag::Init {
                let mut item = locked_parent.frames.remove(&parent_idx).unwrap();
                item.tag = PageStateTag::Init;
                locked_child.frames.insert(cur_idx, item);
            } else {
                // copy a page from `frame`
                let paddr = frame.frame.addr();
                let new_frame = PageState::new(vmo_alloc_copy_frame(paddr)?);
                // set `frame` as splited
                frame.tag = if pos == 1 {
                    PageStateTag::RightSplit
                } else {
                    PageStateTag::LeftSplit
                };
                // insert the new page into locked_cur.frames
                locked_child.frames.insert(cur_idx, new_frame);
                // range_change on the other sub-tree
                let parent_offset = parent_idx * PAGE_SIZE;
                locked_parent.children[(pos + 1) % 2]
                    .upgrade()
                    .unwrap()
                    .inner
                    .lock()
                    .range_change(
                        parent_offset,
                        parent_offset + PAGE_SIZE,
                        RangeChangeOp::Unmap,
                    );
            }
            drop(locked_child);
            drop(locked_parent);
            parent = cur;
            parent_idx = cur_idx;
        }

        // 最终，当前vmo的父vmo必定持有对应的frame，从父vmo处拷贝或移动。
        let locked_parent = self.parent.as_ref().unwrap();
        let mut parent = locked_parent.inner.lock();
        let idx = page_idx + self.parent_offset / PAGE_SIZE;
        let frame = parent.frames.get_mut(&idx).unwrap();
        let new_frame = if frame.tag != PageStateTag::Init {
            let mut item = parent.frames.remove(&parent_idx).unwrap();
            item.tag = PageStateTag::Init;
            item
        } else {
            frame.tag = PageStateTag::LeftSplit;
            PageState::new(vmo_alloc_copy_frame(frame.frame.addr())?)
        };
        res = new_frame.frame.addr();
        self.frames.insert(page_idx, new_frame);
        Ok(res)
    }

    fn decommit(&mut self, page_idx: usize) {
        self.frames.remove(&page_idx);
    }

    fn range_change(&self, parent_offset: usize, parent_limit: usize, op: RangeChangeOp) {
        let mut start = self.parent_offset.max(parent_offset);
        let mut end = self.parent_limit().min(parent_limit);
        debug!(
            "range_change: {:#x} {:#x} {:#x} {:#x}",
            self.parent_offset,
            self.parent_limit(),
            start,
            end
        );
        if start >= end {
            return;
        } else {
            debug!("begin change range");
            start -= self.parent_offset;
            end -= self.parent_offset;
            self.mappings
                .iter()
                .for_each(|map| map.range_change(pages(start), pages(end), op));
            self.children.iter().for_each(|child| {
                child
                    .upgrade()
                    .unwrap()
                    .inner
                    .lock()
                    .range_change(start, end, op)
            });
        }
        debug!("range changed");
    }

    /// Count committed pages in the VMO and its ancestors.
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
            while let Some(locked_) = current {
                let locked_cur = locked_.inner.lock();
                if let Some(frame) = locked_cur.frames.get(&current_idx) {
                    if frame.tag != PageStateTag::Init
                        || locked_cur.page_attribution_user_id == self.page_attribution_user_id
                    {
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

    fn remove_child(&mut self, myself: &Weak<VMObjectPaged>, child: &Weak<VMObjectPaged>) {
        self.children
            .retain(|c| c.strong_count() != 0 && !c.ptr_eq(child));
        self.contract_hidden(myself);
    }

    /// Contract hidden node which has only 1 child.
    ///
    /// This is an optimization and is not necessary for functionality.
    ///
    /// ```text
    ///   |      |
    ///   H      |
    ///   |  =>  |
    ///   S      S
    /// ```
    fn contract_hidden(&mut self, myself: &Weak<VMObjectPaged>) {
        assert_eq!(self.type_, VMOType::Hidden);
        assert_eq!(self.children.len(), 1);

        let weak_child = self.children.remove(0);
        let locked_child = weak_child.upgrade().unwrap();
        let mut child = locked_child.inner.lock();
        let start = child.parent_offset / PAGE_SIZE;
        let end = child.parent_limit() / PAGE_SIZE;
        // merge nodes to the child
        for (key, value) in self.frames.split_off(&start) {
            if key >= end {
                break;
            }
            let idx = key - start;
            if !child.frames.contains_key(&idx) {
                child.frames.insert(idx, value);
            }
        }
        // connect child to my parent
        if let Some(parent) = &self.parent {
            for child in parent.inner.lock().children.iter_mut() {
                if Weak::ptr_eq(child, myself) {
                    *child = weak_child.clone();
                }
            }
        }
        child.parent = self.parent.take();
        child.parent_offset += self.parent_offset;
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
            page_attribution_user_id: self.page_attribution_user_id,
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
            map.range_change(pages(offset), pages(len), RangeChangeOp::RemoveWrite);
        }

        // create hidden_vmo's another child as result
        let child = VMObjectPaged::wrap(VMObjectPagedInner {
            type_: VMOType::Snapshot,
            page_attribution_user_id: 0,
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
        let vmo = VmObject::new_paged(10);
        vmo.write(0, &[1, 2, 3, 4]).unwrap();
        let mut buf = [0u8; 4];
        vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[1, 2, 3, 4]);
        let child_vmo = vmo.create_child(true, 0, 4 * 4096);
        child_vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.write(0, &[6, 7, 8, 9]).unwrap();
        vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[1, 2, 3, 4]);
        child_vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[6, 7, 8, 9]);
    }
}

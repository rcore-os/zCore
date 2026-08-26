//! B-tree engine: search, range iteration and in-place editing.
//!
//! Editing never copies-on-write: blocks are modified where they live and
//! their checksum recomputed, keeping all parent pointers and generations
//! valid. Structural changes (splits, new/freed blocks) go through the
//! in-memory allocator, which records the extent-tree bookkeeping to be
//! applied by the caller afterwards (see [`crate::alloc_ext`]).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::alloc_ext::FreeSpace;
use crate::structs::*;
use crate::volume::Volume;
use crate::{Error, Result};

/// Cached root pointers: tree id → (bytenr, level).
pub type RootCache = BTreeMap<u64, (u64, u8)>;

/// Borrowed editing context over a mounted volume.
pub struct Tree<'a> {
    pub vol: &'a mut Volume,
    pub roots: &'a mut RootCache,
    pub alloc: &'a mut FreeSpace,
}

/// One step of a root→leaf path.
#[derive(Debug, Clone, Copy)]
pub struct PathElem {
    pub bytenr: u64,
    pub slot: usize,
    pub level: u8,
}

enum DeleteOutcome {
    Done,
    /// The child block ended up empty and has been freed; remove its pointer.
    Empty,
}

impl<'a> Tree<'a> {
    // -- roots --------------------------------------------------------------

    /// Resolve the root (bytenr, level) of `tree`.
    pub fn root(&mut self, tree: u64) -> Result<(u64, u8)> {
        if let Some(&r) = self.roots.get(&tree) {
            return Ok(r);
        }
        let r = match tree {
            ROOT_TREE => (self.vol.sb.root(), self.vol.sb.root_level()),
            CHUNK_TREE => (self.vol.sb.chunk_root(), self.vol.sb.chunk_root_level()),
            _ => {
                let (key, data) = self
                    .find_first_in_range(
                        ROOT_TREE,
                        Key::new(tree, ROOT_ITEM_KEY, 0),
                        Key::new(tree, ROOT_ITEM_KEY, u64::MAX),
                    )?
                    .ok_or(Error::NotFound)?;
                let _ = key;
                let item = RootItem::parse(&data).ok_or(Error::Corrupt("root item"))?;
                (item.bytenr, item.level)
            }
        };
        self.roots.insert(tree, r);
        Ok(r)
    }

    /// Persist a new root pointer for `tree`.
    fn set_tree_root(&mut self, tree: u64, bytenr: u64, level: u8) -> Result<()> {
        self.roots.insert(tree, (bytenr, level));
        match tree {
            ROOT_TREE => {
                self.vol.sb.set_root(bytenr);
                self.vol.sb.set_root_level(level);
            }
            CHUNK_TREE => {
                self.vol.sb.set_chunk_root(bytenr);
                self.vol.sb.set_chunk_root_level(level);
            }
            _ => {
                let key = self
                    .find_first_in_range(
                        ROOT_TREE,
                        Key::new(tree, ROOT_ITEM_KEY, 0),
                        Key::new(tree, ROOT_ITEM_KEY, u64::MAX),
                    )?
                    .ok_or(Error::Corrupt("missing root item"))?
                    .0;
                self.update_in_place(ROOT_TREE, key, |data| {
                    put_u64(data, 176, bytenr);
                    data[238] = level;
                })?;
            }
        }
        Ok(())
    }

    // -- search -------------------------------------------------------------

    /// Descend from the root towards `key`. Returns the path (root..leaf) and
    /// whether the leaf contains `key` exactly; the leaf slot is the position
    /// of the match or the insertion point (possibly == nritems).
    pub fn search(&mut self, tree: u64, key: Key) -> Result<(Vec<PathElem>, bool)> {
        let (mut bytenr, mut level) = self.root(tree)?;
        let mut path = Vec::new();
        loop {
            let block = self.vol.read_block(bytenr)?;
            if header::level(&block) != level {
                return Err(Error::Corrupt("tree level mismatch"));
            }
            let n = header::nritems(&block) as usize;
            if level == 0 {
                // First slot with leaf_key >= key.
                let mut lo = 0usize;
                let mut hi = n;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if leaf::key(&block, mid) < key {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let exact = lo < n && leaf::key(&block, lo) == key;
                path.push(PathElem {
                    bytenr,
                    slot: lo,
                    level,
                });
                return Ok((path, exact));
            }
            // Last slot with node_key <= key (or 0).
            let mut lo = 0usize;
            let mut hi = n;
            while lo < hi {
                let mid = (lo + hi) / 2;
                if node::key(&block, mid) <= key {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let slot = lo.saturating_sub(1);
            path.push(PathElem {
                bytenr,
                slot,
                level,
            });
            bytenr = node::blockptr(&block, slot);
            level -= 1;
        }
    }

    /// Exact lookup; returns the item data.
    pub fn get(&mut self, tree: u64, key: Key) -> Result<Option<Vec<u8>>> {
        let (path, exact) = self.search(tree, key)?;
        if !exact {
            return Ok(None);
        }
        let last = path.last().unwrap();
        let block = self.vol.read_block(last.bytenr)?;
        Ok(Some(leaf::data(&block, last.slot).to_vec()))
    }

    /// First item with `lo <= key <= hi`, if any.
    pub fn find_first_in_range(
        &mut self,
        tree: u64,
        lo: Key,
        hi: Key,
    ) -> Result<Option<(Key, Vec<u8>)>> {
        let mut out = None;
        self.iter_from(tree, lo, |k, data| {
            if *k <= hi {
                out = Some((*k, data.to_vec()));
            }
            Ok(false)
        })?;
        Ok(out)
    }

    /// Iterate items with key >= `from` in order, until the callback returns
    /// `false` or the tree is exhausted. The tree must not be modified from
    /// inside the callback.
    pub fn iter_from<F>(&mut self, tree: u64, from: Key, mut f: F) -> Result<()>
    where
        F: FnMut(&Key, &[u8]) -> Result<bool>,
    {
        let (mut path, _) = self.search(tree, from)?;
        loop {
            let last = *path.last().unwrap();
            let block = self.vol.read_block(last.bytenr)?;
            let n = header::nritems(&block) as usize;
            for slot in last.slot..n {
                let key = leaf::key(&block, slot);
                if !f(&key, leaf::data(&block, slot))? {
                    return Ok(());
                }
            }
            drop(block);
            match self.next_leaf(path)? {
                Some(p) => path = p,
                None => return Ok(()),
            }
        }
    }

    /// Advance a search path to the first slot of the next leaf, or `None`
    /// when the current leaf is the rightmost one.
    fn next_leaf(&mut self, mut path: Vec<PathElem>) -> Result<Option<Vec<PathElem>>> {
        path.pop(); // leaf
        while let Some(mut elem) = path.pop() {
            let block = self.vol.read_block(elem.bytenr)?;
            let n = header::nritems(&block) as usize;
            if elem.slot + 1 >= n {
                continue;
            }
            elem.slot += 1;
            let mut bytenr = node::blockptr(&block, elem.slot);
            let mut level = elem.level - 1;
            drop(block);
            path.push(elem);
            loop {
                if level == 0 {
                    path.push(PathElem {
                        bytenr,
                        slot: 0,
                        level: 0,
                    });
                    return Ok(Some(path));
                }
                let b = self.vol.read_block(bytenr)?;
                path.push(PathElem {
                    bytenr,
                    slot: 0,
                    level,
                });
                bytenr = node::blockptr(&b, 0);
                level -= 1;
            }
        }
        Ok(None)
    }

    /// Greatest item with key <= `key`, if any.
    pub fn prev_item(&mut self, tree: u64, key: Key) -> Result<Option<(Key, Vec<u8>)>> {
        let (path, exact) = self.search(tree, key)?;
        let last = path.last().unwrap();
        let block = self.vol.read_block(last.bytenr)?;
        if exact {
            return Ok(Some((key, leaf::data(&block, last.slot).to_vec())));
        }
        if last.slot > 0 {
            let k = leaf::key(&block, last.slot - 1);
            return Ok(Some((k, leaf::data(&block, last.slot - 1).to_vec())));
        }
        // Walk to the rightmost item of the left sibling subtree.
        for elem in path.iter().rev().skip(1) {
            if elem.slot > 0 {
                let block = self.vol.read_block(elem.bytenr)?;
                let mut bytenr = node::blockptr(&block, elem.slot - 1);
                loop {
                    let b = self.vol.read_block(bytenr)?;
                    let n = header::nritems(&b) as usize;
                    if n == 0 {
                        return Ok(None);
                    }
                    if header::level(&b) == 0 {
                        let k = leaf::key(&b, n - 1);
                        return Ok(Some((k, leaf::data(&b, n - 1).to_vec())));
                    }
                    bytenr = node::blockptr(&b, n - 1);
                }
            }
        }
        Ok(None)
    }

    // -- block (re)construction ----------------------------------------------

    fn leaf_items(&mut self, bytenr: u64) -> Result<Vec<(Key, Vec<u8>)>> {
        let block = self.vol.read_block(bytenr)?;
        let n = header::nritems(&block) as usize;
        let mut items = Vec::with_capacity(n + 1);
        for slot in 0..n {
            items.push((leaf::key(&block, slot), leaf::data(&block, slot).to_vec()));
        }
        Ok(items)
    }

    fn node_ptrs(&mut self, bytenr: u64) -> Result<Vec<(Key, u64, u64)>> {
        let block = self.vol.read_block(bytenr)?;
        let n = header::nritems(&block) as usize;
        let mut ptrs = Vec::with_capacity(n + 1);
        for slot in 0..n {
            ptrs.push((
                node::key(&block, slot),
                node::blockptr(&block, slot),
                node::generation(&block, slot),
            ));
        }
        Ok(ptrs)
    }

    fn header_template(&self, tree: u64, bytenr: u64, generation: u64, level: u8) -> Vec<u8> {
        let mut b = alloc::vec![0u8; self.vol.nodesize];
        let fsid = self.vol.sb.fsid();
        b[header::OFF_FSID..header::OFF_FSID + 16].copy_from_slice(&fsid);
        b[header::OFF_CHUNK_TREE_UUID..header::OFF_CHUNK_TREE_UUID + 16]
            .copy_from_slice(&self.vol.chunk_tree_uuid);
        header::set_bytenr(&mut b, bytenr);
        header::set_flags(&mut b, HEADER_FLAG_WRITTEN | BACKREF_REV_MIXED);
        header::set_generation(&mut b, generation);
        header::set_owner(&mut b, tree);
        header::set_level(&mut b, level);
        b
    }

    fn write_leaf(
        &mut self,
        tree: u64,
        bytenr: u64,
        generation: u64,
        items: &[(Key, Vec<u8>)],
    ) -> Result<()> {
        let nodesize = self.vol.nodesize;
        let mut b = self.header_template(tree, bytenr, generation, 0);
        header::set_nritems(&mut b, items.len() as u32);
        let mut data_off = nodesize - HEADER_SIZE;
        for (slot, (key, data)) in items.iter().enumerate() {
            data_off = data_off
                .checked_sub(data.len())
                .ok_or(Error::Corrupt("leaf overflow"))?;
            leaf::set_key(&mut b, slot, key);
            leaf::set_data_off(&mut b, slot, data_off);
            leaf::set_data_size(&mut b, slot, data.len());
            b[HEADER_SIZE + data_off..HEADER_SIZE + data_off + data.len()].copy_from_slice(data);
        }
        if HEADER_SIZE + items.len() * ITEM_SIZE > HEADER_SIZE + data_off {
            return Err(Error::Corrupt("leaf overflow"));
        }
        self.vol.write_block(bytenr, b)
    }

    fn write_node(
        &mut self,
        tree: u64,
        bytenr: u64,
        generation: u64,
        level: u8,
        ptrs: &[(Key, u64, u64)],
    ) -> Result<()> {
        let mut b = self.header_template(tree, bytenr, generation, level);
        header::set_nritems(&mut b, ptrs.len() as u32);
        for (slot, (key, ptr, gen)) in ptrs.iter().enumerate() {
            node::set_key(&mut b, slot, key);
            node::set_blockptr(&mut b, slot, *ptr);
            node::set_generation(&mut b, slot, *gen);
        }
        self.vol.write_block(bytenr, b)
    }

    fn items_size(items: &[(Key, Vec<u8>)]) -> usize {
        items.iter().map(|(_, d)| ITEM_SIZE + d.len()).sum()
    }

    fn leaf_capacity(&self) -> usize {
        self.vol.nodesize - HEADER_SIZE
    }

    /// Owner-appropriate block-group flags for a tree's blocks.
    fn meta_flags(&self, tree: u64) -> u64 {
        if tree == CHUNK_TREE {
            BLOCK_GROUP_SYSTEM
        } else {
            BLOCK_GROUP_METADATA
        }
    }

    // -- insertion ------------------------------------------------------------

    /// Insert a new item; fails with `Exists` if the key is present.
    pub fn insert(&mut self, tree: u64, key: Key, data: &[u8]) -> Result<()> {
        if ITEM_SIZE + data.len() > self.leaf_capacity() {
            return Err(Error::Invalid);
        }
        let (root_bytenr, root_level) = self.root(tree)?;
        if let Some((split_key, split_ptr)) =
            self.insert_rec(tree, root_bytenr, root_level, key, data)?
        {
            // Grow the tree: new root with the old root and the new sibling.
            let old = self.vol.read_block(root_bytenr)?;
            let old_key0 = if root_level == 0 {
                leaf::key(&old, 0)
            } else {
                node::key(&old, 0)
            };
            let old_gen = header::generation(&old);
            drop(old);
            let gen = self.vol.sb.generation();
            let new_root =
                self.alloc
                    .alloc_tree_block(tree, root_level + 1, self.meta_flags(tree))?;
            self.write_node(
                tree,
                new_root,
                gen,
                root_level + 1,
                &[
                    (old_key0, root_bytenr, old_gen),
                    (split_key, split_ptr, gen),
                ],
            )?;
            self.set_tree_root(tree, new_root, root_level + 1)?;
        }
        Ok(())
    }

    /// Recursive insert; returns `Some((first_key, bytenr))` when the block at
    /// this level split and the new right sibling must be linked by the caller.
    fn insert_rec(
        &mut self,
        tree: u64,
        bytenr: u64,
        level: u8,
        key: Key,
        data: &[u8],
    ) -> Result<Option<(Key, u64)>> {
        if level == 0 {
            return self.leaf_insert(tree, bytenr, key, data);
        }
        let block = self.vol.read_block(bytenr)?;
        let n = header::nritems(&block) as usize;
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if node::key(&block, mid) <= key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let slot = lo.saturating_sub(1);
        let child = node::blockptr(&block, slot);
        drop(block);

        let split = self.insert_rec(tree, child, level - 1, key, data)?;

        // Refresh the separator key for the child (covers inserts at slot 0).
        let child_block = self.vol.read_block(child)?;
        let child_key0 = if level == 1 {
            leaf::key(&child_block, 0)
        } else {
            node::key(&child_block, 0)
        };
        drop(child_block);
        let mut ptrs = self.node_ptrs(bytenr)?;
        let generation = {
            let b = self.vol.read_block(bytenr)?;
            header::generation(&b)
        };
        ptrs[slot].0 = child_key0;

        let result = if let Some((new_key, new_ptr)) = split {
            ptrs.insert(slot + 1, (new_key, new_ptr, self.vol.sb.generation()));
            if ptrs.len() <= node::max_items(self.vol.nodesize) {
                None
            } else {
                let mid = ptrs.len() / 2;
                let right: Vec<_> = ptrs.split_off(mid);
                let right_key0 = right[0].0;
                let gen = self.vol.sb.generation();
                let right_bytenr =
                    self.alloc
                        .alloc_tree_block(tree, level, self.meta_flags(tree))?;
                self.write_node(tree, right_bytenr, gen, level, &right)?;
                Some((right_key0, right_bytenr))
            }
        } else {
            None
        };
        self.write_node(tree, bytenr, generation, level, &ptrs)?;
        Ok(result)
    }

    fn leaf_insert(
        &mut self,
        tree: u64,
        bytenr: u64,
        key: Key,
        data: &[u8],
    ) -> Result<Option<(Key, u64)>> {
        let mut items = self.leaf_items(bytenr)?;
        let generation = {
            let b = self.vol.read_block(bytenr)?;
            header::generation(&b)
        };
        let slot = match items.binary_search_by(|(k, _)| k.cmp(&key)) {
            Ok(_) => return Err(Error::Exists),
            Err(s) => s,
        };
        items.insert(slot, (key, data.to_vec()));
        let capacity = self.leaf_capacity();
        if Self::items_size(&items) <= capacity {
            self.write_leaf(tree, bytenr, generation, &items)?;
            return Ok(None);
        }
        // Split. Sequential append (slot at the end) keeps the left leaf full;
        // otherwise split around half of the byte usage.
        let split_at = if slot == items.len() - 1 {
            items.len() - 1
        } else {
            let total = Self::items_size(&items);
            let mut acc = 0usize;
            let mut at = items.len() - 1;
            for (i, (_, d)) in items.iter().enumerate() {
                acc += ITEM_SIZE + d.len();
                if acc > total / 2 {
                    at = (i + 1).min(items.len() - 1);
                    break;
                }
            }
            at.max(1)
        };
        let right: Vec<_> = items.split_off(split_at);
        if Self::items_size(&items) > capacity || Self::items_size(&right) > capacity {
            return Err(Error::Corrupt("unsplittable leaf"));
        }
        let right_key0 = right[0].0;
        let gen = self.vol.sb.generation();
        let right_bytenr = self
            .alloc
            .alloc_tree_block(tree, 0, self.meta_flags(tree))?;
        self.write_leaf(tree, right_bytenr, gen, &right)?;
        self.write_leaf(tree, bytenr, generation, &items)?;
        Ok(Some((right_key0, right_bytenr)))
    }

    // -- deletion -------------------------------------------------------------

    /// Delete the item with `key`; `NotFound` if absent.
    pub fn delete(&mut self, tree: u64, key: Key) -> Result<()> {
        let (root_bytenr, root_level) = self.root(tree)?;
        match self.delete_rec(tree, root_bytenr, root_level, key)? {
            DeleteOutcome::Empty if root_level > 0 => {
                // The root node lost all its children. Replace it with an
                // empty leaf so the tree stays valid.
                let gen = {
                    let b = self.vol.read_block(root_bytenr)?;
                    header::generation(&b)
                };
                self.write_leaf(tree, root_bytenr, gen, &[])?;
                self.set_tree_root(tree, root_bytenr, 0)?;
            }
            _ => {}
        }
        // Collapse a chain of single-child root nodes.
        loop {
            let (root_bytenr, root_level) = self.root(tree)?;
            if root_level == 0 {
                break;
            }
            let block = self.vol.read_block(root_bytenr)?;
            if header::nritems(&block) != 1 {
                break;
            }
            let child = node::blockptr(&block, 0);
            let child_level = root_level - 1;
            drop(block);
            self.set_tree_root(tree, child, child_level)?;
            self.alloc
                .free_tree_block(root_bytenr, tree, root_level, self.meta_flags(tree))?;
            self.vol.forget_block(root_bytenr);
        }
        Ok(())
    }

    fn delete_rec(&mut self, tree: u64, bytenr: u64, level: u8, key: Key) -> Result<DeleteOutcome> {
        if level == 0 {
            let mut items = self.leaf_items(bytenr)?;
            let generation = {
                let b = self.vol.read_block(bytenr)?;
                header::generation(&b)
            };
            let slot = items
                .binary_search_by(|(k, _)| k.cmp(&key))
                .map_err(|_| Error::NotFound)?;
            items.remove(slot);
            self.write_leaf(tree, bytenr, generation, &items)?;
            return Ok(if items.is_empty() {
                DeleteOutcome::Empty
            } else {
                DeleteOutcome::Done
            });
        }
        let block = self.vol.read_block(bytenr)?;
        let n = header::nritems(&block) as usize;
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if node::key(&block, mid) <= key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let slot = lo.saturating_sub(1);
        let child = node::blockptr(&block, slot);
        drop(block);

        let outcome = self.delete_rec(tree, child, level - 1, key)?;
        let mut ptrs = self.node_ptrs(bytenr)?;
        let generation = {
            let b = self.vol.read_block(bytenr)?;
            header::generation(&b)
        };
        match outcome {
            DeleteOutcome::Empty => {
                self.alloc
                    .free_tree_block(child, tree, level - 1, self.meta_flags(tree))?;
                self.vol.forget_block(child);
                ptrs.remove(slot);
            }
            DeleteOutcome::Done => {
                let child_block = self.vol.read_block(child)?;
                let child_key0 = if level == 1 {
                    leaf::key(&child_block, 0)
                } else {
                    node::key(&child_block, 0)
                };
                drop(child_block);
                ptrs[slot].0 = child_key0;
            }
        }
        self.write_node(tree, bytenr, generation, level, &ptrs)?;
        Ok(if ptrs.is_empty() {
            DeleteOutcome::Empty
        } else {
            DeleteOutcome::Done
        })
    }

    // -- updates --------------------------------------------------------------

    /// Modify an existing item's data in place (same size).
    pub fn update_in_place<F>(&mut self, tree: u64, key: Key, f: F) -> Result<()>
    where
        F: FnOnce(&mut [u8]),
    {
        let (path, exact) = self.search(tree, key)?;
        if !exact {
            return Err(Error::NotFound);
        }
        let last = path.last().unwrap();
        let block = self.vol.read_block(last.bytenr)?;
        let mut b = (*block).clone();
        drop(block);
        f(leaf::data_mut(&mut b, last.slot));
        self.vol.write_block(last.bytenr, b)
    }

    /// Replace an item's data, resizing as needed (delete+insert fallback when
    /// the leaf is full).
    pub fn set_item(&mut self, tree: u64, key: Key, data: &[u8]) -> Result<()> {
        let (path, exact) = self.search(tree, key)?;
        if !exact {
            return self.insert(tree, key, data);
        }
        let last = path.last().unwrap();
        let block = self.vol.read_block(last.bytenr)?;
        let old_size = leaf::data_size(&block, last.slot);
        if old_size == data.len() {
            let mut b = (*block).clone();
            drop(block);
            leaf::data_mut(&mut b, last.slot).copy_from_slice(data);
            return self.vol.write_block(last.bytenr, b);
        }
        let free = leaf::free_space(&block, self.vol.nodesize);
        drop(block);
        if data.len() <= old_size || data.len() - old_size <= free {
            // Rebuild this leaf with the new data.
            let bytenr = last.bytenr;
            let mut items = self.leaf_items(bytenr)?;
            let generation = {
                let b = self.vol.read_block(bytenr)?;
                header::generation(&b)
            };
            items[last.slot].1 = data.to_vec();
            return self.write_leaf(tree, bytenr, generation, &items);
        }
        self.delete(tree, key)?;
        self.insert(tree, key, data)
    }
}

//! Volume layer: superblock handling, chunk (logical→physical) mapping and
//! tree-block I/O with a small cache.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::device::BlockDevice;
use crate::structs::*;
use crate::{Error, Result};

/// One logical chunk and the physical stripes backing it. For SINGLE there is
/// one stripe; for DUP two (reads use the first, writes hit all of them).
#[derive(Debug, Clone)]
pub struct ChunkMapping {
    pub logical: u64,
    pub length: u64,
    pub type_: u64,
    pub stripes: Vec<Stripe>,
}

/// Cached tree block.
struct CachedBlock {
    data: Arc<Vec<u8>>,
    /// Monotonic counter for crude LRU eviction.
    last_use: u64,
    /// Written in memory but not yet flushed to the device (write-back). Dirty
    /// blocks are never evicted and are flushed (in `flush_dirty`) before the
    /// superblock is written, so on-disk metadata is always self-consistent.
    dirty: bool,
}

const CACHE_MAX_BLOCKS: usize = 256;

pub struct Volume {
    pub dev: Arc<dyn BlockDevice>,
    pub sb: Superblock,
    pub nodesize: usize,
    pub sectorsize: usize,
    /// Sorted by logical start.
    chunks: Vec<ChunkMapping>,
    /// chunk_tree_uuid taken from the chunk-root header (needed when
    /// initializing new tree blocks).
    pub chunk_tree_uuid: [u8; 16],
    cache: BTreeMap<u64, CachedBlock>,
    cache_tick: u64,
    /// Bumped on every device write. The fs read-side cache (inode + extents)
    /// keys off this so any mutation transparently invalidates it, without
    /// having to track every individual write site.
    write_epoch: core::sync::atomic::AtomicU64,
}

impl Volume {
    /// Monotonic counter incremented on each device write.
    pub fn write_epoch(&self) -> u64 {
        self.write_epoch.load(core::sync::atomic::Ordering::Relaxed)
    }
    #[inline]
    fn bump_write_epoch(&self) {
        self.write_epoch
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

impl Volume {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let mut raw = alloc::vec![0u8; SUPERBLOCK_SIZE];
        dev.read_at(SUPERBLOCK_OFFSETS[0], &mut raw)?;
        let sb = Superblock::parse(raw).ok_or(Error::BadSuperblock)?;
        if sb.csum_type() != 0 {
            return Err(Error::Unsupported("csum type != crc32c"));
        }
        if sb.incompat_flags() & !INCOMPAT_SUPPORTED != 0 {
            return Err(Error::Unsupported("unknown incompat flags"));
        }
        if sb.num_devices() != 1 {
            return Err(Error::Unsupported("multi-device filesystems"));
        }
        if sb.log_root() != 0 {
            // A dirty log tree means an unclean shutdown from Linux; replay is
            // not supported, but ignoring it only loses the logged updates.
            warn!("btrfs: ignoring non-empty log tree");
        }
        let nodesize = sb.nodesize() as usize;
        let sectorsize = sb.sectorsize() as usize;
        if !(512..=65536).contains(&nodesize) || !(512..=65536).contains(&sectorsize) {
            return Err(Error::Corrupt("bad node/sector size"));
        }
        let mut vol = Self {
            dev,
            sb,
            nodesize,
            sectorsize,
            chunks: Vec::new(),
            chunk_tree_uuid: [0u8; 16],
            cache: BTreeMap::new(),
            cache_tick: 0,
            write_epoch: core::sync::atomic::AtomicU64::new(0),
        };
        vol.bootstrap_chunks()?;
        vol.load_chunk_tree()?;
        Ok(vol)
    }

    /// Parse the superblock's sys_chunk_array (SYSTEM chunks, enough to read
    /// the chunk tree).
    fn bootstrap_chunks(&mut self) -> Result<()> {
        let arr = self.sb.sys_chunk_array().to_vec();
        let mut off = 0usize;
        while off + KEY_SIZE + CHUNK_ITEM_HDR_LEN <= arr.len() {
            let key = Key::read(&arr, off);
            off += KEY_SIZE;
            if key.item_type != CHUNK_ITEM_KEY {
                return Err(Error::Corrupt("sys_chunk_array entry"));
            }
            let chunk = ChunkItem::parse(&arr[off..]).ok_or(Error::Corrupt("sys chunk"))?;
            let len = CHUNK_ITEM_HDR_LEN + chunk.stripes.len() * STRIPE_LEN;
            self.add_chunk(key.offset, &chunk)?;
            off += len;
        }
        Ok(())
    }

    fn add_chunk(&mut self, logical: u64, chunk: &ChunkItem) -> Result<()> {
        let raid = chunk.type_ & BLOCK_GROUP_RAID_MASK;
        if raid != 0 && raid != BLOCK_GROUP_DUP {
            return Err(Error::Unsupported("RAID profile"));
        }
        let mapping = ChunkMapping {
            logical,
            length: chunk.length,
            type_: chunk.type_,
            stripes: chunk.stripes.clone(),
        };
        match self.chunks.binary_search_by_key(&logical, |c| c.logical) {
            Ok(i) => self.chunks[i] = mapping,
            Err(i) => self.chunks.insert(i, mapping),
        }
        Ok(())
    }

    /// Register a freshly created chunk (used by the allocator).
    pub fn register_chunk(&mut self, logical: u64, chunk: &ChunkItem) -> Result<()> {
        self.add_chunk(logical, chunk)
    }

    /// Walk the chunk tree and load every CHUNK_ITEM. Also records the
    /// chunk_tree_uuid from the chunk-root header.
    fn load_chunk_tree(&mut self) -> Result<()> {
        let root = self.sb.chunk_root();
        let root_block = self.read_block(root)?;
        self.chunk_tree_uuid = root_block
            [header::OFF_CHUNK_TREE_UUID..header::OFF_CHUNK_TREE_UUID + 16]
            .try_into()
            .unwrap();
        self.load_chunk_node(root)
    }

    fn load_chunk_node(&mut self, logical: u64) -> Result<()> {
        let block = self.read_block(logical)?;
        let level = header::level(&block);
        let n = header::nritems(&block) as usize;
        if level > 0 {
            let mut children = Vec::with_capacity(n);
            for slot in 0..n {
                children.push(node::blockptr(&block, slot));
            }
            drop(block);
            for child in children {
                self.load_chunk_node(child)?;
            }
        } else {
            let mut found = Vec::new();
            for slot in 0..n {
                let key = leaf::key(&block, slot);
                if key.item_type == CHUNK_ITEM_KEY {
                    let chunk = ChunkItem::parse(leaf::data(&block, slot))
                        .ok_or(Error::Corrupt("chunk item"))?;
                    found.push((key.offset, chunk));
                }
            }
            drop(block);
            for (logical, chunk) in found {
                self.add_chunk(logical, &chunk)?;
            }
        }
        Ok(())
    }

    /// Map a logical range to physical stripes. Returns (stripes, length
    /// available at this logical offset within the chunk).
    pub fn map_logical(&self, logical: u64, len: u64) -> Result<(Vec<u64>, u64)> {
        let idx = match self.chunks.binary_search_by_key(&logical, |c| c.logical) {
            Ok(i) => i,
            Err(0) => {
                warn!(
                    "btrfs: map_logical {:#x} before first chunk (lowest chunk {:#x})",
                    logical,
                    self.chunks.first().map(|c| c.logical).unwrap_or(0),
                );
                return Err(Error::Corrupt("logical address before first chunk"));
            }
            Err(i) => i - 1,
        };
        let chunk = &self.chunks[idx];
        if logical >= chunk.logical + chunk.length {
            warn!(
                "btrfs: map_logical {:#x} in chunk hole (chunk [{:#x},{:#x}), next {:?})",
                logical,
                chunk.logical,
                chunk.logical + chunk.length,
                self.chunks.get(idx + 1).map(|c| c.logical),
            );
            return Err(Error::Corrupt("logical address in chunk hole"));
        }
        let within = logical - chunk.logical;
        let avail = (chunk.length - within).min(len);
        let phys = chunk.stripes.iter().map(|s| s.offset + within).collect();
        Ok((phys, avail))
    }

    pub fn chunks(&self) -> &[ChunkMapping] {
        &self.chunks
    }

    /// Read bytes at a logical address (no caching; used for file data).
    pub fn read_logical(&self, logical: u64, buf: &mut [u8]) -> Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let (phys, avail) =
                self.map_logical(logical + done as u64, (buf.len() - done) as u64)?;
            let take = avail as usize;
            self.dev.read_at(phys[0], &mut buf[done..done + take])?;
            done += take;
        }
        Ok(())
    }

    /// Write bytes at a logical address, hitting every stripe (DUP).
    pub fn write_logical(&self, logical: u64, buf: &[u8]) -> Result<()> {
        self.bump_write_epoch();
        let mut done = 0usize;
        while done < buf.len() {
            let (phys, avail) =
                self.map_logical(logical + done as u64, (buf.len() - done) as u64)?;
            let take = avail as usize;
            for p in phys {
                self.dev.write_at(p, &buf[done..done + take])?;
            }
            done += take;
        }
        Ok(())
    }

    fn cache_evict_if_needed(&mut self) {
        if self.cache.len() <= CACHE_MAX_BLOCKS {
            return;
        }
        // Drop the least recently used half — but never a dirty (unflushed)
        // block, or its write-back data would be lost. Dirty blocks are bounded
        // by the live tree size between syncs.
        let mut uses: Vec<u64> = self
            .cache
            .values()
            .filter(|c| !c.dirty)
            .map(|c| c.last_use)
            .collect();
        if uses.is_empty() {
            return;
        }
        uses.sort_unstable();
        let cutoff = uses[uses.len() / 2];
        self.cache.retain(|_, c| c.dirty || c.last_use > cutoff);
    }

    /// Read a tree block (cached).
    pub fn read_block(&mut self, logical: u64) -> Result<Arc<Vec<u8>>> {
        self.cache_tick += 1;
        let tick = self.cache_tick;
        if let Some(c) = self.cache.get_mut(&logical) {
            c.last_use = tick;
            return Ok(c.data.clone());
        }
        let mut data = alloc::vec![0u8; self.nodesize];
        self.read_logical(logical, &mut data)?;
        if header::bytenr(&data) != logical {
            return Err(Error::Corrupt("tree block bytenr mismatch"));
        }
        if !header::csum_ok(&data) {
            return Err(Error::Corrupt("tree block checksum"));
        }
        let data = Arc::new(data);
        self.cache.insert(
            logical,
            CachedBlock {
                data: data.clone(),
                last_use: tick,
                dirty: false,
            },
        );
        self.cache_evict_if_needed();
        Ok(data)
    }

    /// Write a tree block: recompute its checksum, update the cache and the
    /// device (all stripes).
    pub fn write_block(&mut self, logical: u64, mut data: Vec<u8>) -> Result<()> {
        debug_assert_eq!(data.len(), self.nodesize);
        header::update_csum(&mut data);
        // Write-back: keep the block in the cache marked dirty instead of
        // writing it to the device now. CoW means a tree block modified many
        // times between syncs is re-allocated (old one `forget_block`-ed), so
        // only the live version is dirty; `flush_dirty` writes each once at
        // commit time. This collapses the per-write metadata write storm.
        // The mutation still bumps the write epoch (it would have via
        // `write_logical`) so the fs read cache is invalidated.
        self.bump_write_epoch();
        self.cache_tick += 1;
        let tick = self.cache_tick;
        self.cache.insert(
            logical,
            CachedBlock {
                data: Arc::new(data),
                last_use: tick,
                dirty: true,
            },
        );
        self.cache_evict_if_needed();
        Ok(())
    }

    /// Flush every dirty (write-back) tree block to the device. Must be called
    /// before writing the superblock so it never references unwritten blocks.
    pub fn flush_dirty(&mut self) -> Result<()> {
        // Collect first to avoid holding an immutable borrow over write_logical.
        let dirty: Vec<(u64, Arc<Vec<u8>>)> = self
            .cache
            .iter()
            .filter(|(_, b)| b.dirty)
            .map(|(&l, b)| (l, b.data.clone()))
            .collect();
        for (logical, data) in dirty {
            self.write_logical(logical, &data)?;
            if let Some(b) = self.cache.get_mut(&logical) {
                b.dirty = false;
            }
        }
        Ok(())
    }

    /// Forget a cached block (after freeing it).
    pub fn forget_block(&mut self, logical: u64) {
        self.cache.remove(&logical);
    }

    /// Flush the (already csum-updated) superblock to every mirror that fits
    /// on the device.
    pub fn write_superblock(&mut self) -> Result<()> {
        self.sb.update_csum();
        let total = self.dev.size();
        for &off in SUPERBLOCK_OFFSETS.iter() {
            if off + SUPERBLOCK_SIZE as u64 <= total {
                // Mirrors must reflect their own bytenr.
                let mut copy = self.sb.raw.clone();
                put_u64(&mut copy, sb::OFF_BYTENR, off);
                let sum = crate::crc::checksum(&copy[CSUM_SIZE..]);
                copy[..CSUM_SIZE].fill(0);
                put_u32(&mut copy, 0, sum);
                self.dev.write_at(off, &copy)?;
            }
        }
        Ok(())
    }
}

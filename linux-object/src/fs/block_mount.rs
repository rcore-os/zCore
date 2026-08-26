//! Block-backed and file-backed devices for mountable filesystems.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::cmp::{max, min};

use lock::Mutex;
use rcore_fs::dev::{DevError, Device, Result as DevResult};
use rcore_fs::vfs::{FsError, INode, Result as VfsResult};
use zcore_drivers::{scheme::BlockScheme, DeviceResult};

use super::devfs::BlockDev;

/// 512-byte sector buffer aligned for AHCI DMA.
#[repr(align(4096))]
struct SectorBuf([u8; 512]);

const SECTOR: usize = 512;

/// Backing store for a mount operation.
pub enum MountBackend {
    Block(Arc<dyn BlockScheme>),
    File(Arc<dyn INode>),
}

impl MountBackend {
    pub fn from_inode(inode: Arc<dyn INode>) -> VfsResult<Self> {
        use rcore_fs::vfs::FileType;
        let ty = inode.metadata()?.type_;
        match ty {
            FileType::BlockDevice => {
                let dev = inode
                    .as_any_ref()
                    .downcast_ref::<BlockDev>()
                    .ok_or(FsError::NotSupported)?;
                Ok(Self::Block(dev.block_scheme()))
            }
            FileType::File => Ok(Self::File(inode)),
            _ => Err(FsError::NotSupported),
        }
    }
}

/// Byte-oriented device over a block driver.
///
/// Deliberately CACHE-FREE: in production this device exists only underneath
/// [`CachedDevice`] (see [`device_from_backend`]), whose sector cache fully
/// shadows anything cached here. The old internal `SectorCache` meant every
/// cold sector was boxed, BTreeMap-inserted and copied TWICE (once per layer)
/// — ~4,100 extra heap boxes and ~8,000 BTreeMap operations per cold MiB, all
/// inside IRQ-off filesystem lock sections, for a cache that could never be
/// hit (the layer above absorbs every repeat).
pub struct BlockByteDevice {
    block: Arc<dyn BlockScheme>,
}

impl BlockByteDevice {
    pub fn new(block: Arc<dyn BlockScheme>) -> Self {
        Self { block }
    }

    /// Read `buf.len() / 512` consecutive sectors starting at `block_id` in one
    /// device transfer. `buf.len()` must be a non-zero multiple of 512.
    fn read_sectors(&self, block_id: usize, buf: &mut [u8]) -> DeviceResult {
        self.block.read_block(block_id, buf)
    }

    /// Write `buf.len() / 512` consecutive sectors starting at `block_id` in
    /// one device transfer. `buf.len()` must be a non-zero multiple of 512.
    fn write_sectors(&self, block_id: usize, buf: &[u8]) -> DeviceResult {
        self.block.write_block(block_id, buf)
    }
}

impl Device for BlockByteDevice {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> DevResult<usize> {
        const BS: usize = 512;
        let mut temp = SectorBuf([0u8; 512]);
        let mut done = 0usize;

        // Partial leading sector.
        let head_off = offset % BS;
        if head_off != 0 && done < buf.len() {
            let take = min(buf.len(), BS - head_off);
            self.read_sectors(offset / BS, &mut temp.0)
                .map_err(|_| DevError)?;
            buf[..take].copy_from_slice(&temp.0[head_off..head_off + take]);
            done += take;
        }

        // Whole-sector middle: a single multi-sector transfer.
        let mid = ((buf.len() - done) / BS) * BS;
        if mid > 0 {
            let block_id = (offset + done) / BS;
            self.read_sectors(block_id, &mut buf[done..done + mid])
                .map_err(|e| {
                    warn!(
                        "blockdev: read_block(block_id={}, {} sectors) failed: {:?} \
                         (byte_off={:#x}, block_count={})",
                        block_id,
                        mid / BS,
                        e,
                        offset + done,
                        self.block.block_count(),
                    );
                    DevError
                })?;
            done += mid;
        }

        // Partial trailing sector.
        if done < buf.len() {
            let take = buf.len() - done;
            self.read_sectors((offset + done) / BS, &mut temp.0)
                .map_err(|_| DevError)?;
            buf[done..].copy_from_slice(&temp.0[..take]);
            done += take;
        }

        Ok(done)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> DevResult<usize> {
        const BS: usize = 512;
        let mut temp = SectorBuf([0u8; 512]);
        let mut done = 0usize;

        // Partial leading sector: read-modify-write.
        let head_off = offset % BS;
        if head_off != 0 && done < buf.len() {
            let take = min(buf.len(), BS - head_off);
            let block_id = offset / BS;
            self.read_sectors(block_id, &mut temp.0)
                .map_err(|_| DevError)?;
            temp.0[head_off..head_off + take].copy_from_slice(&buf[..take]);
            self.write_sectors(block_id, &temp.0)
                .map_err(|_| DevError)?;
            done += take;
        }

        // Whole-sector middle: a single multi-sector transfer.
        let mid = ((buf.len() - done) / BS) * BS;
        if mid > 0 {
            let block_id = (offset + done) / BS;
            self.write_sectors(block_id, &buf[done..done + mid])
                .map_err(|e| {
                    warn!(
                        "blockdev: write_block(block_id={}, {} sectors) failed: {:?} \
                         (byte_off={:#x}, block_count={})",
                        block_id,
                        mid / BS,
                        e,
                        offset + done,
                        self.block.block_count(),
                    );
                    DevError
                })?;
            done += mid;
        }

        // Partial trailing sector: read-modify-write.
        if done < buf.len() {
            let take = buf.len() - done;
            let block_id = (offset + done) / BS;
            self.read_sectors(block_id, &mut temp.0)
                .map_err(|_| DevError)?;
            temp.0[..take].copy_from_slice(&buf[done..]);
            self.write_sectors(block_id, &temp.0)
                .map_err(|_| DevError)?;
            done += take;
        }

        Ok(done)
    }

    fn sync(&self) -> DevResult<()> {
        self.block.flush().map_err(|_| DevError)
    }
}

/// Byte-oriented device over a regular file inode (loop-less loop mounts).
pub struct FileByteDevice {
    inode: Arc<dyn INode>,
    len: usize,
}

impl FileByteDevice {
    pub fn new(inode: Arc<dyn INode>) -> VfsResult<Self> {
        Ok(Self {
            len: inode.metadata()?.size,
            inode,
        })
    }
}

impl Device for FileByteDevice {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> DevResult<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let take = min(buf.len(), self.len - offset);
        self.inode
            .read_at(offset, &mut buf[..take])
            .map_err(|_| DevError)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> DevResult<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let take = min(buf.len(), self.len - offset);
        self.inode
            .write_at(offset, &buf[..take])
            .map_err(|_| DevError)
    }

    fn sync(&self) -> DevResult<()> {
        self.inode.sync_all().map_err(|_| DevError)
    }
}

/// Per-device buffer-cache budget. Entries are allocated lazily as sectors are
/// touched, so a freshly-mounted device uses nothing and grows up to this cap.
/// 8 MiB is generous enough to keep the hot working set (inode table, block
/// group descriptors, directory blocks, recently-read file data) resident while
/// staying well under the per-mount budget the exec/VMO cache already spends.
const CACHE_CAPACITY_SECTORS: usize = 64 * 1024 * 1024 / SECTOR;

/// Forward read-ahead window. On a cache miss for a request smaller than this,
/// the backing device is read this far past the request in one command and the
/// extra sectors are cached. Sequential access — the dominant pattern during
/// boot (init/service files) and demand-paging of executables/libraries —
/// then hits the cache instead of issuing one synchronous device command per
/// piece, which is the dominant cost on high-latency media (USB/SATA, polled
/// AHCI). Sized to roughly one AHCI command so the prefetch is ~free latency-wise.
const READAHEAD_BYTES: usize = 1024 * 1024;

/// One cached 512-byte sector plus its LRU timestamp.
struct CacheLine {
    data: Box<[u8; SECTOR]>,
    /// Monotonic tick of the last access; the smallest tick is the LRU victim.
    tick: u64,
}

/// A write-through, sector-granular LRU buffer cache.
///
/// Every on-disk filesystem in the kernel (ext2 via `Synced`/editor, btrfs via
/// the device adapter) funnels *all* of its reads and writes through a single
/// `Arc<dyn Device>`. Without a cache each `read_at` reaches the AHCI/virtio
/// driver, so a workload re-reads the same inode-table sector on every `stat`,
/// re-scans directory blocks on every path lookup, and re-fetches file data on
/// every re-open. This decorator keeps recently-touched sectors in RAM:
///
/// * **Reads** are served from the cache when every sector of the request is
///   resident (the common case for hot metadata and re-read data); otherwise a
///   single backing read of the sector-aligned span fills both the caller's
///   buffer and the cache.
/// * **Writes** go straight through to the backing device first (so a crash can
///   never lose acknowledged data) and then update the cache so a subsequent
///   read never observes a stale sector.
///
/// Correctness rests on one invariant: a sector present in the cache always
/// holds exactly what is on disk. Write-through plus the post-write update below
/// maintains it. This assumes the wrapped device is reached *only* through this
/// cache (the normal one-`CachedDevice`-per-mount case); it is not a coherent
/// cache across two independent handles to the same disk.
struct BlockCache {
    /// Resident sectors keyed by sector id.
    lines: BTreeMap<u64, CacheLine>,
    /// LRU index: tick -> sector id. The first entry is the eviction victim.
    lru: BTreeMap<u64, u64>,
    /// Monotonic access counter feeding `CacheLine::tick`.
    clock: u64,
    /// Maximum number of resident sectors.
    capacity: usize,
}

impl BlockCache {
    fn new(capacity: usize) -> Self {
        Self {
            lines: BTreeMap::new(),
            lru: BTreeMap::new(),
            clock: 1,
            capacity: capacity.max(1),
        }
    }

    fn contains(&self, sector: u64) -> bool {
        self.lines.contains_key(&sector)
    }

    /// Hand out the next monotonic access tick. Kept tiny and borrow-free so
    /// callers can take it *before* borrowing `lines`, never holding a `lines`
    /// borrow across the `lru` bookkeeping below.
    fn next_tick(&mut self) -> u64 {
        let t = self.clock;
        self.clock = self.clock.wrapping_add(1);
        t
    }

    /// Copy `dst.len()` bytes starting at `off` within `sector` out of the
    /// cache, marking the sector most-recently-used. Returns `false` on a miss.
    fn read_into(&mut self, sector: u64, off: usize, dst: &mut [u8]) -> bool {
        let new_tick = self.next_tick();
        let old = match self.lines.get_mut(&sector) {
            Some(line) => {
                dst.copy_from_slice(&line.data[off..off + dst.len()]);
                let old = line.tick;
                line.tick = new_tick;
                old
            }
            None => return false,
        };
        // `lines` borrow released; update the LRU index.
        self.lru.remove(&old);
        self.lru.insert(new_tick, sector);
        true
    }

    /// Insert or replace a full sector, evicting the LRU victim(s) if at
    /// capacity. `data` must be exactly one sector.
    fn insert(&mut self, sector: u64, data: &[u8]) {
        debug_assert_eq!(data.len(), SECTOR);
        let new_tick = self.next_tick();
        // Update in place if the sector is already resident.
        let mut old_tick: Option<u64> = None;
        if let Some(line) = self.lines.get_mut(&sector) {
            old_tick = Some(line.tick);
            line.data.copy_from_slice(data);
            line.tick = new_tick;
        }
        if let Some(old) = old_tick {
            self.lru.remove(&old);
            self.lru.insert(new_tick, sector);
            return;
        }
        // Fresh sector: evict the least-recently-used until under capacity.
        while self.lines.len() >= self.capacity {
            let victim_tick = match self.lru.keys().next().copied() {
                Some(t) => t,
                None => break,
            };
            if let Some(victim_sector) = self.lru.remove(&victim_tick) {
                self.lines.remove(&victim_sector);
            }
        }
        let mut boxed = Box::new([0u8; SECTOR]);
        boxed.copy_from_slice(data);
        self.lines.insert(
            sector,
            CacheLine {
                data: boxed,
                tick: new_tick,
            },
        );
        self.lru.insert(new_tick, sector);
    }

    /// Patch `src` into a *resident* sector at byte offset `off`. No-op if the
    /// sector is not cached: write-through already put the bytes on disk, so the
    /// next read will fetch them fresh.
    fn patch(&mut self, sector: u64, off: usize, src: &[u8]) {
        if let Some(line) = self.lines.get_mut(&sector) {
            line.data[off..off + src.len()].copy_from_slice(src);
        }
    }
}

/// A [`Device`] decorator adding the [`BlockCache`] above to any backing device.
pub struct CachedDevice {
    inner: Arc<dyn Device>,
    cache: Mutex<BlockCache>,
    /// One past the last addressable byte, rounded up to a sector. Read-ahead is
    /// clamped to this so a prefetch never runs off the end of the device.
    dev_end: usize,
    /// A small ring of independent sequential-read streams for read-ahead
    /// detection. One shared "last read end" is not enough: btrfs interleaves a
    /// btree-node metadata read between consecutive file-data reads, so a single
    /// flag ping-pongs between the data offset and the far-away metadata offset
    /// and the byte-exact continuation test never holds during streaming —
    /// read-ahead simply never fires, which is why sequential btrfs read sat at
    /// ~10 MB/s while raw device read does 108. With per-stream state the data
    /// stream survives the metadata detour (it lives in its own slot), so a
    /// pure streaming read is recognised and collapses into one big command per
    /// window, while a random walk still matches no stream and prefetches
    /// nothing (preserving the random-4K win). Behind a `Mutex` because the
    /// critical section is a 4-entry scan and btrfs already serialises device
    /// I/O behind its own lock; never held across the backing read.
    streams: Mutex<StreamRing>,
}

/// Read-ahead stream tracker: `NUM_STREAMS` most-recent sequential streams.
struct StreamRing {
    slots: [Stream; Self::NUM_STREAMS],
    clock: u64,
}

#[derive(Clone, Copy)]
struct Stream {
    valid: bool,
    /// Device offset one past this stream's most recent backing read.
    last_end: usize,
    /// Exact-continuation count; prefetch only once a stream has proven itself
    /// (>= 1), so a brand-new stream — including every isolated random read —
    /// never triggers the window that would thrash the cache.
    confidence: u32,
    /// LRU tick for victim selection.
    lru: u64,
}

impl StreamRing {
    /// 8, not 4: session bring-up demand-pages several libraries from several
    /// processes at once, plus the btrfs metadata stream — with only 4 slots
    /// the interleaved sequential streams evicted each other, read-ahead
    /// stopped firing, and reads degraded to one 4 KiB command each.
    const NUM_STREAMS: usize = 8;
    const CONF_THRESH: u32 = 1;

    fn new() -> Self {
        StreamRing {
            slots: [Stream {
                valid: false,
                last_end: 0,
                confidence: 0,
                lru: 0,
            }; Self::NUM_STREAMS],
            clock: 0,
        }
    }

    /// Decide the read span for a request whose sector-aligned bounds are
    /// `[aligned_start, aligned_end)`. Returns `(read_end, slot)`: `read_end`
    /// is `aligned_end` for non-prefetch, or extended by `window` for a proven
    /// sequential stream. `slot` receives the corrected end after the read.
    fn plan(
        &mut self,
        aligned_start: usize,
        aligned_end: usize,
        buf_len: usize,
        window: usize,
        dev_end: usize,
    ) -> (usize, usize) {
        self.clock += 1;
        let tick = self.clock;
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.valid && s.last_end == aligned_start)
        {
            let s = &mut self.slots[i];
            s.confidence = s.confidence.saturating_add(1);
            s.lru = tick;
            let read_end = if buf_len < window && s.confidence >= Self::CONF_THRESH {
                (aligned_end + window).min(dev_end).max(aligned_end)
            } else {
                aligned_end
            };
            s.last_end = read_end; // predicted; corrected in `commit`
            (read_end, i)
        } else {
            // No stream continues here: start a fresh low-confidence one and
            // prefetch nothing. Victim = an invalid slot, else the LRU.
            let v = (0..Self::NUM_STREAMS)
                .min_by_key(|&k| (self.slots[k].valid, self.slots[k].lru))
                .unwrap();
            self.slots[v] = Stream {
                valid: true,
                last_end: aligned_end,
                confidence: 0,
                lru: tick,
            };
            (aligned_end, v)
        }
    }

    /// Correct a stream's end to the bytes actually transferred (matters only at
    /// end-of-device, where the backing read returns short of the plan).
    fn commit(&mut self, slot: usize, actual_end: usize) {
        self.slots[slot].last_end = actual_end;
    }
}

impl CachedDevice {
    pub fn new(inner: Arc<dyn Device>, size_bytes: usize) -> Self {
        Self::with_capacity(inner, size_bytes, CACHE_CAPACITY_SECTORS)
    }

    fn with_capacity(inner: Arc<dyn Device>, size_bytes: usize, capacity_sectors: usize) -> Self {
        let dev_end = size_bytes.div_ceil(SECTOR) * SECTOR;
        Self {
            inner,
            cache: Mutex::new(BlockCache::new(capacity_sectors)),
            dev_end,
            streams: Mutex::new(StreamRing::new()),
        }
    }

    /// Fill the cache from a sector-aligned buffer that was just read from the
    /// backing device. Only fully-read sectors (`valid_len` may be short at
    /// end-of-device) are inserted.
    fn populate(&self, first_sector: u64, data: &[u8], valid_len: usize) {
        let mut cache = self.cache.lock();
        let mut off = 0;
        let mut sector = first_sector;
        while off + SECTOR <= valid_len {
            cache.insert(sector, &data[off..off + SECTOR]);
            off += SECTOR;
            sector += 1;
        }
    }
}

impl Device for CachedDevice {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> DevResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset + buf.len();
        let first = (offset / SECTOR) as u64;
        let last = ((end - 1) / SECTOR) as u64;

        // Fast path: every sector is resident -> serve entirely from RAM with no
        // device I/O. This is what turns repeated stat/lookup/re-read traffic
        // into pure memory copies.
        {
            let mut cache = self.cache.lock();
            if (first..=last).all(|s| cache.contains(s)) {
                let mut pos = offset;
                let mut done = 0;
                while pos < end {
                    let sector = (pos / SECTOR) as u64;
                    let soff = pos % SECTOR;
                    let take = min(SECTOR - soff, end - pos);
                    let hit = cache.read_into(sector, soff, &mut buf[done..done + take]);
                    debug_assert!(hit);
                    done += take;
                    pos += take;
                }
                return Ok(done);
            }
        }
        // Lock released before touching the (slow) backing device.

        let aligned_start = (first as usize) * SECTOR;
        let aligned_end = (last as usize + 1) * SECTOR;

        // Read-ahead, but ONLY when this request continues where the last one
        // ended. Sequential streams (boot files, demand-paged libraries, a
        // large file read straight through) then collapse into one big command
        // per window; random access (a btree walk, scattered 4 KiB reads)
        // fetches just what it asked for and leaves the cache full of the
        // metadata it will re-touch, instead of a window's worth of neighbours
        // it never will. Applying the window unconditionally was measured to
        // barely move random 4 KiB reads (a 1 MiB prefetch per 4 KiB op evicts
        // the very metadata the next op needs) while helping sequential; gating
        // it on sequentiality keeps the sequential win and stops the random
        // thrash.
        // Read-ahead only for a PROVEN sequential stream (byte-exact
        // continuation, tracked per-stream so btrfs's interleaved metadata reads
        // do not break the data stream's continuity — see `StreamRing`). A
        // random walk matches no stream and prefetches nothing, preserving the
        // random-4K win; a streaming read matches its stream and extends by the
        // window, collapsing hundreds of 4 KiB reads into one command.
        let (read_end, slot) = {
            let mut streams = self.streams.lock();
            streams.plan(
                aligned_start,
                aligned_end,
                buf.len(),
                READAHEAD_BYTES,
                self.dev_end,
            )
        };
        let read_len = read_end - aligned_start;

        // No read-ahead and already sector-aligned: read straight into the
        // caller's buffer and cache from it — no temporary allocation.
        if read_end == aligned_end && offset == aligned_start && buf.len() == read_len {
            let n = self.inner.read_at(aligned_start, buf)?;
            self.populate(first, buf, n);
            self.streams.lock().commit(slot, aligned_start + n);
            return Ok(n);
        }

        // Otherwise read the (possibly read-ahead-extended) span once, cache
        // whole sectors, then return just the requested slice.
        let mut tmp = vec![0u8; read_len];
        let n = self.inner.read_at(aligned_start, &mut tmp)?;
        self.populate(first, &tmp, n);
        self.streams.lock().commit(slot, aligned_start + n);
        let skip = offset - aligned_start;
        let avail = n.saturating_sub(skip);
        let copy = min(buf.len(), avail);
        buf[..copy].copy_from_slice(&tmp[skip..skip + copy]);
        Ok(copy)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> DevResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // Write-through first: the disk is authoritative and acknowledged bytes
        // survive a crash regardless of cache state.
        let n = self.inner.write_at(offset, buf)?;
        if n == 0 {
            return Ok(0);
        }
        // Then reconcile the cache with exactly the bytes that landed on disk.
        let end = offset + n;
        let first = offset / SECTOR;
        let last = (end - 1) / SECTOR;
        let mut cache = self.cache.lock();
        for sector in first..=last {
            let s_start = sector * SECTOR;
            let s_end = s_start + SECTOR;
            let cov_start = max(s_start, offset);
            let cov_end = min(s_end, end);
            if cov_start == s_start && cov_end == s_end {
                // Whole sector overwritten: store the authoritative copy.
                cache.insert(sector as u64, &buf[cov_start - offset..cov_end - offset]);
            } else {
                // Partial sector: patch a resident copy so it cannot go stale;
                // if absent, leave it out (disk already has the new bytes).
                cache.patch(
                    sector as u64,
                    cov_start - s_start,
                    &buf[cov_start - offset..cov_end - offset],
                );
            }
        }
        Ok(n)
    }

    fn sync(&self) -> DevResult<()> {
        // Write-through keeps the cache clean, so flushing the backing device is
        // sufficient.
        self.inner.sync()
    }
}

pub fn device_from_backend(backend: &MountBackend) -> VfsResult<Arc<dyn Device>> {
    let (raw, size_bytes): (Arc<dyn Device>, usize) = match backend {
        MountBackend::Block(block) => (
            Arc::new(BlockByteDevice::new(block.clone())) as Arc<dyn Device>,
            block.block_count() * 512,
        ),
        MountBackend::File(file) => {
            let dev = FileByteDevice::new(file.clone())?;
            let size = dev.len;
            (Arc::new(dev) as Arc<dyn Device>, size)
        }
    };
    // Wrap every mount's backing device in the buffer cache. ext2 and btrfs both
    // reach disk exclusively through this handle, so the cache accelerates all
    // of their metadata and data traffic transparently, and read-ahead collapses
    // sequential boot/exec reads into far fewer (synchronous) device commands.
    Ok(Arc::new(CachedDevice::new(raw, size_bytes)))
}

#[cfg(test)]
mod block_byte_tests {
    //! Host tests for the byte<->sector translation in `BlockByteDevice` — the
    //! kernel layer that sits between btrfs and the AHCI driver. btrfs is
    //! exonerated by the `btrfs` crate's own suites; this exercises the layer
    //! below it (partial-sector read-modify-write, multi-sector bulk, and
    //! end-of-device bounds) against an in-memory reference so a translation bug
    //! is caught on the host instead of only on hardware.
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use rcore_fs::dev::Device;
    use std::sync::Mutex;
    use zcore_drivers::scheme::{BlockScheme, Scheme};
    use zcore_drivers::{DeviceError, DeviceResult};

    /// In-memory 512-byte-sector block device. Records every `write_block`
    /// length so the test can also assert how the byte layer chunked the I/O,
    /// and counts sectors fetched via `read_block` so cache tests can prove a
    /// repeat read touched the device zero times.
    struct MockBlock {
        sectors: Mutex<Vec<u8>>,
        nsec: usize,
        sectors_read: AtomicUsize,
    }
    impl MockBlock {
        fn new(nsec: usize) -> Arc<Self> {
            Arc::new(Self {
                sectors: Mutex::new(vec![0u8; nsec * 512]),
                nsec,
                sectors_read: AtomicUsize::new(0),
            })
        }
        /// Total sectors transferred out of the device via `read_block`.
        fn sectors_read(&self) -> usize {
            self.sectors_read.load(Ordering::Relaxed)
        }
    }
    impl Scheme for MockBlock {
        fn name(&self) -> &str {
            "mockblock"
        }
    }
    impl BlockScheme for MockBlock {
        fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DeviceResult {
            if buf.is_empty() || buf.len() % 512 != 0 {
                return Err(DeviceError::InvalidParam);
            }
            let start = block_id * 512;
            let d = self.sectors.lock().unwrap();
            if start + buf.len() > d.len() {
                return Err(DeviceError::InvalidParam);
            }
            buf.copy_from_slice(&d[start..start + buf.len()]);
            self.sectors_read
                .fetch_add(buf.len() / 512, Ordering::Relaxed);
            Ok(())
        }
        fn write_block(&self, block_id: usize, buf: &[u8]) -> DeviceResult {
            if buf.is_empty() || buf.len() % 512 != 0 {
                return Err(DeviceError::InvalidParam);
            }
            let start = block_id * 512;
            let mut d = self.sectors.lock().unwrap();
            if start + buf.len() > d.len() {
                return Err(DeviceError::InvalidParam);
            }
            d[start..start + buf.len()].copy_from_slice(buf);
            Ok(())
        }
        fn flush(&self) -> DeviceResult {
            Ok(())
        }
        fn block_count(&self) -> usize {
            self.nsec
        }
    }

    fn pat(i: usize) -> u8 {
        (i.wrapping_mul(2654435761) >> 13) as u8
    }

    /// Write through the byte device at many aligned/unaligned offsets and
    /// sizes, mirror into a reference buffer, then read every byte back two ways
    /// and require an exact match. Catches off-by-one / partial-sector RMW bugs.
    #[test]
    fn byte_sector_roundtrip() {
        let nsec = 2048; // 1 MiB device
        let dev = MockBlock::new(nsec);
        let bbd = BlockByteDevice::new(dev.clone());
        let total = nsec * 512;
        let mut reference = vec![0u8; total];

        // (offset, len): aligned, unaligned head, unaligned tail, sub-sector,
        // multi-sector spanning, and right up to the last byte.
        let cases = [
            (0usize, 512usize),
            (1, 10),
            (511, 2),
            (500, 100),
            (0, 4096),
            (1234, 5000),
            (512 * 100 + 7, 9000),
            (total - 10, 10),
            (total - 513, 513),
            (777, 1),
        ];
        for (k, &(off, len)) in cases.iter().enumerate() {
            let payload: Vec<u8> = (0..len).map(|i| pat(off + i + k)).collect();
            let w = bbd.write_at(off, &payload).unwrap();
            assert_eq!(w, len, "short write off={off} len={len}");
            reference[off..off + len].copy_from_slice(&payload);
        }

        // Read back the whole device in one call.
        let mut got = vec![0u8; total];
        let mut p = 0;
        while p < total {
            let n = bbd.read_at(p, &mut got[p..]).unwrap();
            assert!(n > 0);
            p += n;
        }
        assert_eq!(got, reference, "full-device readback mismatch");

        // Read back in awkward unaligned slices too.
        for &(off, len) in &cases {
            let mut buf = vec![0u8; len];
            let mut q = 0;
            while q < len {
                let n = bbd.read_at(off + q, &mut buf[q..]).unwrap();
                assert!(n > 0);
                q += n;
            }
            assert_eq!(buf, &reference[off..off + len], "slice readback off={off}");
        }
    }

    /// A large contiguous write (bigger than any single AHCI command would do)
    /// must land correctly sector-for-sector.
    #[test]
    fn large_contiguous_write() {
        let nsec = 4096; // 2 MiB
        let dev = MockBlock::new(nsec);
        let bbd = BlockByteDevice::new(dev.clone());
        let len = 1_500_000usize; // ~1.4 MiB, spans hundreds of sectors
        let payload: Vec<u8> = (0..len).map(pat).collect();
        assert_eq!(bbd.write_at(2048, &payload).unwrap(), len);
        let mut got = vec![0u8; len];
        let mut p = 0;
        while p < len {
            p += bbd.read_at(2048 + p, &mut got[p..]).unwrap();
        }
        assert_eq!(got, payload);
    }

    // ---- CachedDevice (buffer cache) tests ----

    /// Build a buffer-cached device over a fresh `BlockByteDevice` on `dev`.
    fn cached(dev: Arc<MockBlock>, cap_sectors: usize) -> CachedDevice {
        let size = dev.block_count() * 512;
        CachedDevice::with_capacity(Arc::new(BlockByteDevice::new(dev)), size, cap_sectors)
    }

    /// Read-ahead is gated on a PROVEN sequential stream (see `StreamRing`):
    /// the first cold read fetches exactly what was asked — an isolated random
    /// read must never trigger a window that would thrash the cache — and once
    /// byte-exact continuations prove the stream, the window fires and further
    /// sequential reads inside it never touch the device.
    ///
    /// (This test originally asserted a prefetch on the FIRST cold read, the
    /// pre-gating behavior; it predated the confidence gate and could not run
    /// in this environment until the hosted build of the stack was fixed.)
    #[test]
    fn cache_readahead_prefetches_sequential() {
        let dev = MockBlock::new(2048); // 1 MiB
        let raw = BlockByteDevice::new(dev.clone());
        let total = 2048 * 512;
        let data: Vec<u8> = (0..total).map(pat).collect();
        raw.write_at(0, &data).unwrap();

        let cache = cached(dev.clone(), 4096);
        // Cold, unproven stream: exactly the requested sector, no window.
        let mut b0 = [0u8; 512];
        cache.read_at(0, &mut b0).unwrap();
        assert_eq!(&b0[..], &data[0..512]);
        assert_eq!(
            dev.sectors_read(),
            1,
            "an unproven stream must not prefetch (isolated random reads would thrash)"
        );
        // Two byte-exact continuations: whichever of them crosses the
        // confidence threshold, a window has fired by the end of them.
        let mut b1 = [0u8; 512];
        cache.read_at(512, &mut b1).unwrap();
        assert_eq!(&b1[..], &data[512..1024]);
        let mut b2 = [0u8; 512];
        cache.read_at(1024, &mut b2).unwrap();
        assert_eq!(&b2[..], &data[1024..1536]);
        let after_seq = dev.sectors_read();
        assert!(
            after_seq > 3,
            "a proven sequential stream should have prefetched a window (got {} sectors)",
            after_seq
        );
        // Follow-up inside the prefetched window: served from cache only.
        let mut b3 = [0u8; 512];
        cache.read_at(1536, &mut b3).unwrap();
        assert_eq!(&b3[..], &data[1536..2048]);
        assert_eq!(
            dev.sectors_read(),
            after_seq,
            "a sequential read within the prefetch window must not touch the device"
        );
    }

    /// Once a span is resident, repeat reads must not touch the device at all.
    #[test]
    fn cache_serves_repeat_reads() {
        let dev = MockBlock::new(2048);
        let cache = cached(dev.clone(), 4096);
        // Seed disk through the cache (write-through); 8 aligned sectors.
        let data: Vec<u8> = (0..4096).map(pat).collect();
        assert_eq!(cache.write_at(0, &data).unwrap(), 4096);
        let base = dev.sectors_read();
        for _ in 0..16 {
            let mut buf = vec![0u8; 4096];
            assert_eq!(cache.read_at(0, &mut buf).unwrap(), 4096);
            assert_eq!(buf, data);
        }
        assert_eq!(
            dev.sectors_read(),
            base,
            "repeat reads of a resident span must be served from cache"
        );
    }

    /// A cold read fetches from disk and populates the cache; the next identical
    /// read is free.
    #[test]
    fn cache_cold_then_warm() {
        let dev = MockBlock::new(2048);
        // Seed disk directly (bypass cache) so the cache starts cold.
        let raw = BlockByteDevice::new(dev.clone());
        let data: Vec<u8> = (0..8192).map(pat).collect();
        raw.write_at(0, &data).unwrap();

        let cache = cached(dev.clone(), 4096);
        let r0 = dev.sectors_read();
        let mut buf = vec![0u8; 8192];
        cache.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, data);
        let r1 = dev.sectors_read();
        assert!(r1 > r0, "cold read should hit the device");
        cache.read_at(0, &mut buf).unwrap();
        assert_eq!(dev.sectors_read(), r1, "warm read should be cache-only");
        assert_eq!(buf, data);
    }

    /// With a cache far smaller than the working set, every read must still
    /// return exactly what is on disk — eviction never serves stale/corrupt data.
    #[test]
    fn cache_eviction_preserves_correctness() {
        let nsec = 256;
        let dev = MockBlock::new(nsec);
        let cache = cached(dev.clone(), 8); // 8-sector cache vs 256-sector device
        let total = nsec * 512;
        let data: Vec<u8> = (0..total).map(pat).collect();
        assert_eq!(cache.write_at(0, &data).unwrap(), total);
        for k in 0..nsec {
            let s = (k * 397) % nsec; // odd stride -> visits every sector, churns LRU
            let mut buf = [0u8; 512];
            cache.read_at(s * 512, &mut buf).unwrap();
            assert_eq!(&buf[..], &data[s * 512..s * 512 + 512], "sector {s}");
        }
    }

    /// A sub-sector write must update a resident cached sector in place so a
    /// later cache hit reflects the new bytes, and the disk must agree.
    #[test]
    fn cache_partial_write_consistency() {
        let dev = MockBlock::new(64);
        let cache = cached(dev.clone(), 4096);
        // Make sector 0 resident.
        let mut warm = [0u8; 512];
        cache.read_at(0, &mut warm).unwrap();
        // Patch 10 bytes in the middle of sector 0.
        let patch = [0xABu8; 10];
        assert_eq!(cache.write_at(100, &patch).unwrap(), 10);
        // Cache hit must see the patch and leave neighbours untouched.
        let mut got = [0u8; 512];
        cache.read_at(0, &mut got).unwrap();
        assert_eq!(&got[100..110], &patch);
        assert_eq!(got[99], 0);
        assert_eq!(got[110], 0);
        // Disk (read via an independent uncached handle) must agree.
        let raw = BlockByteDevice::new(dev.clone());
        let mut disk = [0u8; 512];
        raw.read_at(0, &mut disk).unwrap();
        assert_eq!(
            &disk[100..110],
            &patch,
            "write-through lost the partial write"
        );
    }

    /// Drive a mix of aligned/unaligned writes through the cache (small cache to
    /// force eviction), then require a full readback through the cache to equal a
    /// full readback taken straight from the backing store. Proves write-through
    /// plus the post-write cache reconciliation stay coherent.
    #[test]
    fn cache_write_through_matches_disk() {
        let nsec = 512;
        let dev = MockBlock::new(nsec);
        let cache = cached(dev.clone(), 64);
        let total = nsec * 512;
        let mut reference = vec![0u8; total];

        let cases = [
            (0usize, 512usize),
            (1, 10),
            (511, 2),
            (500, 100),
            (0, 4096),
            (1234, 5000),
            (512 * 100 + 7, 9000),
            (total - 10, 10),
            (total - 513, 513),
            (777, 1),
        ];
        for (k, &(off, len)) in cases.iter().enumerate() {
            let payload: Vec<u8> = (0..len).map(|i| pat(off + i + k * 31)).collect();
            assert_eq!(cache.write_at(off, &payload).unwrap(), len);
            reference[off..off + len].copy_from_slice(&payload);
        }
        // Readback via the cache.
        let mut via_cache = vec![0u8; total];
        let mut p = 0;
        while p < total {
            let n = cache.read_at(p, &mut via_cache[p..]).unwrap();
            assert!(n > 0);
            p += n;
        }
        assert_eq!(via_cache, reference, "cache readback mismatch");
        // Readback straight from disk via a fresh uncached handle.
        let raw = BlockByteDevice::new(dev.clone());
        let mut via_disk = vec![0u8; total];
        let mut q = 0;
        while q < total {
            let n = raw.read_at(q, &mut via_disk[q..]).unwrap();
            assert!(n > 0);
            q += n;
        }
        assert_eq!(
            via_disk, reference,
            "disk content mismatch (write-through failed)"
        );
    }

    /// Unaligned reads spanning sector boundaries must return the right bytes
    /// whether served cold (from disk) or warm (from cache).
    #[test]
    fn cache_unaligned_reads() {
        let dev = MockBlock::new(64);
        let warm_cache = cached(dev.clone(), 4096);
        let total = 64 * 512;
        let data: Vec<u8> = (0..total).map(pat).collect();
        warm_cache.write_at(0, &data).unwrap(); // populates sectors 0..64 in warm_cache
        for &(off, len) in &[(1usize, 600usize), (511, 3), (1000, 2048), (5, 5000)] {
            // Cold: a fresh cache must reconstruct the span from disk.
            let fresh = cached(dev.clone(), 4096);
            let mut cold = vec![0u8; len];
            fresh.read_at(off, &mut cold).unwrap();
            assert_eq!(cold, data[off..off + len], "cold off={off} len={len}");
            // Warm: the same span served from cache must match.
            let mut warm = vec![0u8; len];
            warm_cache.read_at(off, &mut warm).unwrap();
            assert_eq!(warm, data[off..off + len], "warm off={off} len={len}");
        }
    }

    // ---- BlockCache (LRU buffer cache) unit tests ----
    //
    // These poke the cache directly rather than through `CachedDevice`, pinning
    // the eviction policy and the read/patch/insert bookkeeping that the
    // higher-level coherence tests above rely on.

    #[test]
    fn cache_lru_evicts_least_recently_used() {
        let mut c = BlockCache::new(2);
        c.insert(0, &[0xAA; SECTOR]);
        c.insert(1, &[0xBB; SECTOR]);
        assert!(c.contains(0) && c.contains(1));
        // Touch sector 0 -> sector 1 becomes the least-recently-used victim.
        let mut buf = [0u8; SECTOR];
        assert!(c.read_into(0, 0, &mut buf));
        assert_eq!(buf[0], 0xAA);
        c.insert(2, &[0xCC; SECTOR]);
        assert!(c.contains(0), "recently-read sector 0 stays resident");
        assert!(c.contains(2), "freshly inserted sector 2 present");
        assert!(!c.contains(1), "LRU sector 1 evicted");
    }

    #[test]
    fn cache_read_into_misses_when_absent() {
        let mut c = BlockCache::new(4);
        let mut buf = [0u8; SECTOR];
        assert!(!c.read_into(7, 0, &mut buf));
    }

    #[test]
    fn cache_patch_on_resident_updates_in_place() {
        let mut c = BlockCache::new(4);
        c.insert(3, &[0u8; SECTOR]);
        c.patch(3, 100, &[0x9, 0x9, 0x9]);
        let mut buf = [0u8; SECTOR];
        assert!(c.read_into(3, 0, &mut buf));
        assert_eq!(&buf[100..103], &[0x9, 0x9, 0x9]);
        assert_eq!(buf[99], 0);
    }

    #[test]
    fn cache_patch_on_absent_is_noop() {
        let mut c = BlockCache::new(4);
        c.patch(9, 0, &[1, 2, 3]); // must not create an entry
        assert!(!c.contains(9));
    }

    #[test]
    fn cache_insert_updates_in_place_without_growing() {
        let mut c = BlockCache::new(2);
        c.insert(5, &[0x11; SECTOR]);
        c.insert(5, &[0x22; SECTOR]); // overwrite, not a new entry
        let mut buf = [0u8; SECTOR];
        assert!(c.read_into(5, 0, &mut buf));
        assert_eq!(buf[0], 0x22);
        // Still room for exactly one more before eviction.
        c.insert(6, &[0x33; SECTOR]);
        assert!(c.contains(5) && c.contains(6));
    }

    #[test]
    fn cache_capacity_zero_is_clamped_to_one() {
        let mut c = BlockCache::new(0);
        c.insert(0, &[1; SECTOR]);
        assert!(c.contains(0));
        c.insert(1, &[2; SECTOR]);
        assert!(c.contains(1));
        assert!(
            !c.contains(0),
            "capacity floored at 1 -> one resident sector"
        );
    }
}

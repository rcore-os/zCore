//! File handle for process

use alloc::{boxed::Box, string::String, sync::Arc};

use async_trait::async_trait;
use lock::RwLock;

use rcore_fs::vfs::{FileType, FsError, INode, Metadata, PollStatus, Timespec};
use zircon_object::object::*;
use zircon_object::vm::{pages, VmObject, PAGE_SIZE};

use super::FileLike;
use crate::error::{LxError, LxResult};

bitflags::bitflags! {
    /// File open flags
    pub struct OpenFlags: usize {
        /// read only
        const RDONLY = 0;
        /// write only
        const WRONLY = 1;
        /// read write
        const RDWR = 2;
        /// create file if it does not exist
        const CREATE = 1 << 6;
        /// error if CREATE and the file exists
        const EXCLUSIVE = 1 << 7;
        /// truncate file upon open
        const TRUNCATE = 1 << 9;
        /// append on each write
        const APPEND = 1 << 10;
        /// non block open
        const NON_BLOCK = 1 << 11;
        /// close on exec
        const CLOEXEC = 1 << 19;
    }
}

impl OpenFlags {
    /// check if the OpenFlags is readable
    pub fn readable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::RDONLY.bits() || b == Self::RDWR.bits()
    }
    /// check if the OpenFlags is writable
    pub fn writable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::WRONLY.bits() || b == Self::RDWR.bits()
    }
    /// check if the OpenFlags caontains append
    pub fn is_append(self) -> bool {
        self.contains(Self::APPEND)
    }
    /// check if the OpenFlags caontains non-block
    pub fn non_block(self) -> bool {
        self.contains(Self::NON_BLOCK)
    }
    /// close on exec
    pub fn close_on_exec(self) -> bool {
        self.contains(Self::CLOEXEC)
    }
}

bitflags::bitflags! {
    pub struct PollEvents: u16 {
        /// There is data to read.
        const IN = 0x0001;
        /// Writing is now possible.
        const OUT = 0x0004;
        /// Error condition (return only)
        const ERR = 0x0008;
        /// Hang up (return only)
        const HUP = 0x0010;
        /// Invalid request: fd not open (return only)
        const INVAL = 0x0020;
    }
}

/// file seek type
#[derive(Debug)]
pub enum SeekFrom {
    /// seek from start point
    Start(u64),
    /// seek from end
    End(i64),
    /// seek from current
    Current(i64),
}

/// file inner mut data struct
#[derive(Clone)]
struct FileInner {
    /// content offset on read/write
    offset: u64,
    /// file open options
    flags: OpenFlags,
    /// file INode
    inode: Arc<dyn INode>,
}

/// file implement struct
pub struct File {
    /// object base
    base: KObjectBase,
    /// file path
    path: String,
    /// file inner mut data
    inner: RwLock<FileInner>,
}

impl_kobject!(File);

/// Demand-paging source for a file-backed `mmap` (see [`get_vmo`]).
///
/// Reads one page from the backing inode the first time that page is touched,
/// so a large mapping (e.g. `libLLVM.so`) is paged in lazily instead of being
/// read into memory in full at map time.
///
/// [`get_vmo`]: File::get_vmo
struct FileFrameFiller {
    inode: Arc<dyn INode>,
    /// File offset that VMO offset 0 maps to.
    file_offset: usize,
    /// Number of readable bytes from `file_offset` within the mapping. Pages
    /// past this are left zero (the BSS tail of the mapping).
    source_len: usize,
}

/// Per-inode file-VMO registry — the PAGE CACHE. Entry: `(cache VMO, weak
/// inode handle for pruning, ever_shared)`.
///
/// One VMO per inode serves BOTH mapping flavours: `MAP_SHARED` maps it
/// directly (stores propagate between processes), and `MAP_PRIVATE` creates a
/// borrower over it (`VmObject::new_paged_borrowing`) so clean pages are the
/// cache's frames and only dirtied pages are copied. `ever_shared` records
/// whether a MAP_SHARED mapping was ever handed out: only then can the cache's
/// pages differ from the file, so only then is eviction-time writeback needed
/// -- without the flag every library file would be rewritten with identical
/// bytes when its last user exits.
type SharedVmoMap = alloc::collections::BTreeMap<
    (usize, usize),
    (Arc<VmObject>, alloc::sync::Weak<dyn INode>, bool),
>;

/// Registry key for a file: `(filesystem identity, inode number)`.
///
/// The old key -- the inode ARC's data pointer -- deduplicated nothing across
/// `open()`s: the VFS builds a fresh `Arc<dyn INode>` per lookup, so eight
/// processes opening the same library produced eight distinct "shared" caches
/// (measured: 16 PagedSource VMOs / 512 MiB for 8 readers of one 32 MiB file).
/// It only ever worked when the SAME fd was inherited or SCM_RIGHTS-passed,
/// which is why wl_shm masked it. Files whose filesystem reports inode 0 fall
/// back to the Arc pointer: no cross-open dedup, but never a false merge.
fn cache_key(inode: &Arc<dyn INode>) -> (usize, usize) {
    match inode.metadata() {
        Ok(md) if md.inode != 0 => {
            (Arc::as_ptr(&inode.fs()) as *const () as usize, md.inode)
        }
        _ => (Arc::as_ptr(inode) as *const () as usize, usize::MAX),
    }
}

lazy_static::lazy_static! {
    /// Per-inode shared VMOs for `MAP_SHARED` file mappings, keyed by the
    /// inode's Arc data pointer. Every mapper of the same file gets the SAME
    /// VmObject, so stores by one process are visible to all others — the
    /// wl_shm contract. Without this each mmap produced an independent
    /// demand-paged snapshot: foot rendered its terminal into its own copy
    /// while labwc composited an all-zeros copy — every client window and the
    /// lunarbg background showed pure black.
    ///
    /// The VMO is held with a STRONG ref, anchored to the *inode's* lifetime
    /// (a Weak<INode> alongside it), NOT to any one mapping's. This is what
    /// makes the wl_keyboard keymap work: wlroots writes the keymap into a
    /// memfd via mmap(MAP_SHARED)+memcpy and then *munmaps and closes the write
    /// fd* before sending the read fd — with a Weak VMO the shared pages would
    /// be freed on that munmap, so the client's later read (and the MAP_PRIVATE
    /// COW off this VMO) would see zeros and foot would SIGSEGV on the empty
    /// keymap. There is no MAP_SHARED->inode writeback path (msync is a no-op),
    /// so the shared VMO *is* the file's storage for as long as an fd keeps the
    /// inode alive. Dead-inode entries are pruned on every access, freeing the
    /// VMO (and its frames) once the last fd closes.
    static ref SHARED_FILE_VMOS: lock::Mutex<SharedVmoMap> =
        lock::Mutex::new(alloc::collections::BTreeMap::new());
}

/// Drop shared-VMO entries whose backing inode has been freed (all fds closed).
/// Called under the registry lock before any lookup/insert.
/// Entry count and committed bytes held by the MAP_SHARED file-VMO registry.
///
/// These VMOs are held with a STRONG ref and are NOT attributable to any
/// process: once every mapper has exited, the pages stay committed for as long
/// as the backing inode is alive. If a filesystem caches its inodes, "alive"
/// means forever, and this registry becomes a one-way memory sink. This is the
/// number that turns "physical RAM is exhausted and no process accounts for it"
/// into a specific answer, so `/proc/memhogs` reports it next to the per-process
/// totals.
pub fn shared_file_vmo_stats() -> (usize, u64) {
    let registry = SHARED_FILE_VMOS.lock();
    let mut bytes = 0u64;
    for (vmo, _, _) in registry.values() {
        let pages = vmo.len() / PAGE_SIZE;
        bytes += (vmo.committed_pages_in_range(0, pages) * PAGE_SIZE) as u64;
    }
    (registry.len(), bytes)
}

/// Reference-count histogram of the registry, for `/proc/memhogs`.
///
/// Returns `(entries, sole_vmo_holder, inode_refs_min, inode_refs_max)`:
/// how many entries exist, how many have the registry as the ONLY holder of
/// their VMO (i.e. nothing maps them any more), and the range of strong
/// references on their inodes.
///
/// These two counts decide what a correct eviction rule can look like. The
/// documented rule -- "pruned once the last fd closes" -- is unimplementable as
/// written, because the registry's own entry keeps the inode alive:
///
///   registry --Arc--> VmObject --Arc--> FileFrameFiller --Arc--> INode
///
/// so `inode_weak.strong_count()` can never reach zero while the entry exists.
/// What matters is how much of the count that cycle accounts for.
pub fn shared_file_vmo_refs() -> (usize, usize, usize, usize) {
    let registry = SHARED_FILE_VMOS.lock();
    let mut sole = 0;
    let mut lo = usize::MAX;
    let mut hi = 0;
    for (vmo, inode_weak, _) in registry.values() {
        if Arc::strong_count(vmo) == 1 {
            sole += 1;
        }
        let n = inode_weak.strong_count();
        lo = lo.min(n);
        hi = hi.max(n);
    }
    (
        registry.len(),
        sole,
        if lo == usize::MAX { 0 } else { lo },
        hi,
    )
}

/// Evict registry entries that nothing can reach any more.
///
/// The rule this used to implement -- "drop entries whose backing inode has
/// been freed (all fds closed)" -- was unimplementable as written, because the
/// entry itself keeps the inode alive:
///
///   registry --Arc--> VmObject --Arc--> FileFrameFiller --Arc--> INode
///
/// `inode_weak.strong_count()` therefore never reached 0 and NOTHING was ever
/// pruned. Measured in QEMU: six `mmap(MAP_SHARED)` + munmap + close + unlink
/// cycles over an 8 MiB file left six entries holding 48 MiB, with no mapper
/// left and inode strong refs `1..1` -- that single reference being the cycle's
/// own. Thirty cycles held 240 MiB. Physical use tracked it exactly, and the
/// pages belong to no process, so nothing in per-process accounting showed
/// them. This is what exhausted RAM in the XFCE session (X11 MIT-SHM, Wayland
/// shm pools and font caches all map shared): `frame_alloc FAILED: 2561 MiB
/// used / 2561 MiB managed`, about two minutes in.
///
/// The cycle contributes exactly ONE strong inode reference, which is what
/// makes a correct rule expressible:
///
///   keep while  `Arc::strong_count(vmo) > 1`      something still maps it
///          or   `inode_weak.strong_count() > 1`   some fd is still open
///
/// Both halves matter. The second is what makes the wl_keyboard keymap work --
/// a writer that mmaps, writes, munmaps and closes its fd while the reader's fd
/// is already open keeps the entry, which is the case the strong ref was added
/// for in the first place.

/// Get-or-create the per-inode page-cache VMO, covering at least
/// `offset + len` bytes (grown to the file size). `mark_shared` records that a
/// MAP_SHARED mapping was handed out, which is what arms eviction-time
/// writeback. Returns `None` when an existing cache is too short for the
/// requested window (file grew after creation) -- the caller falls back to a
/// private snapshot exactly as before.
fn inode_cache_vmo(
    inode: &Arc<dyn INode>,
    path: &str,
    file_size: usize,
    offset: usize,
    len: usize,
    mark_shared: bool,
) -> Option<Arc<VmObject>> {
    let key = cache_key(inode);
    let mut registry = SHARED_FILE_VMOS.lock();
    prune_shared_vmos(&mut registry);
    if let Some((vmo, inode_weak, ever_shared)) = registry.get_mut(&key) {
        if offset + len > vmo.len() {
            return None;
        }
        if mark_shared {
            *ever_shared = true;
        }
        // Point the weak handle at the LATEST opener's Arc: the one captured
        // at creation dies with its fd even while other opens keep the file
        // busy, and eviction-time writeback needs a live inode to write to.
        *inode_weak = Arc::downgrade(inode);
        return Some(vmo.clone());
    }
    // Cover the whole file (so later mappers at other offsets share it too),
    // demand-paged from the inode. Created under the registry lock so a
    // concurrent first-map cannot race us into two caches.
    let vmo_len = file_size.max(offset + len);
    let source: Arc<dyn zircon_object::vm::FrameFiller> = Arc::new(FileFrameFiller {
        inode: inode.clone(),
        file_offset: 0,
        source_len: file_size,
    });
    let vmo = VmObject::new_paged_with_source(pages(vmo_len), source);
    vmo.set_name(path);
    registry.insert(key, (vmo.clone(), Arc::downgrade(inode), mark_shared));
    Some(vmo)
}


fn prune_shared_vmos(registry: &mut SharedVmoMap) {
    registry.retain(|_, (vmo, inode_weak, ever_shared)| {
        if Arc::strong_count(vmo) > 1 || inode_weak.strong_count() > 1 {
            return true;
        }
        // Sole cycle holder: drop the entry. Writeback is only for durable
        // (still-linked) files so a later open sees MAP_SHARED writes.
        //
        // memfd / unlinked shm (`nlinks == 0`) dies with the inode — densifying
        // into ramfs first doubles residency (VMO frames still held + one
        // heap 4KiB block per page) and was the desktop-start OOM
        // (`4096B x ~99000`, leaktrace → `PagedBytes::write_at`).
        if *ever_shared {
            if let Some(inode) = inode_weak.upgrade() {
                let nlinks = inode.metadata().map(|m| m.nlinks).unwrap_or(0);
                if nlinks > 0 {
                    writeback_shared_vmo(vmo, &inode);
                }
            }
        }
        false
    });
}

/// Flush a shared VMO's committed pages to its inode before the VMO is dropped.
///
/// There is no MAP_SHARED->inode writeback path in this kernel (msync is a
/// no-op), so while the VMO lives it *is* the file's storage. Evicting it
/// without this would turn "write through a shared mapping, close everything,
/// reopen" into stale zeros. Doing it here also makes those writes visible to a
/// plain `read()`, which they never were.
///
/// Bounded by the inode's CURRENT size: a shared VMO is rounded up to whole
/// pages and may be longer than the file (`vmo_len = file_size.max(offset+len)`),
/// and writing those pages back would silently EXTEND the file.
fn writeback_shared_vmo(vmo: &Arc<VmObject>, inode: &Arc<dyn INode>) {
    let size = match inode.metadata() {
        Ok(m) => m.size,
        Err(_) => return,
    };
    if size == 0 {
        return;
    }
    let mut buf = alloc::vec![0u8; PAGE_SIZE];
    for idx in 0..vmo.len() / PAGE_SIZE {
        let offset = idx * PAGE_SIZE;
        if offset >= size {
            break;
        }
        // Only pages that were actually faulted in can differ from the file.
        if vmo.committed_pages_in_range(idx, idx + 1) == 0 {
            continue;
        }
        let n = PAGE_SIZE.min(size - offset);
        if vmo.read(offset, &mut buf[..n]).is_err() {
            continue;
        }
        // Keep ramfs holes as holes: writing zeros would allocate a 4KiB heap
        // block per page and recreate the densification OOM on sparse pools.
        if buf[..n].iter().all(|&b| b == 0) {
            continue;
        }
        // Best effort: a read-only filesystem, or an inode that no longer
        // accepts writes, must not turn eviction into a failure.
        let _ = inode.write_at(offset, &buf[..n]);
    }
}

impl zircon_object::vm::FrameFiller for FileFrameFiller {
    fn source_len(&self) -> usize {
        self.source_len
    }

    fn fill_page(&self, offset: usize, buf: &mut [u8]) {
        if offset >= self.source_len {
            return;
        }
        let want = (self.source_len - offset).min(buf.len());
        let file_pos = self.file_offset + offset;
        let mut done = 0;
        while done < want {
            match self.inode.read_at(file_pos + done, &mut buf[done..want]) {
                Ok(0) => break,
                Ok(n) => done += n,
                // A read error mid-mapping leaves the rest zero-filled; the
                // faulting access proceeds rather than wedging the kernel.
                Err(_) => break,
            }
        }
    }
}

// NOTE: the former `VmoFrameFiller` (a MAP_PRIVATE copy sourced from a live
// MAP_SHARED VMO — the wl_keyboard keymap coherence path) is gone: private
// mappings now BORROW from the same per-inode cache VMO the shared mappings
// write into, so they read those writes directly instead of copying them.

impl FileInner {
    /// write to file
    fn write(&mut self, buf: &[u8]) -> LxResult<usize> {
        let offset = if self.flags.is_append() {
            self.inode.metadata()?.size as u64
        } else {
            self.offset
        };
        let len = self.write_at(offset, buf)?;
        self.offset = offset + len as u64;
        Ok(len)
    }

    /// write to file at given offset
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        if !self.flags.writable() {
            return Err(LxError::EBADF);
        }
        let len = self.inode.write_at(offset as usize, buf)?;
        Ok(len)
    }
}

impl File {
    /// create a file struct
    pub fn new(inode: Arc<dyn INode>, flags: OpenFlags, path: String) -> Arc<Self> {
        Arc::new(File {
            base: KObjectBase::new(),
            path,
            inner: RwLock::new(FileInner {
                offset: 0,
                flags,
                inode,
            }),
        })
    }

    /// Returns the file path.
    pub fn path(&self) -> &String {
        &self.path
    }

    /// seek from given type and offset
    pub fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        let mut inner = self.inner.write();
        // Compute the new offset with checked arithmetic and reject results
        // that would be negative; otherwise a negative relative seek would wrap
        // to a huge `u64` and let later reads/writes use an out-of-range offset.
        let new_offset: i64 = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => (inner.inode.metadata()?.size as i64)
                .checked_add(offset)
                .ok_or(LxError::EINVAL)?,
            SeekFrom::Current(offset) => (inner.offset as i64)
                .checked_add(offset)
                .ok_or(LxError::EINVAL)?,
        };
        if new_offset < 0 {
            return Err(LxError::EINVAL);
        }
        inner.offset = new_offset as u64;
        Ok(inner.offset)
    }

    /// resize the file
    pub fn set_len(&self, len: u64) -> LxResult {
        let inner = self.inner.write();
        if !inner.flags.writable() {
            return Err(LxError::EBADF);
        }
        inner.inode.resize(len as usize)?;
        Ok(())
    }

    /// Sync all data and metadata
    pub fn sync_all(&self) -> LxResult {
        self.inner.read().inode.sync_all()?;
        Ok(())
    }

    /// Sync data (not include metadata)
    pub fn sync_data(&self) -> LxResult {
        self.inner.read().inode.sync_data()?;
        Ok(())
    }

    /// get metadata of file
    /// fstat
    pub fn metadata(&self) -> LxResult<Metadata> {
        Ok(self.inner.read().inode.metadata()?)
    }

    /// lookup the file following the link
    pub fn lookup_follow(&self, path: &str, max_follow: usize) -> LxResult<Arc<dyn INode>> {
        Ok(self.inner.read().inode.lookup_follow(path, max_follow)?)
    }

    /// get the name of dir entry
    pub fn read_entry(&self) -> LxResult<String> {
        Ok(self.read_entry_with_metadata()?.1)
    }

    /// get the next directory entry and its metadata
    pub fn read_entry_with_metadata(&self) -> LxResult<(Metadata, String)> {
        let mut inner = self.inner.write();
        if !inner.flags.readable() {
            return Err(LxError::EBADF);
        }
        let offset = inner.offset as usize;
        match inner.inode.get_entry_with_metadata(offset) {
            Ok(entry) => {
                inner.offset += 1;
                Ok(entry)
            }
            Err(e) => {
                // `get_entry_with_metadata`'s default implementation resolves
                // the entry's metadata via `find(name)`, which can fail even
                // though the entry exists — e.g. the devfs root's ".." has no
                // parent, so `find("..")` returns EntryNotFound. Treating that
                // as end-of-directory truncates the listing (this made
                // `ls /dev` appear empty). Distinguish the two: if the name
                // still resolves, emit it with a synthetic directory metadata;
                // only a missing name means we reached the end.
                let name = inner.inode.get_entry(offset).map_err(|_| e)?;
                inner.offset += 1;
                let meta = Metadata {
                    dev: 0,
                    inode: 0,
                    size: 0,
                    blk_size: 0,
                    blocks: 0,
                    atime: Timespec { sec: 0, nsec: 0 },
                    mtime: Timespec { sec: 0, nsec: 0 },
                    ctime: Timespec { sec: 0, nsec: 0 },
                    type_: FileType::Dir,
                    mode: 0,
                    nlinks: 1,
                    uid: 0,
                    gid: 0,
                    rdev: 0,
                };
                Ok((meta, name))
            }
        }
    }

    /// get INode of this file
    pub fn inode(&self) -> Arc<dyn INode> {
        self.inner.read().inode.clone()
    }
}

#[async_trait]
impl FileLike for File {
    fn flags(&self) -> OpenFlags {
        self.inner.read().flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let flags = &mut self.inner.write().flags;
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            path: self.path.clone(),
            inner: RwLock::new(self.inner.read().clone()),
        })
    }

    fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        // Delegate to the inherent offset-tracking seek (UFCS avoids recursing
        // into this trait method).
        File::seek(self, pos)
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        let (offset, flags, inode) = {
            let inner = self.inner.read();
            (inner.offset, inner.flags, inner.inode.clone())
        };

        if !flags.readable() {
            return Err(LxError::EBADF);
        }

        let len = if !flags.non_block() {
            // block
            loop {
                match inode.read_at(offset as usize, buf) {
                    Ok(read_len) => break read_len,
                    Err(FsError::Again) => {
                        // DRM card fd: park on the shared eventbus directly.
                        // Going through INode::async_poll Box::pin's another
                        // async loop on top of this already-boxed File::read
                        // future — deep enough under page-flip storms to
                        // contribute to coroutine stack overflow at labwc start.
                        use super::devfs::DrmDev;
                        if inode.downcast_ref::<DrmDev>().is_some() {
                            let bus = super::devfs::drm::get_eventbus();
                            crate::sync::wait_for_event(bus, crate::sync::Event::READABLE).await;
                        } else {
                            inode.async_poll().await?;
                        }
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        } else {
            inode.read_at(offset as usize, buf)?
        };

        let mut inner = self.inner.write();
        inner.offset += len as u64;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        self.inner.write().write(buf)
    }

    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        let (flags, inode) = {
            let inner = self.inner.read();
            (inner.flags, inner.inode.clone())
        };

        if !flags.readable() {
            return Err(LxError::EBADF);
        }

        if !flags.non_block() {
            // block
            loop {
                match inode.read_at(offset as usize, buf) {
                    Ok(read_len) => return Ok(read_len),
                    Err(FsError::Again) => {
                        use super::devfs::DrmDev;
                        if inode.downcast_ref::<DrmDev>().is_some() {
                            let bus = super::devfs::drm::get_eventbus();
                            crate::sync::wait_for_event(bus, crate::sync::Event::READABLE).await;
                        } else {
                            inode.async_poll().await?;
                        }
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        let len = inode.read_at(offset as usize, buf)?;
        Ok(len)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        self.inner.write().write_at(offset, buf)
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        let inner = self.inner.read();
        // A FIFO node opened from the fs has no pipe-buffer / writer tracking
        // here, so a reader polling an empty FIFO (e.g. a control FIFO that
        // never gets a writer) would spin: the node reads as an
        // empty regular file (0 bytes = EOF) yet polls "readable". Treat it as
        // readable only when it actually holds bytes, so the reader blocks
        // instead of busy-looping on repeated 0-byte reads.
        //
        // Use metadata() best-effort: some fds (sockets, special devices) don't
        // implement it and return ENOSYS — those must fall through to the inode's
        // own poll(), NOT propagate the error (that regressed `poll()` on packet
        // sockets, e.g. udhcpc, to "Function not implemented").
        if let Ok(meta) = inner.inode.metadata() {
            if meta.type_ == FileType::NamedPipe {
                return Ok(PollStatus {
                    read: meta.size > 0,
                    write: true,
                    error: false,
                });
            }
        }
        Ok(inner.inode.poll()?)
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        let inode = self.inner.read().inode.clone();

        // Special-case DrmDev: since INode::async_poll doesn't accept the requested
        // PollEvents, its default implementation blocks on Event::READABLE even if
        // the caller only polled for writability (which is currently always ready
        // on DRM — see DrmDev::poll). Intercept here and return immediately if
        // the requested events are ready; otherwise use the flat DrmEventWait
        // behind INode::async_poll (no nested async-loop state machine).
        use super::devfs::DrmDev;
        if let Some(drmdev) = inode.downcast_ref::<DrmDev>() {
            let want_read = _events.contains(PollEvents::IN);
            let want_write = _events.contains(PollEvents::OUT);
            let status = drmdev.poll()?;
            let ready = (want_read && status.read)
                || (want_write && status.write)
                || (!want_read && !want_write);
            if ready {
                return Ok(status);
            }
            return Ok(inode.async_poll().await?);
        }

        // See `poll`: special-case an empty FIFO so the reader blocks, but only
        // when metadata() is available — sockets/special devices return ENOSYS
        // and must fall through to the inode's own async_poll() rather than
        // failing the whole poll() syscall.
        if let Ok(meta) = inode.metadata() {
            if meta.type_ == FileType::NamedPipe {
                return Ok(PollStatus {
                    read: meta.size > 0,
                    write: true,
                    error: false,
                });
            }
        }
        Ok(inode.async_poll().await?)
    }

    fn subscribe_readiness(
        &self,
        events: PollEvents,
        waker: &core::task::Waker,
    ) -> Option<crate::sync::ReadinessSub> {
        let inode = self.inner.read().inode.clone();
        // DRM card fd: park on the shared DRM event bus (same detection the
        // blocking-read path uses above).
        use super::devfs::DrmDev;
        if inode.downcast_ref::<DrmDev>().is_some() {
            let bus = super::devfs::drm::get_eventbus();
            let mask = super::poll_events_to_bus_mask(events);
            return Some(crate::sync::subscribe_readiness_on(&bus, mask, waker));
        }
        if let Some(pipe) = inode.downcast_ref::<super::pipe::Pipe>() {
            return Some(pipe.subscribe_readiness(events, waker));
        }
        if let Some(master) = inode.downcast_ref::<super::pty::PtyMaster>() {
            return Some(master.subscribe_readiness(events, waker));
        }
        if let Some(slave) = inode.downcast_ref::<super::pty::PtySlave>() {
            return Some(slave.subscribe_readiness(events, waker));
        }
        // Regular files / directories are always ready (poll never parks on
        // them), and everything else — device nodes without an event bus,
        // FIFOs resolved through the fs — keeps the caller's short re-poll
        // backstop by reporting "not subscribable".
        None
    }

    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        // ioctl syscall
        self.inner.read().inode.io_control(request as u32, arg1)?;
        Ok(0)
    }

    fn is_input_device(&self) -> bool {
        use super::devfs::{EventDev, MiceDev};
        let inode = self.inner.read().inode.clone();
        inode.downcast_ref::<MiceDev>().is_some() || inode.downcast_ref::<EventDev>().is_some()
    }

    fn is_char_device(&self) -> bool {
        self.metadata()
            .map(|m| m.type_ == FileType::CharDevice)
            .unwrap_or(false)
    }

    /// Returns the [`VmObject`] representing the file with given `offset` and `len`.
    fn get_vmo(&self, offset: usize, len: usize) -> LxResult<Arc<VmObject>> {
        let inner = self.inner.read();
        match inner.inode.metadata()?.type_ {
            FileType::File => {
                // Back the file mapping with a *paged* (non-contiguous) VMO that
                // is demand-paged from the file: each page is read in on the
                // page fault that first touches it, instead of reading the whole
                // mapping up front.
                //
                // Eagerly reading the whole mapping used to stall the machine:
                // the dynamic linker maps a library's entire LOAD span in one
                // `mmap`, and the ~150 MiB `libLLVM.so` pulled in by `perf`
                // forced ~9.6k synchronous 16 KiB reads plus a full commit of
                // every page before the syscall returned — on real hardware that
                // looked like a hard freeze (couldn't even switch VT). A non-PIE
                // program only touches a fraction of such a library, so paging it
                // in on demand reads (and commits) only what is actually used.
                //
                // The source captures the file inode and the file offset; bytes
                // past end-of-file stay zero (the BSS tail of a file mapping).
                let file_size = inner.inode.metadata()?.size;

                // MAP_PRIVATE borrows from the per-inode page cache: clean
                // pages resolve to the cache's frames (shared by every process
                // mapping this file) and the first write copies just that page.
                // This is what keeps N GTK processes from holding N private
                // copies of libgtk/libglib -- the failure measured as three
                // 266 MiB processes and OOM half a minute into the session.
                // It also subsumes the old coherence special-case for files
                // with a live MAP_SHARED VMO: the borrower reads the very same
                // cache those writes land in.
                //
                // An unaligned offset cannot borrow (frames are page-grained);
                // a cache too short for the window (file grew) declines. Both
                // fall back to the private demand-paged snapshot below.
                if offset.is_multiple_of(PAGE_SIZE) {
                    if let Some(cache) = inode_cache_vmo(
                        &inner.inode,
                        &self.path,
                        file_size,
                        offset,
                        len,
                        false,
                    ) {
                        let vmo = VmObject::new_paged_borrowing(pages(len), cache, offset);
                        vmo.set_name(&self.path);
                        return Ok(vmo);
                    }
                }

                let source_len = file_size.saturating_sub(offset).min(len);
                if len >= 16 * 1024 * 1024 {
                    info!(
                        "get_vmo: demand-paged file map len={} MiB offset={:#x} source={} MiB",
                        len / (1024 * 1024),
                        offset,
                        source_len / (1024 * 1024),
                    );
                }
                let source: Arc<dyn zircon_object::vm::FrameFiller> = Arc::new(FileFrameFiller {
                    inode: inner.inode.clone(),
                    file_offset: offset,
                    source_len,
                });
                // Name the VMO after the file it is paged from. The name is what
                // `/proc/<pid>/maps` shows, and what turns a bare crash address
                // into "<library>+<offset>" in the [exit]/[crash-bt] dumps — a
                // constant `pc=0x499f8c` is useless on its own, but
                // `libglib-2.0.so.0+0x…` can be fed straight to addr2line.
                let vmo = VmObject::new_paged_with_source(pages(len), source);
                vmo.set_name(&self.path);
                Ok(vmo)
            }
            FileType::CharDevice => {
                use super::devfs::{DrmDev, FbDev};
                if let Some(fbdev) = inner.inode.downcast_ref::<FbDev>() {
                    fbdev.get_vmo(offset, len)
                } else if let Some(drmdev) = inner.inode.downcast_ref::<DrmDev>() {
                    drmdev.get_vmo(offset, len).map_err(Into::into)
                } else {
                    Err(LxError::ENOSYS)
                }
            }
            _ => Err(LxError::ENOSYS),
        }
    }

    fn get_vmo_shared(&self, offset: usize, len: usize) -> LxResult<(Arc<VmObject>, usize)> {
        let inner = self.inner.read();
        if inner.inode.metadata()?.type_ != FileType::File {
            // Devices keep their own get_vmo (fb/drm are inherently shared).
            drop(inner);
            return self.get_vmo(offset, len).map(|vmo| (vmo, 0));
        }
        if !offset.is_multiple_of(4096) {
            return Err(LxError::EINVAL);
        }
        let file_size = inner.inode.metadata()?.size;
        // One VMO per inode, shared with the MAP_PRIVATE borrowers; marking it
        // `shared` is what arms eviction-time writeback (a MAP_SHARED mapping
        // can dirty the cache's own pages; borrowers cannot). Held STRONG,
        // anchored to the inode's lifetime (see SHARED_FILE_VMOS) so MAP_SHARED
        // writes survive the writer's munmap — the wl_keyboard keymap depends
        // on this.
        if let Some(vmo) =
            inode_cache_vmo(&inner.inode, &self.path, file_size, offset, len, true)
        {
            return Ok((vmo, offset));
        }
        // Mapping reaches past the cache VMO (file grew after creation). Rare;
        // fall back to a snapshot rather than silently truncating — but warn,
        // because writes through this mapping will NOT be visible to other
        // mappers.
        warn!(
            "get_vmo_shared: mapping {:#x}+{:#x} exceeds the cache vmo for {} — snapshot fallback",
            offset,
            len,
            self.path()
        );
        drop(inner);
        self.get_vmo(offset, len).map(|vmo| (vmo, 0))
    }
}
